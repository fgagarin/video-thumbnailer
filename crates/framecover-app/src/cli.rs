//! Command-line interface: `framecover <command> ...`.
//!
//! The same executable is the GUI and the CLI. [`is_cli_invocation`] decides
//! from the first argument: a known command name or an option (`--help`,
//! `--version`) selects the CLI; no arguments or plain file paths open the
//! window exactly as before (`framecover video.mp4 [cover.jpg]`).
//!
//! Saving goes through the same job body as the GUI ([`workers::run_apply`]):
//! copy-first via `save_copy` (with its receipts), or in place with the
//! writer, followed by the OS thumbnail-cache refresh.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use framecover_core::converter::default_conversion_path;
use framecover_core::imaging::{self, ImageFit};
use framecover_core::save_copy::default_copy_path;
use framecover_core::update_checker::{GitHubUpdateChecker, UpdateChecker as _};
use framecover_core::{ApplyError, Error, UpdateStatus, VideoFile, VideoFormat};
use image::RgbImage;
use serde_json::json;

use crate::services::Services;
use crate::workers::{SaveJob, run_apply};

/// Command names that select the CLI (plus `help`, provided by clap).
const COMMANDS: &[&str] = &["set", "info", "convert", "extract", "check-update", "help"];

/// Exit code: every file succeeded.
pub const EXIT_OK: i32 = 0;
/// Exit code: at least one file failed (the others were still processed).
pub const EXIT_FAILED: i32 = 1;
/// Exit code: invalid command line.
pub const EXIT_USAGE: i32 = 2;

const AFTER_HELP: &str = "\
Without a command, FrameCover opens its window: `framecover`, `framecover video.mp4`
or `framecover video.mp4 cover.jpg`. To open a file whose name is a command, write
it as a path (`framecover ./info`).

Exit codes: 0 = all files succeeded, 1 = at least one failed (the others are still
processed), 2 = invalid command line.

Windows: framecover.exe is a windowed program, so an interactive cmd.exe or
PowerShell prompt does not wait for it; its output appears after the next prompt.
To wait for it and get the exit code, run `start /wait framecover ...` (cmd) or
pipe it, e.g. `framecover info video.mp4 | Out-Host` (PowerShell). Batch files,
redirected output and scripts wait normally.";

#[derive(Parser, Debug)]
#[command(
    name = "framecover",
    bin_name = "framecover",
    version = framecover_core::VERSION,
    about = "Pick any frame of a video and make it the file's thumbnail",
    after_help = AFTER_HELP,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Set the thumbnail of videos to a frame or an image
    #[command(long_about = SET_ABOUT)]
    Set(SetArgs),
    /// Show format, duration, current thumbnail and how saving would work
    Info(InfoArgs),
    /// Convert a video to MP4 (for containers FrameCover cannot edit)
    #[command(long_about = CONVERT_ABOUT)]
    Convert(ConvertArgs),
    /// Save a frame of a video as a PNG or JPEG image
    Extract(ExtractArgs),
    /// Check GitHub for a newer FrameCover release
    CheckUpdate,
}

const SET_ABOUT: &str = "\
Set the thumbnail of videos to a frame or an image.

By default each video is left unchanged and a copy named <name>-thumbnail.<ext> is
saved next to it (like the app's Save a copy). An existing copy is replaced only
with --overwrite, unless it is a copy FrameCover saved earlier from the same,
unchanged video (verified by the hidden receipt next to it). --in-place modifies
the original instead (the app's Overwrite original): this may damage the file, so
try it on a copy first.

For MP4 and MOV, the chosen frame is inserted as a new first frame (the video and
audio shift by one frame) and attached as cover art; MKV gets a cover.jpg
attachment; AVI, WebM and FLV cannot hold cover art, so only the file manager's
thumbnail cache is refreshed.

Containers FrameCover cannot edit (M2TS, MTS, MPEG-TS...) fail unless --convert is
given: the video is then converted to a separate MP4 (the source is not modified)
and the thumbnail is set on that MP4, which is the output.";

const CONVERT_ABOUT: &str = "\
Convert a video to MP4 for containers FrameCover cannot edit (M2TS, MTS, MPEG-TS...).

When the streams can be stored in MP4 as they are, the first playable video and all
audio tracks are remuxed without re-encoding; otherwise the video is converted once
to H.264/AAC. Subtitles, additional video streams, attachments and data tracks are
not transferred. The source is never modified.";

#[derive(Args, Debug)]
struct SetArgs {
    /// Video files to update
    #[arg(required = true, value_name = "VIDEO")]
    videos: Vec<PathBuf>,

    /// Use the frame at TIME: seconds (90, 12.5), [HH:]MM:SS[.mmm] (1:30,
    /// 01:02:03.250), milliseconds (1500ms) or a percentage of the duration
    /// (33%) [default: 33%]
    #[arg(long, value_name = "TIME", value_parser = parse_time, conflicts_with = "image")]
    at: Option<TimeSpec>,

    /// Use an image file (JPEG, PNG, WebP, BMP) instead of a frame
    #[arg(long, value_name = "FILE")]
    image: Option<PathBuf>,

    /// Fit the image inside the frame; black bars fill the rest (default)
    #[arg(long, requires = "image", conflicts_with = "crop")]
    fit: bool,

    /// Fill the whole frame with the image, cropping the overflow
    #[arg(long, requires = "image")]
    crop: bool,

    /// Save the copy (with --convert: the MP4) to PATH (one VIDEO only)
    #[arg(short, long, value_name = "PATH", conflicts_with_all = ["in_place", "output_dir"])]
    output: Option<PathBuf>,

    /// Save the copies (with --convert: the MP4s) into DIR (created if missing)
    #[arg(long, value_name = "DIR", conflicts_with = "in_place")]
    output_dir: Option<PathBuf>,

    /// Overwrite the original instead of saving a copy
    #[arg(long)]
    in_place: bool,

    /// Replace an existing copy or converted MP4
    #[arg(long)]
    overwrite: bool,

    /// Convert unsupported containers to MP4 first and set the thumbnail on the MP4
    #[arg(long)]
    convert: bool,

    /// Print no progress or warnings (errors and results still appear)
    #[arg(short, long)]
    quiet: bool,

    /// Print one JSON object per video on stdout instead of the output path
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct InfoArgs {
    /// Video files to inspect
    #[arg(required = true, value_name = "VIDEO")]
    videos: Vec<PathBuf>,

    /// Print one JSON object per video
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct ConvertArgs {
    /// Video to convert
    #[arg(value_name = "SOURCE")]
    source: PathBuf,

    /// Where to save the MP4 [default: <name>.mp4 next to the source, or
    /// <name>-converted.mp4 if that exists]
    #[arg(value_name = "DEST.mp4")]
    destination: Option<PathBuf>,

    /// Replace an existing destination
    #[arg(long)]
    overwrite: bool,

    /// Print no progress
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args, Debug)]
struct ExtractArgs {
    /// Video to take the frame from
    #[arg(value_name = "VIDEO")]
    video: PathBuf,

    /// Time of the frame (same forms as `set --at`)
    #[arg(long, value_name = "TIME", value_parser = parse_time)]
    at: TimeSpec,

    /// Image to write; the extension (.png, .jpg, .jpeg) selects the format
    #[arg(short, long, value_name = "IMAGE")]
    output: PathBuf,

    /// Replace an existing image
    #[arg(long)]
    overwrite: bool,
}

// ---------------------------------------------------------------------------
// Time values
// ---------------------------------------------------------------------------

/// A position on the timeline: absolute, or relative to the duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeSpec {
    Millis(u64),
    /// 0.0 to 100.0.
    Percent(f64),
}

/// Default frame position of `set` when neither `--at` nor `--image` is given.
pub const DEFAULT_AT: TimeSpec = TimeSpec::Percent(33.0);

impl TimeSpec {
    /// The position in a video of `duration_ms`.
    pub fn resolve(self, duration_ms: u64) -> Result<u64, String> {
        match self {
            Self::Percent(p) => {
                Ok(((duration_ms as f64 * p / 100.0).round() as u64).min(duration_ms))
            }
            Self::Millis(ms) if ms > duration_ms => Err(format!(
                "{} is past the end of the video ({})",
                format_ms(ms),
                format_ms(duration_ms)
            )),
            Self::Millis(ms) => Ok(ms),
        }
    }
}

/// Parse `90`, `12.5`, `1:30`, `01:02:03.250`, `1500ms`, `2.5s` or `33%`.
pub fn parse_time(text: &str) -> Result<TimeSpec, String> {
    let invalid = || {
        format!(
            "invalid time '{text}': use seconds (90, 12.5), [HH:]MM:SS[.mmm] (1:30, \
             01:02:03.250), milliseconds (1500ms) or a percentage of the duration (33%)"
        )
    };
    let number = |s: &str| -> Result<f64, String> {
        let s = s.trim();
        if s.is_empty() || s.starts_with(['+', '-']) {
            return Err(invalid());
        }
        s.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(invalid)
    };
    let text_t = text.trim();
    if let Some(p) = text_t.strip_suffix('%') {
        let p = number(p)?;
        return if p <= 100.0 {
            Ok(TimeSpec::Percent(p))
        } else {
            Err(format!(
                "invalid time '{text}': a percentage must be 0-100%"
            ))
        };
    }
    let ms = if let Some(ms) = text_t.strip_suffix("ms") {
        number(ms)?
    } else if let Some(s) = text_t.strip_suffix('s') {
        number(s)? * 1000.0
    } else if text_t.contains(':') {
        let parts: Vec<&str> = text_t.split(':').collect();
        let whole = |s: &str| -> Result<f64, String> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            number(s)
        };
        let (h, m, s) = match parts.as_slice() {
            [m, s] => (0.0, whole(m)?, number(s)?),
            [h, m, s] => {
                let m = whole(m)?;
                if m >= 60.0 {
                    return Err(invalid());
                }
                (whole(h)?, m, number(s)?)
            }
            _ => return Err(invalid()),
        };
        if s >= 60.0 {
            return Err(invalid());
        }
        ((h * 60.0 + m) * 60.0 + s) * 1000.0
    } else {
        number(text_t)? * 1000.0
    };
    if ms > u64::MAX as f64 / 2.0 {
        return Err(invalid());
    }
    Ok(TimeSpec::Millis(ms.round() as u64))
}

/// `H:MM:SS.mmm` (or `M:SS.mmm` under an hour).
pub fn format_ms(ms: u64) -> String {
    let (h, rest) = (ms / 3_600_000, ms % 3_600_000);
    let (m, rest) = (rest / 60_000, rest % 60_000);
    let (s, milli) = (rest / 1000, rest % 1000);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{milli:03}")
    } else {
        format!("{m}:{s:02}.{milli:03}")
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "";
    for u in UNITS {
        value /= 1024.0;
        unit = u;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit} ({bytes} bytes)")
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Whether `args` (including the program name) ask for the CLI rather than
/// the window.
pub fn is_cli_invocation(args: &[OsString]) -> bool {
    let Some(first) = args.get(1).and_then(|a| a.to_str()) else {
        return false;
    };
    // macOS used to pass `-psn_<serial>` to apps launched from Finder.
    COMMANDS.contains(&first) || (first.starts_with('-') && !first.starts_with("-psn_"))
}

/// Run the CLI in this process: attach to the parent console on Windows,
/// install the embedded FFmpeg, run, and return the exit code.
pub fn main(args: Vec<OsString>) -> i32 {
    #[cfg(windows)]
    console::attach_parent();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off")).init();
    crate::embedded::install();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = run(args, &mut stdout.lock(), &mut stderr.lock());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    code
}

/// Parse `args` (including the program name) and run the command, writing
/// results to `out` and progress, warnings and errors to `err`.
pub fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => return report_clap_error(&e, out, err),
    };
    let mut io = Io { out, err };
    let services = Services::production();
    match cli.command {
        Command::Set(args) => run_set(&services, &args, &mut io),
        Command::Info(args) => run_info(&services, &args, &mut io),
        Command::Convert(args) => run_convert(&services, &args, &mut io),
        Command::Extract(args) => run_extract(&services, &args, &mut io),
        Command::CheckUpdate => run_check_update(&mut io),
    }
}

fn report_clap_error(e: &clap::Error, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let text = e.render().to_string();
    if e.use_stderr() {
        let _ = write!(err, "{text}");
        EXIT_USAGE
    } else {
        // --help and --version.
        let _ = write!(out, "{text}");
        EXIT_OK
    }
}

struct Io<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

impl Io<'_> {
    fn error(&mut self, subject: &Path, message: &str) {
        let _ = writeln!(self.err, "error: {}: {message}", subject.display());
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn unsupported_hint(err: &Error, source: &Path) -> String {
    format!(
        "{err} Convert it with `framecover convert \"{}\"`, or add --convert.",
        source.display()
    )
}

// ---------------------------------------------------------------------------
// set
// ---------------------------------------------------------------------------

/// Result of `set` for one video (one JSON object with `--json`).
#[derive(Debug, Default)]
struct SetReport {
    output: Option<PathBuf>,
    error: Option<String>,
    note: Option<String>,
    position_ms: Option<u64>,
    duration_shift_ms: u64,
    reencode_reason: Option<String>,
    converted: Option<PathBuf>,
    elapsed_ms: u64,
}

fn run_set(services: &Services, args: &SetArgs, io: &mut Io) -> i32 {
    if args.output.is_some() && args.videos.len() > 1 {
        let e = Cli::command().error(
            ErrorKind::ArgumentConflict,
            "--output takes one VIDEO; use --output-dir for several",
        );
        return report_clap_error(&e, io.out, io.err);
    }
    if let Some(dir) = &args.output_dir
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        io.error(dir, &format!("could not create the directory: {e}"));
        return EXIT_FAILED;
    }
    let image = match &args.image {
        Some(path) => match imaging::load_image(path) {
            Ok(img) => Some(img),
            Err(e) => {
                io.error(path, &e.to_string());
                return EXIT_FAILED;
            }
        },
        None => None,
    };
    let fit = if args.crop {
        ImageFit::Crop
    } else {
        ImageFit::Fit
    };

    let mut failed = 0;
    for input in &args.videos {
        let started = std::time::Instant::now();
        let mut report = SetReport::default();
        if let Err(message) = set_one(services, args, input, image.as_ref(), fit, &mut report, io) {
            report.error = Some(message);
        }
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        if report.error.is_some() {
            failed += 1;
        }
        print_set_report(args, input, &report, io);
    }
    if failed > 0 { EXIT_FAILED } else { EXIT_OK }
}

fn print_set_report(args: &SetArgs, input: &Path, report: &SetReport, io: &mut Io) {
    if args.json {
        let value = json!({
            "input": input.display().to_string(),
            "output": report.output.as_ref().map(|p| p.display().to_string()),
            "success": report.error.is_none(),
            "error": report.error,
            "note": report.note,
            "position_ms": report.position_ms,
            "image": args.image.as_ref().map(|p| p.display().to_string()),
            "duration_shift_ms": report.duration_shift_ms,
            "reencoded": report.reencode_reason.is_some(),
            "reencode_reason": report.reencode_reason,
            "converted": report.converted.is_some(),
            "elapsed_ms": report.elapsed_ms,
        });
        let _ = writeln!(io.out, "{value}");
        if let Some(message) = &report.error {
            io.error(input, message);
        }
        return;
    }
    match (&report.error, &report.output) {
        (Some(message), _) => io.error(input, message),
        (None, Some(output)) => {
            if let Some(note) = &report.note
                && !args.quiet
            {
                let _ = writeln!(io.err, "note: {}: {note}", name_of(input));
            }
            let _ = writeln!(io.out, "{}", output.display());
        }
        (None, None) => {}
    }
}

fn set_one(
    services: &Services,
    args: &SetArgs,
    input: &Path,
    image: Option<&RgbImage>,
    fit: ImageFit,
    report: &mut SetReport,
    io: &mut Io,
) -> Result<(), String> {
    let label = name_of(input);
    let quiet = args.quiet;
    let progress = |io: &mut Io<'_>, stage: &str| stage_line(quiet, &label, io, stage);

    progress(io, "Opening");
    let (video, in_place) = match services.loader.load(input) {
        Ok(video) => (video, args.in_place),
        Err(e @ Error::UnsupportedFormat(_)) if !args.convert => {
            return Err(unsupported_hint(&e, input));
        }
        Err(Error::UnsupportedFormat(_)) => {
            let destination = match (&args.output, &args.output_dir) {
                (Some(output), _) => output.clone(),
                (None, Some(dir)) => {
                    let default = default_conversion_path(input);
                    dir.join(default.file_name().unwrap_or_default())
                }
                (None, None) => default_conversion_path(input),
            };
            let converted = services
                .converter
                .convert(input, &destination, args.overwrite, &mut |line| {
                    progress(io, line)
                })
                .map_err(|e| with_overwrite_hint(&e))?;
            report.converted = Some(converted.clone());
            progress(io, "Opening the MP4");
            // The MP4 is a new file of our own: set its thumbnail in place.
            (
                services
                    .loader
                    .load(&converted)
                    .map_err(|e| e.to_string())?,
                true,
            )
        }
        Err(e) => return Err(e.to_string()),
    };

    let (thumbnail, position_ms) = match image {
        Some(image) => {
            progress(io, "Fitting the image to the frame");
            (
                imaging::compose(image, video.width, video.height, fit),
                None,
            )
        }
        None => {
            let at = args.at.unwrap_or(DEFAULT_AT);
            let position = at.resolve(video.duration_ms)?;
            progress(
                io,
                &format!("Extracting the frame at {}", format_ms(position)),
            );
            let frame = services
                .extractor
                .extract(&video, position)
                .map_err(|e| e.to_string())?;
            (frame, Some(position))
        }
    };
    report.position_ms = position_ms;

    report.reencode_reason = services.writer.reencode_reason(&video);
    if let Some(reason) = &report.reencode_reason
        && !args.quiet
    {
        let _ = writeln!(
            io.err,
            "warning: {label}: saving re-encodes the whole video, which can take time and \
             reduce quality. {reason}"
        );
    }

    let destination = if in_place {
        None
    } else {
        Some(match (&args.output, &args.output_dir) {
            (Some(output), _) => output.clone(),
            (None, Some(dir)) => dir.join(
                default_copy_path(&video.path)
                    .file_name()
                    .unwrap_or_default(),
            ),
            (None, None) => default_copy_path(&video.path),
        })
    };
    let job = SaveJob {
        video: Arc::new(video),
        thumbnail: Arc::new(thumbnail),
        position_ms,
        destination,
        overwrite: args.overwrite,
    };
    let result = run_apply(services, &job, &mut |stage, done| {
        if done.is_none() {
            progress(io, stage);
        }
    });
    report.duration_shift_ms = result.duration_shift_ms;
    if !result.success {
        let mut message = result
            .error_message
            .unwrap_or_else(|| "The thumbnail could not be saved.".to_owned());
        if result.error_code == Some(ApplyError::DestinationExists) {
            message.push_str(" Pass --overwrite to replace it.");
        }
        return Err(message);
    }
    report.note = result.error_message;
    report.output = Some(result.output_path.unwrap_or_else(|| job.video.path.clone()));
    Ok(())
}

fn stage_line(quiet: bool, label: &str, io: &mut Io<'_>, stage: &str) {
    if !quiet {
        let _ = writeln!(io.err, "{label}: {stage}...");
    }
}

fn with_overwrite_hint(e: &Error) -> String {
    match e {
        Error::DestinationExists(_) => format!("{e}. Pass --overwrite to replace it."),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

/// How saving a thumbnail into `video` works: `(json key, description)`.
fn save_mode(video: &VideoFile, reencode_reason: Option<&str>) -> (&'static str, String) {
    match video.format {
        VideoFormat::Mp4 | VideoFormat::Mov => match reencode_reason {
            None => (
                "fast",
                "fast: only the inserted preview frame is encoded".to_owned(),
            ),
            Some(reason) => ("reencode", format!("re-encodes the whole video. {reason}")),
        },
        VideoFormat::Mkv => ("attach", "attaches cover.jpg (no re-encoding)".to_owned()),
        VideoFormat::Avi => (
            "remux",
            "AVI can't hold cover art: re-muxed, only the thumbnail cache is refreshed".to_owned(),
        ),
        VideoFormat::Webm | VideoFormat::Flv => (
            "cache-only",
            format!(
                "{} can't hold cover art: only the thumbnail cache is refreshed",
                video.format.name()
            ),
        ),
        VideoFormat::Unsupported => ("unsupported", "not supported".to_owned()),
    }
}

fn run_info(services: &Services, args: &InfoArgs, io: &mut Io) -> i32 {
    let mut failed = 0;
    for (i, input) in args.videos.iter().enumerate() {
        let video = match services.loader.load(input) {
            Ok(video) => video,
            Err(e) => {
                failed += 1;
                let message = match e {
                    Error::UnsupportedFormat(_) => unsupported_hint(&e, input),
                    other => other.to_string(),
                };
                if args.json {
                    let value = json!({
                        "input": input.display().to_string(),
                        "success": false,
                        "error": message,
                    });
                    let _ = writeln!(io.out, "{value}");
                }
                io.error(input, &message);
                continue;
            }
        };
        let reason = services.writer.reencode_reason(&video);
        let (mode, mode_text) = save_mode(&video, reason.as_deref());
        let file_size = std::fs::metadata(&video.path).map(|m| m.len()).ok();
        let inserted = video.has_inserted_preview();
        if args.json {
            let value = json!({
                "input": input.display().to_string(),
                "path": video.path.display().to_string(),
                "success": true,
                "format": video.format.name(),
                "duration_ms": video.duration_ms,
                "width": video.width,
                "height": video.height,
                "file_size": file_size,
                "frame_step_ms": video.frame_step_ms,
                "writable": video.is_writable,
                "has_cover": video.existing_thumbnail.is_some(),
                "has_inserted_preview": inserted,
                "saved_position_ms": video.thumbnail_position_ms,
                "saved_frame_number": video.thumbnail_frame_number,
                "save_mode": mode,
                "fast_path": mode == "fast",
                "reencode_reason": reason,
            });
            let _ = writeln!(io.out, "{value}");
            continue;
        }
        if i > 0 {
            let _ = writeln!(io.out);
        }
        let yes_no = |b: bool| if b { "yes" } else { "no" };
        let saved = match (video.thumbnail_position_ms, video.thumbnail_frame_number) {
            (Some(ms), Some(frame)) => format!("{} (frame {frame})", format_ms(ms)),
            (Some(ms), None) => format_ms(ms),
            (None, Some(frame)) => format!("frame {frame}"),
            (None, None) if inserted => "none (preview made from an image)".to_owned(),
            (None, None) => "none".to_owned(),
        };
        let lines = [
            ("Format", video.format.name().to_owned()),
            (
                "Duration",
                format!(
                    "{} ({} ms)",
                    format_ms(video.duration_ms),
                    video.duration_ms
                ),
            ),
            ("Frame size", format!("{}x{}", video.width, video.height)),
            (
                "File size",
                file_size.map_or("unknown".to_owned(), format_size),
            ),
            ("Frame step", format!("{} ms", video.frame_step_ms)),
            ("Writable", yes_no(video.is_writable).to_owned()),
            (
                "Cover art",
                yes_no(video.existing_thumbnail.is_some()).to_owned(),
            ),
            ("Inserted preview", yes_no(inserted).to_owned()),
            ("Saved position", saved),
            ("Saving", mode_text),
        ];
        let _ = writeln!(io.out, "{}", video.path.display());
        for (key, value) in lines {
            let _ = writeln!(io.out, "  {:<18}{value}", format!("{key}:"));
        }
    }
    if failed > 0 { EXIT_FAILED } else { EXIT_OK }
}

// ---------------------------------------------------------------------------
// convert, extract, check-update
// ---------------------------------------------------------------------------

fn run_convert(services: &Services, args: &ConvertArgs, io: &mut Io) -> i32 {
    let destination = args
        .destination
        .clone()
        .unwrap_or_else(|| default_conversion_path(&args.source));
    let label = name_of(&args.source);
    let quiet = args.quiet;
    let err = &mut *io.err;
    let result =
        services
            .converter
            .convert(&args.source, &destination, args.overwrite, &mut |line| {
                if !quiet {
                    let _ = writeln!(err, "{label}: {line}...");
                }
            });
    match result {
        Ok(path) => {
            let _ = writeln!(io.out, "{}", path.display());
            EXIT_OK
        }
        Err(e) => {
            io.error(&args.source, &with_overwrite_hint(&e));
            EXIT_FAILED
        }
    }
}

fn run_extract(services: &Services, args: &ExtractArgs, io: &mut Io) -> i32 {
    let ext = args
        .output
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let jpeg = match ext.as_str() {
        "png" => false,
        "jpg" | "jpeg" => true,
        _ => {
            let e = Cli::command().error(
                ErrorKind::InvalidValue,
                "the output image must end in .png, .jpg or .jpeg",
            );
            return report_clap_error(&e, io.out, io.err);
        }
    };
    if args.output.exists() && !args.overwrite {
        io.error(
            &args.output,
            "the file already exists. Pass --overwrite to replace it.",
        );
        return EXIT_FAILED;
    }
    let outcome = (|| -> Result<(), String> {
        let video = services.loader.load(&args.video).map_err(|e| match e {
            Error::UnsupportedFormat(_) => unsupported_hint(&e, &args.video),
            other => other.to_string(),
        })?;
        let position = args.at.resolve(video.duration_ms)?;
        let frame = services
            .extractor
            .extract(&video, position)
            .map_err(|e| e.to_string())?;
        let bytes = if jpeg {
            imaging::encode_jpeg(&frame, imaging::JPEG_QUALITY)
        } else {
            imaging::encode_png(&frame)
        }
        .map_err(|e| e.to_string())?;
        std::fs::write(&args.output, bytes)
            .map_err(|e| format!("could not write '{}': {e}", args.output.display()))
    })();
    match outcome {
        Ok(()) => {
            let _ = writeln!(io.out, "{}", args.output.display());
            EXIT_OK
        }
        Err(message) => {
            io.error(&args.video, &message);
            EXIT_FAILED
        }
    }
}

fn run_check_update(io: &mut Io) -> i32 {
    match GitHubUpdateChecker::new().check(framecover_core::VERSION) {
        Ok(info) => {
            let latest = info.latest_version.as_deref().unwrap_or("none");
            let line = match info.status {
                UpdateStatus::UpToDate => {
                    format!("FrameCover {} is up to date.", info.current_version)
                }
                UpdateStatus::UpdateAvailable => format!(
                    "FrameCover {latest} is available (you have {}): {}",
                    info.current_version, info.release_url
                ),
                UpdateStatus::NewerThanRelease => format!(
                    "FrameCover {} is newer than the latest release ({latest}).",
                    info.current_version
                ),
            };
            let _ = writeln!(io.out, "{line}");
            EXIT_OK
        }
        Err(e) => {
            let _ = writeln!(io.err, "error: {e}");
            EXIT_FAILED
        }
    }
}

// ---------------------------------------------------------------------------
// Windows console
// ---------------------------------------------------------------------------

/// Release builds use the GUI subsystem, so the process starts without a
/// console. Attach to the console of the shell that started us so that
/// output appears there.
#[cfg(windows)]
mod console {
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    fn valid(handle: HANDLE) -> bool {
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    }

    pub fn attach_parent() {
        let ids: [STD_HANDLE; 2] = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        // SAFETY: plain Win32 calls with valid arguments.
        unsafe {
            let had = ids.map(|id| valid(GetStdHandle(id)));
            // A console program (debug build) or fully redirected output.
            if had.iter().all(|&h| h) {
                return;
            }
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            // Keep redirected handles; point the missing ones at the console.
            for (id, had) in ids.into_iter().zip(had) {
                if had || valid(GetStdHandle(id)) {
                    continue;
                }
                let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
                let handle = CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if valid(handle) {
                    SetStdHandle(id, handle);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("framecover").chain(args.iter().copied()))
    }

    fn set_args(args: &[&str]) -> SetArgs {
        match parse(args).unwrap().command {
            Command::Set(set) => set,
            other => panic!("expected set, got {other:?}"),
        }
    }

    fn run_cli(args: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            std::iter::once("framecover").chain(args.iter().copied()),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn parses_times() {
        use TimeSpec::*;
        assert_eq!(parse_time("90"), Ok(Millis(90_000)));
        assert_eq!(parse_time("12.5"), Ok(Millis(12_500)));
        assert_eq!(parse_time("1:30"), Ok(Millis(90_000)));
        assert_eq!(parse_time("01:02:03.250"), Ok(Millis(3_723_250)));
        assert_eq!(parse_time("0:00.040"), Ok(Millis(40)));
        assert_eq!(parse_time("1500ms"), Ok(Millis(1500)));
        assert_eq!(parse_time("2.5s"), Ok(Millis(2500)));
        assert_eq!(parse_time("33%"), Ok(Percent(33.0)));
        assert_eq!(parse_time("12.5%"), Ok(Percent(12.5)));
        assert_eq!(parse_time(" 0 "), Ok(Millis(0)));
        for bad in [
            "", "-1", "+5", "abc", "1:60", "1:61:00", "1:2:3:4", ":30", "1::3", "101%", "nan",
            "inf", "1:-5", "1.5:00", "ms", "%",
        ] {
            assert!(parse_time(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn resolves_times_against_the_duration() {
        assert_eq!(TimeSpec::Percent(33.0).resolve(3000), Ok(990));
        assert_eq!(TimeSpec::Percent(100.0).resolve(3000), Ok(3000));
        assert_eq!(TimeSpec::Millis(3000).resolve(3000), Ok(3000));
        let err = TimeSpec::Millis(3001).resolve(3000).unwrap_err();
        assert!(err.contains("past the end"), "{err}");
        assert_eq!(format_ms(3_723_250), "1:02:03.250");
        assert_eq!(format_ms(90_000), "1:30.000");
    }

    #[test]
    fn detects_cli_invocations() {
        let args = |list: &[&str]| -> Vec<OsString> {
            std::iter::once("framecover")
                .chain(list.iter().copied())
                .map(OsString::from)
                .collect()
        };
        assert!(!is_cli_invocation(&args(&[])));
        assert!(!is_cli_invocation(&args(&["video.mp4"])));
        assert!(!is_cli_invocation(&args(&["video.mp4", "cover.jpg"])));
        assert!(!is_cli_invocation(&args(&["./info"])));
        assert!(!is_cli_invocation(&args(&["-psn_0_12345"])));
        for cli in ["set", "info", "convert", "extract", "check-update", "help"] {
            assert!(is_cli_invocation(&args(&[cli])), "{cli}");
        }
        assert!(is_cli_invocation(&args(&["--help"])));
        assert!(is_cli_invocation(&args(&["-V"])));
        assert!(is_cli_invocation(&args(&["--version"])));
    }

    #[test]
    fn parses_set_arguments() {
        let set = set_args(&[
            "set",
            "a.mp4",
            "b.mov",
            "--at",
            "1:30",
            "--output-dir",
            "out",
        ]);
        assert_eq!(set.videos.len(), 2);
        assert_eq!(set.at, Some(TimeSpec::Millis(90_000)));
        assert_eq!(set.output_dir, Some(PathBuf::from("out")));
        assert!(!set.in_place && !set.overwrite && !set.json && !set.quiet);

        let set = set_args(&["set", "a.mp4", "--image", "c.jpg", "--crop", "--in-place"]);
        assert_eq!(set.image, Some(PathBuf::from("c.jpg")));
        assert!(set.crop && set.in_place);

        let set = set_args(&["set", "a.ts", "-o", "x.mp4", "--convert", "-q", "--json"]);
        assert!(set.convert && set.quiet && set.json);
        assert_eq!(set.at, None);
    }

    #[test]
    fn rejects_conflicting_set_arguments() {
        for bad in [
            &["set"][..],
            &["set", "a.mp4", "--at", "1", "--image", "c.jpg"],
            &["set", "a.mp4", "--crop"],
            &["set", "a.mp4", "--image", "c.jpg", "--fit", "--crop"],
            &["set", "a.mp4", "--output", "x.mp4", "--in-place"],
            &["set", "a.mp4", "--output-dir", "d", "--in-place"],
            &["set", "a.mp4", "--output", "x.mp4", "--output-dir", "d"],
            &["set", "a.mp4", "--at", "soon"],
            &["extract", "a.mp4", "-o", "x.png"],
            &["frobnicate"],
        ] {
            let err = parse(bad).unwrap_err();
            assert_eq!(err.exit_code(), EXIT_USAGE, "{bad:?}");
        }
    }

    #[test]
    fn usage_errors_exit_with_2() {
        let (code, out, err) = run_cli(&["set", "a.mp4", "b.mp4", "-o", "x.mp4"]);
        assert_eq!(code, EXIT_USAGE);
        assert!(out.is_empty());
        assert!(err.contains("--output-dir"), "{err}");

        let (code, _, err) = run_cli(&["extract", "a.mp4", "--at", "1", "-o", "x.gif"]);
        assert_eq!(code, EXIT_USAGE);
        assert!(err.contains(".png"), "{err}");

        let (code, _, err) = run_cli(&["set", "a.mp4", "--at", "later"]);
        assert_eq!(code, EXIT_USAGE);
        assert!(err.contains("1500ms"), "{err}");
    }

    #[test]
    fn help_and_version_go_to_stdout() {
        let (code, out, _) = run_cli(&["--help"]);
        assert_eq!(code, EXIT_OK);
        assert!(
            out.contains("Commands:") && out.contains("Exit codes"),
            "{out}"
        );
        let (code, out, _) = run_cli(&["--version"]);
        assert_eq!(code, EXIT_OK);
        assert_eq!(
            out.trim(),
            format!("framecover {}", framecover_core::VERSION)
        );
        let (code, out, _) = run_cli(&["set", "--help"]);
        assert_eq!(code, EXIT_OK);
        assert!(
            out.contains("--in-place") && out.contains("receipt"),
            "{out}"
        );
    }
}

/// End-to-end runs of the CLI against the real core services and FFmpeg.
#[cfg(test)]
mod production_tests {
    use super::*;
    use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};

    fn generate(path: &Path, extra: &[&str]) {
        let mut args = vec![
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
        ];
        args.extend_from_slice(extra);
        args.push(path.to_str().unwrap());
        framecover_core::ffmpeg::ffmpeg(&args).expect("generate fixture");
    }

    fn mp4(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        generate(
            &path,
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"],
        );
        path
    }

    fn run_cli(args: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            std::iter::once("framecover").chain(args.iter().copied()),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn s(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    fn load(path: &Path) -> VideoFile {
        FfprobeVideoLoader.load(path).unwrap()
    }

    #[test]
    fn set_at_saves_a_copy_and_updates_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let source = mp4(dir.path(), "clip.mp4");
        let before = std::fs::read(&source).unwrap();

        let (code, out, err) = run_cli(&["set", s(&source), "--at", "1.2"]);
        assert_eq!(code, EXIT_OK, "{err}");
        let copy = dir.path().join("clip-thumbnail.mp4");
        assert_eq!(
            PathBuf::from(out.trim()),
            std::path::absolute(&copy).unwrap()
        );
        assert!(
            err.contains("clip.mp4: Extracting the frame at 0:01.200..."),
            "{err}"
        );
        assert!(err.contains("Refreshing thumbnail cache"), "{err}");
        assert_eq!(std::fs::read(&source).unwrap(), before, "source untouched");
        let saved = load(&copy);
        assert!(saved.existing_thumbnail.is_some());
        assert!(saved.has_inserted_preview());
        assert!(saved.thumbnail_position_ms.is_some());

        // The verified copy of the same source can be updated without --overwrite.
        let (code, _, err) = run_cli(&["set", s(&source), "--at", "50%", "-q"]);
        assert_eq!(code, EXIT_OK, "{err}");
        assert!(err.is_empty(), "quiet: {err}");

        // A foreign file at the destination is not replaced without --overwrite.
        let other = dir.path().join("other.mp4");
        std::fs::write(&other, b"not a copy").unwrap();
        let (code, _, err) = run_cli(&["set", s(&source), "-o", s(&other), "-q"]);
        assert_eq!(code, EXIT_FAILED);
        assert!(err.contains("--overwrite"), "{err}");
        assert_eq!(std::fs::read(&other).unwrap(), b"not a copy");
        let (code, _, err) = run_cli(&["set", s(&source), "-o", s(&other), "-q", "--overwrite"]);
        assert_eq!(code, EXIT_OK, "{err}");
        assert!(load(&other).existing_thumbnail.is_some());
    }

    #[test]
    fn set_image_crop_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let source = mp4(dir.path(), "clip.mp4");
        let image = dir.path().join("cover.png");
        let img = RgbImage::from_pixel(100, 30, image::Rgb([200, 30, 30]));
        std::fs::write(&image, imaging::encode_png(&img).unwrap()).unwrap();

        let (code, out, err) = run_cli(&[
            "set",
            s(&source),
            "--image",
            s(&image),
            "--crop",
            "--in-place",
        ]);
        assert_eq!(code, EXIT_OK, "{err}");
        assert_eq!(
            PathBuf::from(out.trim()),
            std::path::absolute(&source).unwrap()
        );
        assert!(!dir.path().join("clip-thumbnail.mp4").exists());
        let saved = load(&source);
        let cover = saved.existing_thumbnail.as_ref().expect("cover art");
        assert!(saved.has_inserted_preview());
        assert_eq!(saved.thumbnail_position_ms, None);
        // Cropped, not letterboxed: the corners are red too.
        let corner = cover.get_pixel(2, 2).0;
        assert!(corner[0] > 150 && corner[1] < 80, "{corner:?}");
    }

    #[test]
    fn batch_continues_after_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let good = mp4(dir.path(), "good.mp4");
        let missing = dir.path().join("missing.mp4");
        let outdir = dir.path().join("out");
        let (code, out, err) = run_cli(&[
            "set",
            s(&missing),
            s(&good),
            "--at",
            "1",
            "--output-dir",
            s(&outdir),
            "--json",
        ]);
        assert_eq!(code, EXIT_FAILED);
        let lines: Vec<serde_json::Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert_eq!(lines[0]["success"], false);
        assert!(lines[0]["error"].as_str().unwrap().contains("not found"));
        assert!(err.contains("missing.mp4"), "{err}");
        assert_eq!(lines[1]["success"], true, "{}", lines[1]);
        assert_eq!(lines[1]["reencoded"], false);
        assert_eq!(lines[1]["position_ms"], 1000);
        assert!(lines[1]["duration_shift_ms"].as_u64().unwrap() > 0);
        let copy = outdir.join("good-thumbnail.mp4");
        assert_eq!(
            PathBuf::from(lines[1]["output"].as_str().unwrap()),
            std::path::absolute(&copy).unwrap()
        );
        assert!(load(&copy).existing_thumbnail.is_some());
    }

    #[test]
    fn info_json_and_text() {
        let dir = tempfile::tempdir().unwrap();
        let source = mp4(dir.path(), "clip.mp4");
        let (code, out, err) = run_cli(&["info", s(&source), "--json"]);
        assert_eq!(code, EXIT_OK, "{err}");
        let info: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(info["format"], "MP4");
        assert_eq!(info["width"], 320);
        assert_eq!(info["height"], 240);
        assert_eq!(info["frame_step_ms"], 40);
        assert_eq!(info["has_cover"], false);
        assert_eq!(info["has_inserted_preview"], false);
        assert_eq!(info["save_mode"], "fast");
        assert_eq!(info["fast_path"], true);
        assert!(info["duration_ms"].as_u64().unwrap() >= 2900);

        let (code, out, _) = run_cli(&["info", s(&source)]);
        assert_eq!(code, EXIT_OK);
        assert!(out.contains("Frame size:       320x240"), "{out}");
        assert!(out.contains("Saving:           fast"), "{out}");
    }

    #[test]
    fn extract_writes_png_and_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let source = mp4(dir.path(), "clip.mp4");
        for name in ["frame.png", "frame.jpg"] {
            let output = dir.path().join(name);
            let (code, out, err) =
                run_cli(&["extract", s(&source), "--at", "50%", "-o", s(&output)]);
            assert_eq!(code, EXIT_OK, "{err}");
            assert_eq!(out.trim(), s(&output));
            let img = imaging::load_image(&output).unwrap();
            assert_eq!(img.dimensions(), (320, 240));
        }
        let output = dir.path().join("frame.png");
        let (code, _, err) = run_cli(&["extract", s(&source), "--at", "1", "-o", s(&output)]);
        assert_eq!(code, EXIT_FAILED);
        assert!(err.contains("--overwrite"), "{err}");
        let (code, _, err) = run_cli(&["extract", s(&source), "--at", "9", "-o", "x.png"]);
        assert_eq!(code, EXIT_FAILED);
        assert!(err.contains("past the end"), "{err}");
    }

    #[test]
    fn convert_and_set_with_convert() {
        let dir = tempfile::tempdir().unwrap();
        let ts = dir.path().join("clip.ts");
        generate(
            &ts,
            &[
                "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-f", "mpegts",
            ],
        );

        let (code, _, err) = run_cli(&["set", s(&ts), "--at", "1"]);
        assert_eq!(code, EXIT_FAILED);
        assert!(
            err.contains("framecover convert") && err.contains("--convert"),
            "{err}"
        );

        let (code, out, err) = run_cli(&["convert", s(&ts)]);
        assert_eq!(code, EXIT_OK, "{err}");
        let mp4 = dir.path().join("clip.mp4");
        assert_eq!(
            std::fs::canonicalize(out.trim()).unwrap(),
            std::fs::canonicalize(&mp4).unwrap()
        );
        assert_eq!(load(&mp4).format, VideoFormat::Mp4);
        let (code, _, err) = run_cli(&["convert", s(&ts), s(&mp4)]);
        assert_eq!(code, EXIT_FAILED);
        assert!(err.contains("--overwrite"), "{err}");

        // clip.mp4 exists now, so --convert writes clip-converted.mp4.
        let (code, out, err) = run_cli(&["set", s(&ts), "--convert", "--at", "1", "--json"]);
        assert_eq!(code, EXIT_OK, "{err}");
        let report: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(report["converted"], true);
        let output = PathBuf::from(report["output"].as_str().unwrap());
        assert_eq!(output.file_name().unwrap(), "clip-converted.mp4");
        let saved = load(&output);
        assert!(saved.existing_thumbnail.is_some() && saved.has_inserted_preview());
        assert!(!dir.path().join("clip-converted-thumbnail.mp4").exists());
    }
}
