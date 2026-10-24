//! Persistent frame decoding for smooth scrubbing.
//!
//! [`FfmpegFrameExtractor`] starts one ffmpeg process per frame and decodes
//! from the previous keyframe every time. [`FfmpegFrameStream`] instead keeps
//! one ffmpeg *session* running per video: a child that seeks once and then
//! streams every following frame as raw RGB24 on stdout, while the
//! `showinfo` filter reports each frame's timestamp and size on stderr.
//! Decoded frames are kept in a small byte-bounded cache, so:
//!
//! * stepping or dragging forward reads the next frames from the running
//!   session (no process start, no re-decoding from the keyframe);
//! * stepping backward starts a session a short window before the target and
//!   caches the whole window, so the following backward steps are cache hits;
//! * long jumps start a session exactly where the extractor would seek.
//!
//! Frame selection reproduces the extractor's rule exactly, in stream time
//! base ticks: the extractor's accurate seek to `S = target - frame_step / 2`
//! keeps the first frame whose timestamp is `>= round(S + start_time)`, and
//! returns the last frame of the video when there is none. Without `-copyts`
//! ffmpeg shifts a session's timestamps by `-round(seek + start_time)`, so a
//! frame's absolute timestamp is `pts + round(seek + start_time)`; the
//! container start time comes from the input summary ffmpeg prints. The
//! RGB conversion is the same swscale conversion the extractor uses (for HDR
//! sources the same tone-mapping chain, [`crate::color::display_filter`]),
//! so the returned pixels are identical. Any session failure falls back to a
//! one-off [`FfmpegFrameExtractor`] call for that request.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use image::RgbImage;

use crate::color;
use crate::error::Result;
use crate::ffmpeg::{self, Tool};
use crate::frames::{FfmpegFrameExtractor, FrameExtractor, format_secs, seek_point_us};
use crate::models::VideoFile;

/// A stateful frame provider for one video, used for interactive scrubbing.
pub trait FrameSource: Send {
    /// The frame nearest to `offset_ms`, with the same contract and the same
    /// pixels as [`FrameExtractor::extract`] for this source's video.
    fn frame_at(&mut self, offset_ms: u64) -> Result<RgbImage>;
}

/// A [`FrameSource`] that runs a [`FrameExtractor`] for every request.
pub struct ExtractorFrameSource {
    extractor: Arc<dyn FrameExtractor>,
    video: VideoFile,
}

impl ExtractorFrameSource {
    pub fn new(extractor: Arc<dyn FrameExtractor>, video: VideoFile) -> Self {
        Self { extractor, video }
    }
}

impl FrameSource for ExtractorFrameSource {
    fn frame_at(&mut self, offset_ms: u64) -> Result<RgbImage> {
        self.extractor.extract(&self.video, offset_ms)
    }
}

/// Bound on the bytes of decoded frames kept per stream for videos up to
/// 1080p (about 50 frames at 1080p).
pub const DEFAULT_CACHE_BYTES: usize = 300 << 20;

/// Cache bound for videos larger than 1080p (about 43 frames at 4K, enough
/// for a ~21-frame backward window).
pub const LARGE_CACHE_BYTES: usize = 1 << 30;

/// The cache budget [`FfmpegFrameStream::new`] uses for `video`.
pub fn cache_budget_for(video: &VideoFile) -> usize {
    let pixels = u64::from(video.width) * u64::from(video.height);
    if pixels > 1920 * 1080 {
        LARGE_CACHE_BYTES
    } else {
        DEFAULT_CACHE_BYTES
    }
}

/// Most frames a backward step decodes ahead of time (about one second).
const MAX_BACK_WINDOW_FRAMES: u64 = 25;

/// Longest forward distance ever read through instead of seeking; within
/// it, [`Costs`] decides which of the two is cheaper.
const READ_AHEAD_MS: u64 = 2_000;

/// Weight of a new measurement in the running [`Costs`] estimates.
const COST_SMOOTHING: f64 = 0.3;

/// Look-back windows (ms) tried when no frame exists at or after the target,
/// after a window of four frame steps (as in the extractor).
const END_WINDOWS_MS: [u64; 3] = [1_000, 10_000, 60_000];

/// Persistent ffmpeg decoding session plus frame cache for one video.
pub struct FfmpegFrameStream {
    video: VideoFile,
    cache: FrameCache,
    clock: Option<Clock>,
    session: Option<Session>,
    /// Selection threshold of the previous request (detects backward steps).
    previous: Option<i64>,
    costs: Costs,
    sessions_started: u64,
    fallbacks: u64,
}

/// Running estimates (seconds) of reading one more frame from a session and
/// of answering with a new session, to pick the cheaper way forward.
#[derive(Debug, Clone, Copy)]
struct Costs {
    frame: f64,
    seek: f64,
}

impl Costs {
    /// Guesses from the frame size until measurements arrive: about 0.8 GB/s
    /// through decode, conversion and the pipe, and a seek costing process
    /// start-up plus roughly ten frames.
    fn initial(frame_bytes: u64) -> Self {
        let frame = frame_bytes as f64 / 800e6;
        Self {
            frame,
            seek: 0.08 + 10.0 * frame,
        }
    }

    fn update(estimate: &mut f64, started: Instant) {
        *estimate += COST_SMOOTHING * (started.elapsed().as_secs_f64() - *estimate);
    }
}

impl FfmpegFrameStream {
    /// A stream for `video` with the cache budget for its resolution
    /// ([`cache_budget_for`]).
    pub fn new(video: VideoFile) -> Self {
        let budget = cache_budget_for(&video);
        Self::with_cache_budget(video, budget)
    }

    /// A stream keeping at most about `budget_bytes` of decoded frames.
    pub fn with_cache_budget(video: VideoFile, budget_bytes: usize) -> Self {
        let mut stream = Self {
            video,
            cache: FrameCache::new(budget_bytes),
            clock: None,
            session: None,
            previous: None,
            costs: Costs::initial(0),
            sessions_started: 0,
            fallbacks: 0,
        };
        stream.costs = Costs::initial(stream.frame_bytes());
        stream
    }

    /// How many ffmpeg sessions this stream has started (for diagnostics).
    pub fn sessions_started(&self) -> u64 {
        self.sessions_started
    }

    /// How many requests fell back to a one-off extraction (for diagnostics).
    pub fn fallbacks(&self) -> u64 {
        self.fallbacks
    }

    fn step_us(&self) -> u64 {
        self.video.frame_step_ms.max(1) * 1000
    }

    /// Expected size of one RGB frame (1080p when the size is unknown).
    fn frame_bytes(&self) -> u64 {
        match u64::from(self.video.width) * u64::from(self.video.height) * 3 {
            0 => 1920 * 1080 * 3,
            bytes => bytes,
        }
    }

    /// Frames cached ahead of a backward step: half the cache, at most ~1 s.
    fn back_window_frames(&self) -> u64 {
        (self.cache.budget as u64 / self.frame_bytes() / 2).clamp(1, MAX_BACK_WINDOW_FRAMES)
    }

    /// The frame for `offset_ms` from the cache or a session, or the reason
    /// the session could not provide it.
    fn stream_frame(&mut self, offset_ms: u64) -> Result<Arc<RgbImage>, String> {
        let seek_us = seek_point_us(&self.video, offset_ms);
        let previous = self.previous.take();
        let started = Instant::now();
        // Set when a new session seeks exactly to the target (a seek sample).
        let mut exact_seek = false;
        let threshold = match self.clock {
            Some(clock) => {
                let threshold = clock.threshold(seek_us);
                if let Some(frame) = self.cache.lookup(threshold) {
                    self.previous = Some(threshold);
                    return Ok(frame);
                }
                if !self.can_read_ahead(clock, threshold) {
                    let start = self.session_start(clock, seek_us, threshold, previous);
                    exact_seek = start == seek_us;
                    self.restart(start)?;
                }
                threshold
            }
            None => {
                exact_seek = true;
                self.restart(seek_us)?;
                self.clock
                    .ok_or("no video frame at or after the seek point")?
                    .threshold(seek_us)
            }
        };
        self.previous = Some(threshold);
        if let Some(frame) = self.read_until(threshold)? {
            if exact_seek {
                Costs::update(&mut self.costs.seek, started);
            }
            return Ok(frame);
        }
        self.frame_past_end(offset_ms, threshold)
    }

    /// Whether the running session reaches `threshold` by reading forward.
    fn can_read_ahead(&self, clock: Clock, threshold: i64) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        if session.eof || session.origin.is_none() {
            return false;
        }
        let next = session.covered_from;
        let distance = threshold.saturating_sub(next);
        if threshold < next || distance > clock.ticks(READ_AHEAD_MS as i64 * 1000) {
            return false;
        }
        let step = clock.ticks(self.step_us() as i64).max(1);
        let frames = (distance / step + 1) as f64;
        frames * self.costs.frame <= self.costs.seek
    }

    /// Where a new session starts: a window before the target when stepping
    /// backward (so the next steps are cached), else where the extractor seeks.
    fn session_start(
        &self,
        clock: Clock,
        seek_us: u64,
        threshold: i64,
        previous: Option<i64>,
    ) -> u64 {
        let back_us = self.back_window_frames() * self.step_us();
        let back_ticks = clock.ticks(back_us as i64);
        let stepping_back =
            previous.is_some_and(|p| threshold < p && p.saturating_sub(threshold) <= back_ticks);
        if stepping_back {
            seek_us.saturating_sub(back_us)
        } else {
            seek_us
        }
    }

    /// Replace the session with a new one seeking to `start_us`.
    fn restart(&mut self, start_us: u64) -> Result<(), String> {
        // Stop the old child first: only one decoder runs at a time.
        self.session = None;
        let mut session = Session::spawn(&self.video.path, start_us)?;
        self.sessions_started += 1;
        if let Some(clock) = session.clock()? {
            if self.clock.is_some_and(|known| known != clock) {
                self.cache.clear();
            }
            self.clock = Some(clock);
            session.origin = Some(clock.origin(start_us));
            session.covered_from = clock.threshold(start_us);
        }
        self.session = Some(session);
        Ok(())
    }

    /// Read (and cache) session frames until the first one at or after
    /// `threshold`; `None` at the end of the video.
    fn read_until(&mut self, threshold: i64) -> Result<Option<Arc<RgbImage>>, String> {
        let Some(session) = self.session.as_mut() else {
            return Ok(None);
        };
        loop {
            // The first frame of a session includes the seek; time the rest.
            let started = (session.frames_read > 0).then(Instant::now);
            let Some((tick, frame)) = session.next_frame()? else {
                break;
            };
            if let Some(started) = started {
                Costs::update(&mut self.costs.frame, started);
            }
            let from = session.covered_from;
            if tick < from {
                return Err(format!("frame timestamp {tick} went back before {from}"));
            }
            let frame = Arc::new(frame);
            let focus = if threshold == i64::MIN {
                tick
            } else {
                threshold
            };
            self.cache.insert(from, tick, Arc::clone(&frame), focus);
            session.covered_from = tick.saturating_add(1);
            if tick >= threshold {
                return Ok(Some(frame));
            }
        }
        if let Some(last) = session.last_tick {
            self.cache.set_last_frame(last);
        }
        Ok(None)
    }

    /// No frame exists at or after the target: return the video's last
    /// frame, found like the extractor does with growing look-back windows.
    fn frame_past_end(&mut self, offset_ms: u64, threshold: i64) -> Result<Arc<RgbImage>, String> {
        if let Some(frame) = self.cache.lookup(threshold) {
            return Ok(frame);
        }
        let target_us = offset_ms * 1000;
        let step_ms = self.video.frame_step_ms.max(1);
        let windows = std::iter::once(step_ms * 4).chain(END_WINDOWS_MS);
        for window_ms in windows {
            let start_us = target_us.saturating_sub(window_ms * 1000);
            self.restart(start_us)?;
            self.read_until(i64::MAX)?;
            if let Some(frame) = self.cache.lookup(threshold) {
                return Ok(frame);
            }
            if start_us == 0 {
                break;
            }
        }
        Err("no frame decoded after seek".into())
    }
}

impl FrameSource for FfmpegFrameStream {
    fn frame_at(&mut self, offset_ms: u64) -> Result<RgbImage> {
        if offset_ms > self.video.duration_ms {
            // The extractor reports the out-of-range error without decoding.
            return FfmpegFrameExtractor.extract(&self.video, offset_ms);
        }
        match self.stream_frame(offset_ms) {
            Ok(frame) => Ok(RgbImage::clone(&frame)),
            Err(cause) => {
                log::warn!(
                    "frame stream for '{}' failed at {offset_ms} ms ({cause}); \
                     extracting the frame on its own",
                    self.video.path.display()
                );
                self.session = None;
                self.fallbacks += 1;
                FfmpegFrameExtractor.extract(&self.video, offset_ms)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// Stream time base plus container start time: converts seek points to the
/// stream's timestamp ticks exactly like ffmpeg does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Clock {
    num: i64,
    den: i64,
    start_us: i64,
}

impl Clock {
    /// `us` microseconds in time base ticks (`av_rescale_q`, nearest, ties away from 0).
    fn ticks(&self, us: i64) -> i64 {
        let (b, c) = (i128::from(self.den), 1_000_000 * i128::from(self.num));
        let a = i128::from(us);
        let ticks = if a < 0 {
            -((-a * b + c / 2) / c)
        } else {
            (a * b + c / 2) / c
        };
        ticks.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    }

    /// Timestamp offset ffmpeg removes from a session seeking to `seek_us`.
    fn origin(&self, seek_us: u64) -> i64 {
        self.ticks((seek_us as i64).saturating_add(self.start_us))
    }

    /// Smallest timestamp an accurate seek to `seek_us` keeps (no seek
    /// and no trimming at 0).
    fn threshold(&self, seek_us: u64) -> i64 {
        if seek_us == 0 {
            i64::MIN
        } else {
            self.origin(seek_us)
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Decoded frames by timestamp, plus the tick ranges known to be complete.
#[derive(Debug)]
struct FrameCache {
    frames: BTreeMap<i64, Arc<RgbImage>>,
    /// Sorted, disjoint, non-adjacent `[lo, hi]` ranges: every frame with a
    /// timestamp in a range is cached, and `hi` is a cached frame.
    covered: Vec<(i64, i64)>,
    /// Timestamp of the video's last frame, once a session reached the end.
    last_frame: Option<i64>,
    bytes: usize,
    budget: usize,
}

impl FrameCache {
    fn new(budget: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            covered: Vec::new(),
            last_frame: None,
            bytes: 0,
            budget,
        }
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.covered.clear();
        self.last_frame = None;
        self.bytes = 0;
    }

    /// The first frame at or after `threshold` (the last frame of the video
    /// when `threshold` lies beyond it), if the cache knows it.
    fn lookup(&self, threshold: i64) -> Option<Arc<RgbImage>> {
        if let Some(&(_, hi)) = self
            .covered
            .iter()
            .find(|(lo, hi)| (*lo..=*hi).contains(&threshold))
        {
            return self
                .frames
                .range(threshold..=hi)
                .next()
                .map(|(_, f)| Arc::clone(f));
        }
        let last = self.last_frame?;
        if threshold > last {
            self.frames.get(&last).cloned()
        } else {
            None
        }
    }

    /// Record that no frame exists after `tick`.
    fn set_last_frame(&mut self, tick: i64) {
        self.last_frame = Some(tick);
    }

    /// Add `frame` at `tick`, known to be the only frame in `[from, tick]`,
    /// then evict the frames farthest from `focus` while over budget.
    fn insert(&mut self, from: i64, tick: i64, frame: Arc<RgbImage>, focus: i64) {
        self.bytes += frame.as_raw().len();
        if let Some(old) = self.frames.insert(tick, frame) {
            self.bytes -= old.as_raw().len();
        }
        self.cover(from, tick);
        self.evict(focus);
    }

    fn cover(&mut self, lo: i64, hi: i64) {
        self.covered.push((lo, hi));
        self.covered.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(self.covered.len());
        for &(lo, hi) in &self.covered {
            match merged.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        self.covered = merged;
    }

    fn evict(&mut self, focus: i64) {
        while self.bytes > self.budget && self.frames.len() > 1 {
            match self.farthest_end(focus) {
                Some((index, tick)) => self.remove_end(index, tick),
                None => {
                    // Frames outside every range cannot be looked up anyway.
                    self.clear();
                    return;
                }
            }
        }
    }

    /// `(range index, tick)` of the range-end frame farthest from `focus`.
    /// Only ends are evicted so that ranges stay complete.
    fn farthest_end(&self, focus: i64) -> Option<(usize, i64)> {
        let mut best: Option<(u64, usize, i64)> = None;
        for (index, &(lo, hi)) in self.covered.iter().enumerate() {
            let Some((&first, _)) = self.frames.range(lo..=hi).next() else {
                continue;
            };
            for tick in [first, hi] {
                let distance = tick.abs_diff(focus);
                if best.is_none_or(|(d, _, _)| distance > d) {
                    best = Some((distance, index, tick));
                }
            }
        }
        best.map(|(_, index, tick)| (index, tick))
    }

    fn remove_end(&mut self, index: usize, tick: i64) {
        if let Some(frame) = self.frames.remove(&tick) {
            self.bytes -= frame.as_raw().len();
        }
        let (lo, hi) = self.covered[index];
        let shrunk = if tick == hi {
            self.frames.range(lo..hi).next_back().map(|(&t, _)| (lo, t))
        } else {
            Some((tick.saturating_add(1), hi))
        };
        match shrunk {
            Some(range) => self.covered[index] = range,
            None => {
                self.covered.remove(index);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ffmpeg session
// ---------------------------------------------------------------------------

/// What the stderr reader reports, in output order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StderrEvent {
    /// Container start time (µs) from the input summary.
    Start(i64),
    /// `showinfo` input time base.
    TimeBase(i64, i64),
    Frame(FrameInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrameInfo {
    index: u64,
    /// `None` for frames without a timestamp.
    pts: Option<i64>,
    width: u32,
    height: u32,
}

/// Stderr lines kept for error messages.
const STDERR_TAIL_LINES: usize = 20;

/// One running ffmpeg child streaming frames from a seek point.
struct Session {
    child: Child,
    stdout: ChildStdout,
    events: Receiver<StderrEvent>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// Frames reported before the time base was known.
    pending: VecDeque<FrameInfo>,
    start_us: i64,
    time_base: Option<(i64, i64)>,
    /// Ticks to add to output timestamps (set once the clock is known).
    origin: Option<i64>,
    /// Every frame from this tick up to the last one read has been read.
    covered_from: i64,
    last_tick: Option<i64>,
    frames_read: u64,
    eof: bool,
}

impl Session {
    fn spawn(path: &Path, seek_us: u64) -> Result<Self, String> {
        let mut cmd = ffmpeg::command(Tool::Ffmpeg).map_err(|e| e.to_string())?;
        // `info` level for the input summary and `showinfo`; `-noautorotate`
        // and `0:V:0` as in the extractor.
        cmd.args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-v",
            "info",
            "-noautorotate",
        ]);
        if seek_us > 0 {
            cmd.arg("-ss").arg(format_secs(seek_us));
        }
        // HDR sources: the extractor's tone-mapping chain, then `showinfo`.
        let filter = match color::display_filter_for(path) {
            Some(tonemap) => format!("{tonemap},showinfo=checksum=0"),
            None => "showinfo=checksum=0".to_owned(),
        };
        cmd.arg("-i")
            .arg(path)
            .args(["-map", "0:V:0", "-vf"])
            .arg(filter)
            .args([
                // One output frame per decoded frame, whatever the frame rate.
                "-fps_mode",
                "passthrough",
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "-",
            ]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start ffmpeg: {e}"))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("ffmpeg pipes are unavailable".into());
        };
        let (sender, events) = mpsc::channel();
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Err(e) = spawn_stderr_reader(stderr, sender, Arc::clone(&stderr_tail)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("could not start the ffmpeg log reader: {e}"));
        }
        Ok(Self {
            child,
            stdout,
            events,
            stderr_tail,
            pending: VecDeque::new(),
            start_us: 0,
            time_base: None,
            origin: None,
            covered_from: i64::MIN,
            last_tick: None,
            frames_read: 0,
            eof: false,
        })
    }

    /// Wait for the time base (reported just before the first frame);
    /// `None` if the session ends without decoding a frame.
    fn clock(&mut self) -> Result<Option<Clock>, String> {
        while self.time_base.is_none() {
            match self.events.recv() {
                Ok(StderrEvent::Start(us)) => self.start_us = us,
                Ok(StderrEvent::TimeBase(num, den)) => self.time_base = Some((num, den)),
                Ok(StderrEvent::Frame(info)) => self.pending.push_back(info),
                Err(_) => {
                    self.finish()?;
                    return Ok(None);
                }
            }
        }
        if !self.pending.is_empty() {
            return Err("frame reported before the time base".into());
        }
        Ok(self.time_base.map(|(num, den)| Clock {
            num,
            den,
            start_us: self.start_us,
        }))
    }

    /// The next frame and its absolute timestamp; `None` at the end.
    fn next_frame(&mut self) -> Result<Option<(i64, RgbImage)>, String> {
        if self.eof {
            return Ok(None);
        }
        let origin = self.origin.ok_or("session clock is unknown")?;
        let info = match self.pending.pop_front() {
            Some(info) => info,
            None => match self.next_info()? {
                Some(info) => info,
                None => return self.finish().map(|()| None),
            },
        };
        if info.index != self.frames_read {
            return Err(format!(
                "frame {} reported while expecting frame {}",
                info.index, self.frames_read
            ));
        }
        let pts = info.pts.ok_or("frame without a timestamp")?;
        let len = info.width as usize * info.height as usize * 3;
        let mut data = vec![0u8; len];
        if !read_frame(&mut self.stdout, &mut data).map_err(|e| format!("reading frame: {e}"))? {
            return self
                .finish()
                .and(Err("ffmpeg stopped before writing a frame".into()));
        }
        let frame = RgbImage::from_raw(info.width, info.height, data)
            .ok_or("frame size does not match its data")?;
        self.frames_read += 1;
        let tick = pts.saturating_add(origin);
        self.last_tick = Some(tick);
        Ok(Some((tick, frame)))
    }

    /// The next frame report, or `None` once ffmpeg has exited.
    fn next_info(&mut self) -> Result<Option<FrameInfo>, String> {
        loop {
            match self.events.recv() {
                Ok(StderrEvent::Frame(info)) => return Ok(Some(info)),
                Ok(StderrEvent::TimeBase(num, den)) if self.time_base != Some((num, den)) => {
                    return Err("time base changed mid-stream".into());
                }
                Ok(_) => {}
                Err(_) => return Ok(None),
            }
        }
    }

    /// ffmpeg's output ended: reap it and report a failed exit.
    fn finish(&mut self) -> Result<(), String> {
        self.eof = true;
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            return Ok(());
        }
        let tail = self
            .stderr_tail
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        Err(format!(
            "ffmpeg failed (exit {}): {}",
            status.code().unwrap_or(-1),
            tail.trim()
        ))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.eof {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Fill `buf` from `reader`: `Ok(false)` if the stream ended before the
/// first byte, an error if it ended part-way.
fn read_frame(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Parse ffmpeg's stderr on a background thread, forwarding frame reports
/// and keeping the other lines' tail for error messages.
fn spawn_stderr_reader(
    stderr: ChildStderr,
    events: Sender<StderrEvent>,
    tail: Arc<Mutex<VecDeque<String>>>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ffmpeg-stderr".into())
        .spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end();
                match parse_line(text) {
                    Some(event) => {
                        if events.send(event).is_err() {
                            break;
                        }
                    }
                    None if !text.starts_with("[Parsed_showinfo") => {
                        if let Ok(mut lines) = tail.lock() {
                            if lines.len() == STDERR_TAIL_LINES {
                                lines.pop_front();
                            }
                            lines.push_back(text.to_owned());
                        }
                    }
                    None => {}
                }
            }
        })
        .map(drop)
}

/// Recognise the input summary's start time and `showinfo` reports.
fn parse_line(line: &str) -> Option<StderrEvent> {
    if line.trim_start().starts_with("Duration:") {
        return parse_start_us(line).map(StderrEvent::Start);
    }
    if !line.starts_with("[Parsed_showinfo") {
        return None;
    }
    let (_, body) = line.split_once("] ")?;
    if let Some(rest) = body.strip_prefix("config in time_base: ") {
        let (num, den) = rest.split(',').next()?.trim().split_once('/')?;
        let (num, den) = (num.parse().ok()?, den.parse().ok()?);
        return (num > 0 && den > 0).then_some(StderrEvent::TimeBase(num, den));
    }
    if !body.starts_with("n:") {
        return None;
    }
    let index = field(body, "n:")?.parse().ok()?;
    let pts = field(body, " pts:")?;
    let (width, height) = field(body, " s:")?.split_once('x')?;
    Some(StderrEvent::Frame(FrameInfo {
        index,
        pts: pts.parse().ok(),
        width: width.parse().ok()?,
        height: height.parse().ok()?,
    }))
}

/// The whitespace-delimited value following the first `key`.
fn field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let at = body.find(key)? + key.len();
    body[at..].split_whitespace().next()
}

/// `start: -1.234567,` in an input summary, in microseconds.
fn parse_start_us(line: &str) -> Option<i64> {
    let (_, rest) = line.split_once("start: ")?;
    let value = rest.split(',').next()?.trim();
    let (negative, value) = match value.strip_prefix('-') {
        Some(v) => (true, v),
        None => (false, value),
    };
    let (secs, frac) = value.split_once('.')?;
    if frac.len() != 6 {
        return None;
    }
    let us = secs.parse::<i64>().ok()? * 1_000_000 + frac.parse::<i64>().ok()?;
    Some(if negative { -us } else { us })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_budget_grows_above_1080p() {
        let video = |width, height| VideoFile {
            path: "v.mp4".into(),
            format: crate::VideoFormat::Mp4,
            duration_ms: 1000,
            width,
            height,
            existing_thumbnail: None,
            is_writable: true,
            frame_step_ms: 40,
            thumbnail_frame_number: None,
            thumbnail_position_ms: None,
        };
        assert_eq!(cache_budget_for(&video(1920, 1080)), DEFAULT_CACHE_BYTES);
        assert_eq!(cache_budget_for(&video(1080, 1920)), DEFAULT_CACHE_BYTES);
        assert_eq!(cache_budget_for(&video(3840, 2160)), LARGE_CACHE_BYTES);
        assert_eq!(cache_budget_for(&video(2560, 1440)), LARGE_CACHE_BYTES);
    }

    fn frame(value: u8) -> Arc<RgbImage> {
        Arc::new(RgbImage::from_pixel(2, 2, image::Rgb([value; 3])))
    }

    fn value(frame: Option<Arc<RgbImage>>) -> Option<u8> {
        frame.map(|f| f.get_pixel(0, 0).0[0])
    }

    const FRAME_BYTES: usize = 12;

    #[test]
    fn rescales_like_av_rescale_q() {
        let clock = Clock {
            num: 1,
            den: 12800,
            start_us: 0,
        };
        assert_eq!(clock.ticks(1_020_000), 13056);
        // 0.5 tick rounds away from zero in both directions.
        let clock = Clock {
            num: 1,
            den: 2,
            start_us: 0,
        };
        assert_eq!(clock.ticks(250_000), 1);
        assert_eq!(clock.ticks(-250_000), -1);
        assert_eq!(clock.ticks(249_999), 0);
        let clock = Clock {
            num: 1,
            den: 90000,
            start_us: 1_400_000,
        };
        assert_eq!(clock.origin(1_000_000), 216_000);
        assert_eq!(clock.threshold(0), i64::MIN);
    }

    #[test]
    fn parses_ffmpeg_lines() {
        assert_eq!(
            parse_line("  Duration: 00:00:05.00, start: 0.000000, bitrate: 40 kb/s"),
            Some(StderrEvent::Start(0))
        );
        assert_eq!(
            parse_line("  Duration: N/A, start: -0.023220, bitrate: N/A"),
            Some(StderrEvent::Start(-23_220))
        );
        assert_eq!(
            parse_line(
                "[Parsed_showinfo_0 @ 0000016f24296200] config in time_base: 1/12800, frame_rate: 25/1"
            ),
            Some(StderrEvent::TimeBase(1, 12800))
        );
        assert_eq!(
            parse_line(
                "[Parsed_showinfo_0 @ 0000016f24296200] n:  12 pts:    256 pts_time:0.02    \
                 duration:    512 duration_time:0.04    fmt:yuv420p cl:left sar:1/1 \
                 s:320x240 i:P iskey:0 type:B "
            ),
            Some(StderrEvent::Frame(FrameInfo {
                index: 12,
                pts: Some(256),
                width: 320,
                height: 240
            }))
        );
        assert_eq!(
            parse_line("[Parsed_showinfo_0 @ 01] n:   0 pts:NOPTS pts_time:NOPTS s:2x2 i:P")
                .and_then(|e| match e {
                    StderrEvent::Frame(info) => Some(info.pts),
                    _ => None,
                }),
            Some(None)
        );
        assert_eq!(
            parse_line("[Parsed_showinfo_0 @ 01] color_range:unknown color_space:unknown"),
            None
        );
        assert_eq!(parse_line("    title           : Duration: x"), None);
        assert_eq!(parse_line("Stream mapping:"), None);
    }

    #[test]
    fn cache_answers_inside_complete_ranges_only() {
        let mut cache = FrameCache::new(usize::MAX);
        // A session from tick 100 read frames 120 and 160.
        cache.insert(100, 120, frame(1), 120);
        cache.insert(121, 160, frame(2), 160);
        assert_eq!(value(cache.lookup(100)), Some(1));
        assert_eq!(value(cache.lookup(120)), Some(1));
        assert_eq!(value(cache.lookup(121)), Some(2));
        assert_eq!(value(cache.lookup(160)), Some(2));
        // Unknown: something may exist before 100 or after 160.
        assert_eq!(value(cache.lookup(99)), None);
        assert_eq!(value(cache.lookup(161)), None);
        // Past the end once the last frame is known.
        cache.set_last_frame(160);
        assert_eq!(value(cache.lookup(10_000)), Some(2));
    }

    #[test]
    fn cache_merges_touching_ranges() {
        let mut cache = FrameCache::new(usize::MAX);
        cache.insert(200, 240, frame(3), 240);
        cache.insert(i64::MIN, 0, frame(1), 0);
        cache.insert(1, 199, frame(2), 199);
        assert_eq!(cache.covered, vec![(i64::MIN, 240)]);
        assert_eq!(value(cache.lookup(-5)), Some(1));
        assert_eq!(value(cache.lookup(150)), Some(2));
        assert_eq!(value(cache.lookup(200)), Some(3));
    }

    #[test]
    fn eviction_drops_far_frames_and_keeps_ranges_honest() {
        let mut cache = FrameCache::new(3 * FRAME_BYTES);
        for (i, tick) in [0i64, 40, 80, 120].into_iter().enumerate() {
            let from = if i == 0 { -20 } else { tick - 39 };
            cache.insert(from, tick, frame(i as u8), 120);
        }
        // Frame 0 is farthest from the focus at 120.
        assert_eq!(cache.bytes, 3 * FRAME_BYTES);
        assert_eq!(cache.covered, vec![(1, 120)]);
        assert_eq!(value(cache.lookup(0)), None);
        assert_eq!(value(cache.lookup(1)), Some(1));
        // Focus moves back to 0: the far end (120) goes.
        cache.insert(-60, -20, frame(9), 0);
        assert_eq!(cache.covered, vec![(-60, -20), (1, 80)]);
        assert_eq!(value(cache.lookup(81)), None);
        assert_eq!(value(cache.lookup(-30)), Some(9));
    }

    #[test]
    fn eviction_of_a_whole_range_removes_it() {
        let mut cache = FrameCache::new(FRAME_BYTES);
        cache.insert(0, 10, frame(1), 10);
        cache.insert(500, 510, frame(2), 510);
        assert_eq!(cache.covered, vec![(500, 510)]);
        assert_eq!(cache.frames.len(), 1);
    }

    #[test]
    fn reads_whole_frames() {
        let mut buf = [0u8; 4];
        let mut r = &b"abcdef"[..];
        assert!(read_frame(&mut r, &mut buf).unwrap());
        assert_eq!(&buf, b"abcd");
        assert!(read_frame(&mut r, &mut buf).is_err());
        let mut empty = &b""[..];
        assert!(!read_frame(&mut empty, &mut buf).unwrap());
    }
}
