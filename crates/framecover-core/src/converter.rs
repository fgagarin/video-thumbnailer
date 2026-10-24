//! Conversion of other containers to MP4 (port of `core/video_converter.py`).
//!
//! Remux first (`-c copy`) when the source's video codec, profile and pixel
//! format allow a concat-compatible result; if the source cannot qualify, the
//! remux fails, or its result is not eligible for the thumbnail writer's
//! concat fast path, encode once to H.264/AAC trying
//! each available H.264 encoder in turn. HEVC (including HDR10/HLG Main 10)
//! and 10-bit H.264/AV1 qualify for the remux; HEVC is remuxed with the
//! `hvc1` sample entry Apple players require. HDR or high-bit-depth sources
//! that must be encoded try libx265 Main 10 (keeping their colour
//! description) before falling back to 8-bit H.264. Only the first playable video and all
//! audio tracks are kept. The source is never modified and the destination is
//! published only after FFmpeg succeeds (hardlink, so a concurrently created
//! file is never clobbered, unless overwrite was approved).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Output;

use log::{info, warn};

use crate::color::TargetColor;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, Tool};
use crate::probe;
use crate::thumbnail_writer::{self, testing};

/// H.264 encoders tried in order when re-encoding is required.
pub const H264_ENCODERS: [&str; 7] = [
    "libx264",
    "libopenh264",
    "h264_mf",
    "h264_videotoolbox",
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
];

/// Converts a video to MP4 without modifying the source (DI seam for the UI).
pub trait VideoConverter: Send + Sync {
    /// Returns the absolute destination path.
    ///
    /// Errors: [`Error::Conversion`] (FFmpeg/encoder failure),
    /// [`Error::NoVideoStream`], [`Error::InvalidInput`] (destination aliases
    /// the source or is not `.mp4`), [`Error::DestinationExists`], [`Error::Io`].
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        overwrite: bool,
        on_progress: Option<&mut dyn FnMut(&str)>,
    ) -> Result<PathBuf>;
}

/// FFmpeg-backed converter.
#[derive(Debug, Default, Clone)]
pub struct FfmpegVideoConverter;

impl FfmpegVideoConverter {
    /// Inherent forwarding method so callers need not import the trait.
    pub fn convert(
        &self,
        source: &Path,
        destination: &Path,
        overwrite: bool,
        on_progress: Option<&mut dyn FnMut(&str)>,
    ) -> Result<PathBuf> {
        convert_with(&RealBackend, source, destination, overwrite, on_progress)
    }
}

impl VideoConverter for FfmpegVideoConverter {
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        overwrite: bool,
        on_progress: Option<&mut dyn FnMut(&str)>,
    ) -> Result<PathBuf> {
        FfmpegVideoConverter::convert(self, source, destination, overwrite, on_progress)
    }
}

/// Suggested destination for converting `source` (as the UI proposes it):
/// `<stem>.mp4`, or `<stem>-converted.mp4` if that is the source or exists.
pub fn default_conversion_path(source: &Path) -> PathBuf {
    let suggested = source.with_extension("mp4");
    if suggested == source || suggested.exists() {
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        return source.with_file_name(format!("{stem}-converted.mp4"));
    }
    suggested
}

// ---------------------------------------------------------------------------
// Backend seam (the Python tests patch `av.open`, `_run_ffmpeg`,
// `get_ffmpeg_exe` and `concat_ineligibility_reason`)
// ---------------------------------------------------------------------------

pub(crate) trait Backend {
    /// Probe the source once: whether it has a non-attached-picture video
    /// stream and, if so, why a stream-copy remux cannot be concat-compatible.
    fn inspect_source(&self, path: &Path) -> Result<SourceCheck>;
    /// Locate ffmpeg; the error message is shown after "FFmpeg is unavailable: ".
    fn locate_ffmpeg(&self) -> std::result::Result<(), String>;
    /// Run ffmpeg with `args` (excluding the executable).
    fn run(&self, args: Vec<OsString>) -> Result<Output>;
    fn concat_reason(&self, path: &Path) -> Result<Option<String>>;
    /// Whether the converted file has any video stream.
    fn has_any_video(&self, path: &Path) -> Result<bool>;
}

/// Result of [`Backend::inspect_source`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourceCheck {
    pub playable: bool,
    /// `Some(reason)`: skip the remux and encode
    /// ([`thumbnail_writer::remux_ineligibility_reason`]).
    pub remux_blocker: Option<String>,
    /// HEVC video: remuxed with the `hvc1` sample entry (Apple players
    /// reject `hev1`, ffmpeg's MP4 default).
    pub hevc: bool,
    /// HDR or high-bit-depth video: when it must be encoded, libx265 Main 10
    /// with this colour description is tried before the 8-bit H.264
    /// encoders, so HDR is not flattened into SDR.
    pub deep_color: Option<TargetColor>,
}

struct RealBackend;

impl Backend for RealBackend {
    fn inspect_source(&self, path: &Path) -> Result<SourceCheck> {
        let info = probe::probe(path)?;
        let video = info.first_playable_video();
        Ok(SourceCheck {
            playable: video.is_some(),
            remux_blocker: thumbnail_writer::remux_ineligibility_reason(&info),
            hevc: video.is_some_and(|s| s.codec_name.as_deref() == Some("hevc")),
            deep_color: video
                .filter(|s| s.is_hdr() || s.bit_depth().unwrap_or(8) > 8)
                .map(TargetColor::of),
        })
    }
    fn locate_ffmpeg(&self) -> std::result::Result<(), String> {
        match ffmpeg::locate(Tool::Ffmpeg) {
            Ok(_) => Ok(()),
            Err(Error::FfmpegMissing(m)) => Err(m),
            Err(e) => Err(e.to_string()),
        }
    }
    fn run(&self, args: Vec<OsString>) -> Result<Output> {
        testing::run_ffmpeg(args)
    }
    fn concat_reason(&self, path: &Path) -> Result<Option<String>> {
        thumbnail_writer::concat_ineligibility_reason(path)
    }
    fn has_any_video(&self, path: &Path) -> Result<bool> {
        Ok(probe::probe(path)?.streams.iter().any(|s| s.is_video()))
    }
}

/// `_failure_details`: `exit code N` plus `: <stderr>` when there is any.
fn failure_details(code: i32, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("exit code {code}")
    } else {
        format!("exit code {code}: {stderr}")
    }
}

/// Probe failures of the converted file are reported like PyAV's FFmpegError.
fn invalid_output(err: Error) -> Error {
    match err {
        Error::Other(msg) => Error::Conversion(format!("Invalid converted MP4: {msg}")),
        other => other,
    }
}

fn os(s: impl AsRef<OsStr>) -> OsString {
    s.as_ref().to_os_string()
}

fn is_same_file(a: &Path, b: &Path) -> Result<bool> {
    let (ca, cb) = (std::fs::canonicalize(a)?, std::fs::canonicalize(b)?);
    if ca == cb {
        return Ok(true);
    }
    same_file_identity(&ca, &cb)
}

#[cfg(unix)]
fn same_file_identity(a: &Path, b: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let (ma, mb) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    Ok(ma.dev() == mb.dev() && ma.ino() == mb.ino())
}

#[cfg(windows)]
fn same_file_identity(a: &Path, b: &Path) -> Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    fn id(path: &Path) -> Result<(u32, u32, u32)> {
        let file = std::fs::File::open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: valid open handle and a properly sized out-structure.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    }
    Ok(id(a)? == id(b)?)
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(_a: &Path, _b: &Path) -> Result<bool> {
    Ok(false)
}

/// Drop the Windows verbatim prefix `canonicalize` adds (`\\?\C:\…` → `C:\…`,
/// `\\?\UNC\server\…` → `\\server\…`) so paths shown to users and passed to
/// ffmpeg look like Python's `Path.resolve()`.
fn simplify(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return PathBuf::from(rest);
    }
    path
}

fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify)
}

/// `Path.resolve()` (non-strict).
fn resolve(path: &Path) -> PathBuf {
    if let Ok(p) = canonicalize(path) {
        return p;
    }
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => canonicalize(parent).map(|p| p.join(name)).unwrap_or(abs),
        _ => abs,
    }
}

pub(crate) fn convert_with(
    backend: &dyn Backend,
    source: &Path,
    destination: &Path,
    overwrite: bool,
    mut on_progress: Option<&mut dyn FnMut(&str)>,
) -> Result<PathBuf> {
    let source_path = canonicalize(source)?;
    let destination_path = resolve(destination);
    if source_path == destination_path
        || (destination_path.exists() && is_same_file(&source_path, &destination_path)?)
    {
        return Err(Error::InvalidInput(
            "The MP4 destination must not be the source file.".to_owned(),
        ));
    }
    let is_mp4 = destination_path
        .extension()
        .is_some_and(|e| e.to_string_lossy().to_lowercase() == "mp4");
    if !is_mp4 {
        return Err(Error::InvalidInput(
            "The destination must have an .mp4 extension.".to_owned(),
        ));
    }
    if destination_path.exists() && !overwrite {
        return Err(Error::DestinationExists(destination_path));
    }

    let source_check = backend.inspect_source(&source_path)?;
    if !source_check.playable {
        return Err(Error::NoVideoStream(source_path));
    }
    backend
        .locate_ffmpeg()
        .map_err(|m| Error::Conversion(format!("FFmpeg is unavailable: {m}")))?;

    let stem = destination_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = destination_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let temporary = tempfile::Builder::new()
        .prefix(&format!(".{stem}-"))
        .suffix(".mp4")
        .rand_bytes(8)
        .tempfile_in(&dir)?
        .into_temp_path();

    // `temporary` is deleted on drop, success or failure (Python `finally`).
    let common: Vec<OsString> = [
        os("-hide_banner"),
        os("-loglevel"),
        os("error"),
        os("-nostdin"),
        os("-y"),
        os("-i"),
        os(&source_path),
        os("-map"),
        os("0:V:0"),
        os("-map"),
        os("0:a?"),
        os("-map_chapters"),
        os("0"),
    ]
    .into();
    let output: Vec<OsString> = vec![os("-movflags"), os("+faststart"), os(&temporary)];

    let outcome = (|| -> Result<()> {
        let reason = if let Some(blocker) = source_check.remux_blocker.clone() {
            // A stream copy keeps codec, profile and pixel format, so the
            // remux could never be concat-compatible: encode directly.
            info!("MP4 remux skipped, source requires normalization: {blocker}");
            Some(blocker)
        } else {
            let mut remux_output = output.clone();
            if source_check.hevc {
                remux_output.splice(0..0, [os("-tag:v"), os("hvc1")]);
            }
            remux(
                backend,
                &common,
                &remux_output,
                &temporary,
                &mut on_progress,
            )?
        };
        if let Some(reason) = reason {
            transcode(
                backend,
                &common,
                &output,
                &temporary,
                &mut on_progress,
                &reason,
                source_check.deep_color.as_ref(),
            )?;
        }

        if !backend.has_any_video(&temporary).map_err(invalid_output)? {
            return Err(Error::Conversion(
                "FFmpeg produced no playable video.".to_owned(),
            ));
        }
        if overwrite {
            std::fs::rename(&temporary, &destination_path)?;
        } else {
            // Publish atomically without clobbering a concurrently created file.
            match std::fs::hard_link(&temporary, &destination_path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(Error::DestinationExists(destination_path.clone()));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    })();

    let _ = temporary.close();
    match outcome {
        Ok(()) => Ok(destination_path),
        Err(Error::Ffmpeg { code, stderr }) => Err(Error::Conversion(format!(
            "FFmpeg failed (exit {code}): {}",
            failure_details(code, &stderr)
        ))),
        Err(other) => Err(other),
    }
}

/// Stream-copy remux into `temporary`. Returns why the result still needs
/// encoding (remux failure or concat incompatibility), or `None` if it is
/// ready.
fn remux(
    backend: &dyn Backend,
    common: &[OsString],
    output: &[OsString],
    temporary: &Path,
    on_progress: &mut Option<&mut dyn FnMut(&str)>,
) -> Result<Option<String>> {
    if let Some(cb) = on_progress.as_mut() {
        cb("Remuxing to MP4 without re-encoding");
    }
    let mut args = common.to_vec();
    args.extend([os("-c"), os("copy")]);
    args.extend(output.iter().cloned());
    Ok(match backend.run(args) {
        Err(Error::Ffmpeg { code, stderr }) => {
            let reason = failure_details(code, &stderr);
            warn!("MP4 remux failed: {reason}");
            Some(reason)
        }
        Err(other) => return Err(other),
        Ok(_) => {
            let reason = backend.concat_reason(temporary).map_err(invalid_output)?;
            if let Some(r) = &reason {
                info!("MP4 remux requires normalization: {r}");
            }
            reason
        }
    })
}

/// `-encoders` parsing exactly as Python: second field of lines whose first
/// field is 6 characters long.
fn parse_encoders(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() >= 2 && fields[0].chars().count() == 6).then(|| fields[1].to_owned())
        })
        .collect()
}

fn transcode(
    backend: &dyn Backend,
    common: &[OsString],
    output: &[OsString],
    temporary: &Path,
    on_progress: &mut Option<&mut dyn FnMut(&str)>,
    reason: &str,
    deep_color: Option<&TargetColor>,
) -> Result<()> {
    let listing = backend.run(vec![os("-hide_banner"), os("-encoders")])?;
    let available = parse_encoders(&String::from_utf8_lossy(&listing.stdout));
    let has = |name: &str| available.iter().any(|a| a == name);
    if let Some(color) = deep_color
        && has("libx265")
        && has("aac")
    {
        if let Some(cb) = on_progress.as_mut() {
            cb("Encoding MP4 with libx265 (10-bit) / AAC");
        }
        let mut args = common.to_vec();
        args.extend(
            [
                "-c:v",
                "libx265",
                "-preset",
                "medium",
                "-crf",
                "20",
                "-profile:v",
                "main10",
                "-tag:v",
                "hvc1",
                "-x265-params",
                "log-level=error",
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-pix_fmt",
                "yuv420p10le",
            ]
            .map(os),
        );
        args.extend(color.encoder_args().into_iter().map(OsString::from));
        args.extend(["-c:a", "aac", "-b:a", "192k"].map(os));
        args.extend(output.iter().cloned());
        match backend.run(args) {
            Ok(_) => match backend.concat_reason(temporary).map_err(invalid_output)? {
                None => return Ok(()),
                Some(incompatibility) => {
                    warn!("libx265 output incompatible: {incompatibility}")
                }
            },
            Err(Error::Ffmpeg { code, stderr }) => {
                warn!(
                    "libx265 encoding failed: {}",
                    failure_details(code, &stderr)
                )
            }
            Err(other) => return Err(other),
        }
    }
    let candidates: Vec<&str> = H264_ENCODERS
        .iter()
        .copied()
        .filter(|name| has(name))
        .collect();
    if candidates.is_empty() || !available.iter().any(|a| a == "aac") {
        return Err(Error::Conversion(format!(
            "A concat-compatible MP4 requires encoding, but FFmpeg has no usable \
             H.264/AAC encoders. Install an FFmpeg build with libx264 and AAC \
             and select it using IMAGEIO_FFMPEG_EXE.\nConversion reason: {reason}"
        )));
    }
    let mut failures: Vec<String> = Vec::new();
    for encoder in candidates {
        if let Some(cb) = on_progress.as_mut() {
            cb(&format!("Encoding MP4 with {encoder} / AAC"));
        }
        let quality: &[&str] = if encoder == "libx264" {
            &["-preset", "medium", "-crf", "20"]
        } else {
            &["-b:v", "5M"]
        };
        let mut args = common.to_vec();
        args.extend([os("-c:v"), os(encoder)]);
        args.extend(quality.iter().map(os));
        args.extend(
            [
                "-profile:v",
                "high",
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ]
            .map(os),
        );
        args.extend(output.iter().cloned());
        let detail = match backend.run(args) {
            Ok(_) => match backend.concat_reason(temporary).map_err(invalid_output)? {
                None => return Ok(()),
                Some(incompatibility) => {
                    format!("{encoder}: incompatible output: {incompatibility}")
                }
            },
            Err(Error::Ffmpeg { code, stderr }) => {
                format!("{encoder}: {}", failure_details(code, &stderr))
            }
            Err(other) => return Err(other),
        };
        warn!("MP4 encoding failed: {detail}");
        failures.push(detail);
    }
    Err(Error::Conversion(format!(
        "All available H.264 encoders failed:\n{}",
        failures.join("\n")
    )))
}

#[cfg(test)]
mod tests;
