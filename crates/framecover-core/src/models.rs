//! Domain model types shared by the core, UI and platform layers.

use std::path::PathBuf;

use image::RgbImage;

/// Supported video container formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoFormat {
    Mp4,
    Mov,
    Mkv,
    Avi,
    Webm,
    Flv,
    Unsupported,
}

impl VideoFormat {
    /// Upper-case display name, as used in user-facing messages (`MP4`, `WEBM`…).
    pub fn name(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Mov => "MOV",
            Self::Mkv => "MKV",
            Self::Avi => "AVI",
            Self::Webm => "WEBM",
            Self::Flv => "FLV",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    /// Whether the container is one of the MP4/MOV family.
    pub fn is_mp4_family(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mov)
    }
}

/// A video file loaded into the application for thumbnail editing.
#[derive(Debug, Clone)]
pub struct VideoFile {
    /// Absolute filesystem path.
    pub path: PathBuf,
    pub format: VideoFormat,
    /// Total duration in milliseconds.
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    /// Currently embedded cover-art image, if any.
    pub existing_thumbnail: Option<RgbImage>,
    pub is_writable: bool,
    /// Approximate duration of one frame in milliseconds (default 40).
    pub frame_step_ms: u64,
    /// Frame number saved with the existing thumbnail.
    pub thumbnail_frame_number: Option<u64>,
    /// Timeline position saved with the existing thumbnail.
    pub thumbnail_position_ms: Option<u64>,
}

impl VideoFile {
    /// True when the file carries a preview previously inserted by FrameCover:
    /// a cover plus either the saved frame position or (for previews made
    /// from an image, which have no position) the preview marker tag, which
    /// is read from the file on disk.
    pub fn has_inserted_preview(&self) -> bool {
        self.existing_thumbnail.is_some()
            && (self.thumbnail_position_ms.is_some()
                || (self.format.is_mp4_family()
                    && crate::mp4_tags::read_preview_source(&self.path).is_some()))
    }
}

/// Machine-readable failure reason for an apply/save operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    FileNotWritable,
    FileLocked,
    UnsupportedFormat,
    NoVideoStream,
    FfmpegError,
    DiskFull,
    DestinationExists,
    Unexpected,
}

/// Outcome of writing a thumbnail. Writers never return `Err`; failures are
/// encoded here so the UI can show them.
#[derive(Debug, Clone, Default)]
pub struct ApplyResult {
    pub success: bool,
    pub error_code: Option<ApplyError>,
    /// Human-readable failure description, or an informational note on success.
    pub error_message: Option<String>,
    pub elapsed_ms: u64,
    /// Milliseconds by which every original frame moved forward because the
    /// concat fast path prepended a frame; 0 when not applicable.
    pub duration_shift_ms: u64,
    /// Final saved copy path (save-copy only).
    pub output_path: Option<PathBuf>,
}

impl ApplyResult {
    pub fn ok() -> Self {
        Self {
            success: true,
            ..Self::default()
        }
    }

    pub fn failure(code: ApplyError, message: impl Into<String>) -> Self {
        Self {
            success: false,
            error_code: Some(code),
            error_message: Some(message.into()),
            ..Self::default()
        }
    }
}

/// How the running version compares with the latest published release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    NewerThanRelease,
}

/// Result of comparing the running version with the latest release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub status: UpdateStatus,
    pub current_version: String,
    /// `None` when nothing has been published yet.
    pub latest_version: Option<String>,
    pub release_url: String,
}

/// Progress callback for multi-stage operations: `(stage name, None)` when a
/// stage starts, `(stage name, Some(seconds))` when it finishes.
pub type ProgressFn<'a> = &'a mut dyn FnMut(&str, Option<f64>);

/// Helper that reports stage start/finish pairs to an optional callback.
pub struct Stages<'a> {
    callback: Option<ProgressFn<'a>>,
    current: Option<(String, std::time::Instant)>,
}

impl<'a> Stages<'a> {
    pub fn new(callback: Option<ProgressFn<'a>>) -> Self {
        Self {
            callback,
            current: None,
        }
    }

    /// Finish the running stage (if any) and start `name`.
    pub fn start(&mut self, name: &str) {
        self.finish();
        self.current = Some((name.to_owned(), std::time::Instant::now()));
        if let Some(cb) = self.callback.as_mut() {
            cb(name, None);
        }
    }

    /// Finish the running stage, reporting its elapsed time.
    pub fn finish(&mut self) {
        if let Some((name, started)) = self.current.take()
            && let Some(cb) = self.callback.as_mut()
        {
            cb(&name, Some(started.elapsed().as_secs_f64()));
        }
    }

    /// Run `f` as a single named stage.
    pub fn stage<T>(&mut self, name: &str, f: impl FnOnce() -> T) -> T {
        self.start(name);
        let out = f();
        self.finish();
        out
    }
}

impl Drop for Stages<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}
