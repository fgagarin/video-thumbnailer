//! Frame extraction (port of `core/frame_extractor.py`).
//!
//! Each extraction runs one short-lived `ffmpeg` process with an accurate
//! input seek and streams the decoded frame back as binary PPM (a tiny header
//! followed by raw RGB24), so the decoded size never has to be guessed.
//!
//! Python picks whichever of "last frame before the target" and "first frame
//! at/after the target" is closer (ties go to the earlier frame). Seeking
//! accurately to `target - frame_step / 2` and taking the first frame that
//! survives gives the same answer for constant-rate streams.

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::Stdio;

use image::RgbImage;

use crate::color;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, Tool};
use crate::models::VideoFile;

/// Extracts a single RGB frame nearest to a timeline position.
pub trait FrameExtractor: Send + Sync {
    /// Errors: `InvalidInput` if `offset_ms > video.duration_ms`, `Extraction` otherwise.
    fn extract(&self, video: &VideoFile, offset_ms: u64) -> Result<RgbImage>;
}

/// ffmpeg-process-backed extractor.
#[derive(Debug, Default, Clone)]
pub struct FfmpegFrameExtractor;

/// Look-back windows (ms) used when no frame exists after the seek point,
/// e.g. at `offset == duration`; tried after a window of four frame steps.
const FALLBACK_WINDOWS_MS: [u64; 3] = [1_000, 10_000, 60_000];

impl FrameExtractor for FfmpegFrameExtractor {
    fn extract(&self, video: &VideoFile, offset_ms: u64) -> Result<RgbImage> {
        if offset_ms > video.duration_ms {
            return Err(Error::InvalidInput(format!(
                "position.offset_ms={offset_ms} is outside [0, {}] for '{}'",
                video.duration_ms,
                video.path.display()
            )));
        }
        let fail = |cause: String| Error::Extraction {
            path: video.path.clone(),
            offset_ms,
            cause,
        };

        let step_ms = video.frame_step_ms.max(1);
        let target_us = offset_ms * 1000;
        let seek_us = seek_point_us(video, offset_ms);
        if let Some(frame) = decode(&video.path, seek_us, Some(1)).map_err(fail)? {
            return Ok(frame);
        }

        // Nothing at or after the seek point: the target lies past the last
        // video frame, so the last decodable frame is the nearest one.
        let windows = std::iter::once(step_ms * 4).chain(FALLBACK_WINDOWS_MS);
        for window_ms in windows {
            let start_us = target_us.saturating_sub(window_ms * 1000);
            if let Some(frame) = decode(&video.path, start_us, None).map_err(fail)? {
                return Ok(frame);
            }
            if start_us == 0 {
                break;
            }
        }
        Err(fail("no frame decoded after seek".into()))
    }
}

/// Accurate-seek point for `offset_ms`: half a frame step before it, so the
/// first frame at or after it is the one nearest to the target. Microseconds
/// keep half-step precision for odd frame steps.
pub(crate) fn seek_point_us(video: &VideoFile, offset_ms: u64) -> u64 {
    (offset_ms * 1000).saturating_sub(video.frame_step_ms.max(1) * 500)
}

/// Decode frames of the first playable video stream from `seek_us` onwards
/// and return the last one read (`max_frames` limits how many are decoded).
fn decode(path: &Path, seek_us: u64, max_frames: Option<u32>) -> Result<Option<RgbImage>, String> {
    let mut cmd = ffmpeg::command(Tool::Ffmpeg).map_err(|e| e.to_string())?;
    // `-noautorotate`: return the stored orientation, like PyAV's `to_image`.
    cmd.args(["-hide_banner", "-nostdin", "-v", "error", "-noautorotate"]);
    if seek_us > 0 {
        cmd.arg("-ss").arg(format_secs(seek_us));
    }
    // `V` selects video streams that are not attached pictures.
    cmd.arg("-i").arg(path).args(["-map", "0:V:0"]);
    if let Some(n) = max_frames {
        cmd.arg("-frames:v").arg(n.to_string());
    }
    // HDR sources are tone-mapped to SDR (the same chain as the frame stream).
    if let Some(filter) = color::display_filter_for(path) {
        cmd.args(["-vf", filter]);
    }
    cmd.args(["-c:v", "ppm", "-pix_fmt", "rgb24", "-f", "image2pipe", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().expect("piped stdout"));
    let mut last = None;
    let read_result = loop {
        match read_ppm(&mut stdout) {
            Ok(Some(frame)) => last = Some(frame),
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    drop(stdout);
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr_text = stderr_reader.join().unwrap_or_default();

    if let Err(e) = read_result {
        return Err(format!("invalid frame data from ffmpeg: {e}"));
    }
    if !status.success() && last.is_none() {
        return Err(format!(
            "ffmpeg failed (exit {}): {}",
            status.code().unwrap_or(-1),
            stderr_text.trim()
        ));
    }
    Ok(last)
}

/// `us` microseconds as an ffmpeg `-ss` value (`S.ffffff`).
pub(crate) fn format_secs(us: u64) -> String {
    format!("{}.{:06}", us / 1_000_000, us % 1_000_000)
}

/// Read one binary PPM (`P6`, maxval 255) image; `Ok(None)` at clean EOF.
fn read_ppm(reader: &mut impl Read) -> std::io::Result<Option<RgbImage>> {
    let invalid = |msg: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_owned());
    let mut fields: Vec<u32> = Vec::with_capacity(3);
    let mut token = Vec::new();
    let mut byte = [0u8; 1];
    let mut started = false;
    // Magic number + width, height, maxval, each followed by one whitespace.
    while fields.len() < 3 {
        if reader.read(&mut byte)? == 0 {
            return if started {
                Err(invalid("truncated PPM header"))
            } else {
                Ok(None)
            };
        }
        started = true;
        let b = byte[0];
        if b.is_ascii_whitespace() {
            if token.is_empty() {
                continue;
            }
            if token.starts_with(b"P") {
                if token != b"P6" {
                    return Err(invalid("not a binary PPM"));
                }
            } else {
                let text = std::str::from_utf8(&token).map_err(|_| invalid("bad PPM header"))?;
                fields.push(text.parse().map_err(|_| invalid("bad PPM header"))?);
            }
            token.clear();
        } else if b == b'#' {
            // Comment until end of line.
            while reader.read(&mut byte)? == 1 && byte[0] != b'\n' {}
        } else {
            token.push(b);
            if token.len() > 16 {
                return Err(invalid("bad PPM header"));
            }
        }
    }
    let (w, h, maxval) = (fields[0], fields[1], fields[2]);
    if maxval != 255 || w == 0 || h == 0 {
        return Err(invalid("unsupported PPM"));
    }
    let mut data = vec![0u8; w as usize * h as usize * 3];
    reader.read_exact(&mut data)?;
    Ok(RgbImage::from_raw(w, h, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_concatenated_ppms() {
        let mut bytes = b"P6\n2 1\n255\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        bytes.extend_from_slice(b"P6 # comment\n1 1 255\n");
        bytes.extend_from_slice(&[7, 8, 9]);
        let mut r = &bytes[..];
        let a = read_ppm(&mut r).unwrap().unwrap();
        assert_eq!(a.dimensions(), (2, 1));
        assert_eq!(a.get_pixel(1, 0).0, [4, 5, 6]);
        let b = read_ppm(&mut r).unwrap().unwrap();
        assert_eq!(b.get_pixel(0, 0).0, [7, 8, 9]);
        assert!(read_ppm(&mut r).unwrap().is_none());
    }

    #[test]
    fn rejects_truncated_ppm() {
        let mut r = &b"P6\n2 2\n255\n\x01\x02"[..];
        assert!(read_ppm(&mut r).is_err());
        let mut r = &b"P6\n2"[..];
        assert!(read_ppm(&mut r).is_err());
        let mut r = &b"P5\n1 1\n255\n\x00"[..];
        assert!(read_ppm(&mut r).is_err());
    }

    #[test]
    fn formats_seconds() {
        assert_eq!(format_secs(2_480_000), "2.480000");
        assert_eq!(format_secs(1), "0.000001");
    }
}
