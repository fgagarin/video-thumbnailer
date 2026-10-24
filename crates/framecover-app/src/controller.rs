//! Window-independent application logic (port of `ui/main_window.py`).
//!
//! The [`Controller`] owns a plain [`State`] and reacts to [`Event`]s from
//! the UI and from background workers. Anything with a side effect (file
//! dialogs, worker threads, persisting settings, opening URLs) is returned
//! as an [`Effect`] for the UI glue (`ui.rs`) to execute, so the whole state
//! machine is unit-testable without a window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use framecover_core::imaging::{IMAGE_EXTENSIONS, ImageFit};
use framecover_core::thumbnail_writer::ThumbnailWriter;
use framecover_core::{ApplyError, ApplyResult, UpdateInfo, UpdateStatus, VideoFile};
use image::RgbImage;
use rand::Rng;
use rand::rngs::StdRng;

use crate::error_report::{ErrorReport, Operation, build_operation_error_report};
use crate::timeline::{Timeline, format_position};
use crate::workers::SaveJob;

pub const LOADED_WINDOW_SIZE: (f32, f32) = (1280.0, 720.0);

/// Extensions offered by the "Open video" dialog (same list as the Qt filter).
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "flv", "m2ts", "mts", "ts", "mpeg", "mpg", "wmv",
    "vob", "mxf", "3gp", "ogv",
];

pub const SAVE_BUTTON_TEXT: &str = "Save thumbnail";
pub const SAVING_BUTTON_TEXT: &str = "Saving…";
pub const SAVE_TOOLTIP: &str =
    "Save a copy with the selected frame as its thumbnail; use the arrow to overwrite original";
pub const DROP_TITLE: &str = "Click to open a video file";
pub const DROP_HINT: &str = "or drag and drop a file here";
pub const LOADING_TITLE: &str = "Loading video…";

pub const CONVERT_TITLE: &str = "Convert to MP4?";
pub const CONVERT_TEXT: &str = "This video format is not supported for thumbnail editing.\n\n\
Convert it to a separate MP4 and open it to choose a thumbnail?\n\
The original file will not be changed. FFmpeg first tries lossless \
remuxing, then H.264/AAC encoding if needed for stream-copy preview \
insertion (which can take time and reduce quality).\n\n\
Only the first playable video and all audio tracks are kept; \
subtitles and data tracks are not transferred.";
pub const CONVERTING_TITLE: &str = "Converting to MP4";
pub const CONVERTING_INITIAL_LABEL: &str = "Preparing MP4 conversion";

pub const OVERWRITE_TITLE: &str = "Overwrite original video?";
pub const OVERWRITE_TEXT: &str = "The program may damage the original video file.\n\n\
If you are unsure or have not tested the program on similar files, save a copy instead.";

pub const CONFLICT_TITLE: &str = "Copy already exists";

pub const IMAGE_NEEDS_VIDEO_TITLE: &str = "Open a video first";
pub const IMAGE_NEEDS_VIDEO_TEXT: &str = "A picture can become the thumbnail of a video \
that is open in FrameCover.\n\n\
Open the video first, then drop the picture onto the window or use the Use image… button.";

/// `true` on macOS, where the primary shortcut modifier is Command.
const MAC: bool = cfg!(target_os = "macos");

/// Text of the keyboard-shortcuts help dialog.
pub fn shortcuts_text() -> String {
    let m = if MAC { "Cmd" } else { "Ctrl" };
    format!(
        "{m}+O  Open video\n\
{m}+S  Save preview as thumbnail\n\
{m}+I  Use an image file as the thumbnail\n\
Left / Right  Previous / next frame\n\
Shift+/  Show keyboard shortcuts"
    )
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Whether `path` has one of the still-image extensions FrameCover reads.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// `<stem>-thumbnail<.ext>` next to `source`.
pub fn default_copy_path(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let suffix = source
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    source.with_file_name(format!("{stem}-thumbnail{suffix}"))
}

/// Suggested output of an MP4 conversion: `<stem>.mp4`, or
/// `<stem>-converted.mp4` if that is the source itself or already exists.
pub fn suggest_conversion_path(source: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let suggested = source.with_extension("mp4");
    if suggested == source || exists(&suggested) {
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        source.with_file_name(format!("{stem}-converted.mp4"))
    } else {
        suggested
    }
}

/// Where the timeline starts for a newly opened video: the position saved
/// with an earlier FrameCover thumbnail, else a random point in the middle
/// third of the video.
pub fn initial_position_ms(video: &VideoFile, rng: &mut impl Rng) -> u64 {
    if let Some(ms) = video.thumbnail_position_ms {
        return ms.min(video.duration_ms);
    }
    if let Some(frame) = video.thumbnail_frame_number {
        return frame
            .saturating_mul(video.frame_step_ms)
            .min(video.duration_ms);
    }
    let lower = video.duration_ms / 3;
    let upper = video.duration_ms * 2 / 3;
    rng.random_range(lower..=upper.max(lower))
}

/// Position one frame before/after `current`, clamped to the video;
/// `None` when it would not move.
pub fn step_position(video: &VideoFile, current_ms: u64, direction: i32) -> Option<u64> {
    let step = video.frame_step_ms.max(1);
    let next = if direction < 0 {
        current_ms.saturating_sub(step)
    } else {
        current_ms.saturating_add(step).min(video.duration_ms)
    };
    (next != current_ms).then_some(next)
}

/// The video after an in-place save: the new thumbnail is embedded and, if
/// the concat fast path prepended a frame, everything moved by `shift_ms`.
/// Returns the new video and the new selected-frame position.
pub fn apply_duration_shift(
    video: &VideoFile,
    thumbnail: &RgbImage,
    position_ms: Option<u64>,
    shift_ms: u64,
) -> (VideoFile, Option<u64>) {
    let duration_ms = video.duration_ms + shift_ms;
    let position = position_ms.map(|p| (p + shift_ms).min(duration_ms));
    let step = video.frame_step_ms.max(1);
    let updated = VideoFile {
        duration_ms,
        existing_thumbnail: Some(thumbnail.clone()),
        thumbnail_frame_number: position
            .map(|p| (p as f64 / step as f64).round() as u64)
            .or(video.thumbnail_frame_number),
        thumbnail_position_ms: position,
        ..video.clone()
    };
    (updated, position)
}

// ---------------------------------------------------------------------------
// State pieces
// ---------------------------------------------------------------------------

/// Colour of the footer update dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Idle,
    Latest,
    Update,
    Error,
}

/// Footer update indicator (port of `AppFooter` status logic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateIndicator {
    pub visible: bool,
    pub dot: Dot,
    pub text: String,
    pub tooltip: String,
    pub enabled: bool,
    pub release_url: Option<String>,
}

impl UpdateIndicator {
    fn status(visible: bool, dot: Dot, text: &str, tooltip: &str) -> Self {
        Self {
            visible,
            dot,
            text: text.to_owned(),
            tooltip: format!("{tooltip}\nClick this indicator to check for updates.")
                .trim()
                .to_owned(),
            enabled: true,
            release_url: None,
        }
    }

    pub fn not_checked(visible: bool) -> Self {
        Self::status(visible, Dot::Idle, "Updates not checked yet", "")
    }

    pub fn checking(&mut self) {
        *self = Self::status(self.visible, Dot::Idle, "Checking for updates…", "");
        self.enabled = false;
        self.tooltip = "Checking GitHub for updates. Please wait.".to_owned();
    }

    pub fn set_info(&mut self, info: &UpdateInfo) {
        let latest = info.latest_version.clone().unwrap_or_default();
        *self = match info.status {
            UpdateStatus::UpdateAvailable => {
                let mut s = Self::status(
                    self.visible,
                    Dot::Update,
                    &format!("Version {latest} is available"),
                    "",
                );
                s.release_url = Some(info.release_url.clone());
                s.tooltip = format!(
                    "You have {}. Click to open this release on GitHub and download the new version.",
                    info.current_version
                );
                s
            }
            UpdateStatus::NewerThanRelease => Self::status(
                self.visible,
                Dot::Latest,
                "Test build · newer than the latest release",
                &format!("Latest published release: {latest}."),
            ),
            UpdateStatus::UpToDate => {
                Self::status(self.visible, Dot::Latest, "You have the latest version", "")
            }
        };
    }

    pub fn set_failed(&mut self, message: &str) {
        *self = Self::status(
            self.visible,
            Dot::Error,
            "Couldn't check for updates",
            message,
        );
    }
}

/// Stage log shown in the thumbnail panel while saving.
#[derive(Debug, Clone, Default)]
pub struct SaveProgress {
    pub visible: bool,
    stages: Vec<(String, f64)>,
    active: Option<(String, Instant)>,
}

impl SaveProgress {
    pub fn start(&mut self) {
        *self = Self {
            visible: true,
            ..Self::default()
        };
    }

    pub fn update(&mut self, name: &str, elapsed: Option<f64>, now: Instant) {
        match elapsed {
            None => self.active = Some((name.to_owned(), now)),
            Some(seconds) => {
                self.stages.push((name.to_owned(), seconds));
                self.active = None;
            }
        }
    }

    pub fn finish(&mut self) {
        self.visible = false;
        self.active = None;
    }

    pub fn text(&self, now: Instant) -> String {
        let mut lines: Vec<String> = self
            .stages
            .iter()
            .map(|(name, t)| format!("{name}: {t:.1} s"))
            .collect();
        if let Some((name, started)) = &self.active {
            let t = now.saturating_duration_since(*started).as_secs_f64();
            lines.push(format!("> {name}: {t:.1} s (running)"));
        }
        lines.join("\n")
    }
}

/// Buttons of the overwrite warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteChoice {
    SaveCopy,
    Overwrite,
    Cancel,
}

/// Buttons of the copy-conflict dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    AnotherSuffix,
    Overwrite,
    Cancel,
}

/// A modal dialog. Dialogs stack; the last one is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    ConvertOffer {
        path: PathBuf,
    },
    Converting {
        label: String,
    },
    OverwriteWarning,
    CopyConflict {
        destination: PathBuf,
        details: Option<String>,
    },
    /// Plain message with an OK button (shortcuts, re-encode reason, …).
    Info {
        title: String,
        text: String,
    },
    About,
    Error {
        report: ErrorReport,
        unhandled: bool,
    },
}

impl Dialog {
    /// Text of the copy-conflict dialog body.
    pub fn conflict_text(destination: &Path) -> String {
        format!(
            "'{}' already exists and could not be verified as a saved copy of this video.\n\n\
Overwrite it or add another suffix to save a separate copy?",
            destination.display()
        )
    }
}

/// A still image used as the thumbnail instead of a video frame ("image
/// mode"), already composed into the video's frame size both ways.
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub path: PathBuf,
    pub fit: Arc<RgbImage>,
    pub crop: Arc<RgbImage>,
    pub mode: ImageFit,
}

impl CoverImage {
    /// The image as it will be saved, for the current fit mode.
    pub fn composed(&self) -> &Arc<RgbImage> {
        match self.mode {
            ImageFit::Fit => &self.fit,
            ImageFit::Crop => &self.crop,
        }
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Everything the window shows, as plain data.
#[derive(Debug)]
pub struct State {
    pub video: Option<Arc<VideoFile>>,
    /// Bumped whenever `video` is replaced, to detect stale frame results.
    pub video_generation: u64,
    pub current_frame: Option<Arc<RgbImage>>,
    pub current_frame_position_ms: Option<u64>,
    /// Image and position shown in the "Frame" panel.
    pub candidate: Option<(Arc<RgbImage>, u64)>,
    /// Image shown in the "Thumbnail" panel.
    pub thumbnail: Option<Arc<RgbImage>>,
    /// Set while an image file, not a video frame, is the thumbnail source.
    pub cover: Option<CoverImage>,
    /// Fit mode for the next image (the last one chosen this session).
    pub image_fit: ImageFit,
    /// Folder of the last image opened this session.
    pub image_directory: Option<PathBuf>,
    /// An image to use once the video being loaded is open.
    pub pending_image: Option<PathBuf>,
    pub reencode_reason: Option<String>,
    pub timeline: Timeline,
    pub saving: bool,
    pub converting: bool,
    pub busy: bool,
    pub copy_destination: Option<PathBuf>,
    pub skip_overwrite_warning: bool,
    pub frame_extract_active: bool,
    pub frame_extract_target_ms: Option<u64>,
    pub loading_path: Option<PathBuf>,
    pub last_open_directory: PathBuf,
    pub save_progress: SaveProgress,
    pub last_save_duration: Option<f64>,
    pub save_tooltip: String,
    pub update: UpdateIndicator,
    pub update_check_active: bool,
    pub dialogs: Vec<Dialog>,
}

impl State {
    pub fn editor_visible(&self) -> bool {
        self.video.is_some()
    }

    /// Unlike Qt, the scrubber stays usable while a frame is being decoded
    /// (requests are coalesced), so dragging is never interrupted.
    pub fn timeline_enabled(&self) -> bool {
        self.video.is_some() && !self.saving && !self.converting
    }

    pub fn save_enabled(&self) -> bool {
        !self.busy && !self.saving && self.selected_thumbnail().is_some()
    }

    /// The thumbnail a save would write and the position to record with it:
    /// the composed image (no position) in image mode, else the frame.
    pub fn selected_thumbnail(&self) -> Option<(Arc<RgbImage>, Option<u64>)> {
        match &self.cover {
            Some(cover) => Some((Arc::clone(cover.composed()), None)),
            None => self
                .current_frame
                .as_ref()
                .map(|frame| (Arc::clone(frame), self.current_frame_position_ms)),
        }
    }

    /// Image shown in the "Frame" panel.
    pub fn panel_image(&self) -> Option<&Arc<RgbImage>> {
        match &self.cover {
            Some(cover) => Some(cover.composed()),
            None => self.candidate.as_ref().map(|c| &c.0),
        }
    }

    pub fn save_button_text(&self) -> &'static str {
        if self.saving {
            SAVING_BUTTON_TEXT
        } else {
            SAVE_BUTTON_TEXT
        }
    }

    pub fn drops_accepted(&self) -> bool {
        !self.busy && !self.saving && !self.converting
    }

    pub fn drop_zone_text(&self) -> (String, String) {
        match &self.loading_path {
            Some(path) => (LOADING_TITLE.to_owned(), path.display().to_string()),
            None => (DROP_TITLE.to_owned(), DROP_HINT.to_owned()),
        }
    }

    pub fn frame_title(&self) -> String {
        if let Some(cover) = &self.cover {
            return format!("Image ({})", cover.name());
        }
        match &self.candidate {
            Some((_, ms)) => format!("Frame ({})", format_position(*ms)),
            None => "Frame".to_owned(),
        }
    }

    pub fn saved_label(&self) -> Option<String> {
        self.last_save_duration.map(|s| format!("Saved in {s:.1}s"))
    }

    pub fn top_dialog(&self) -> Option<&Dialog> {
        self.dialogs.last()
    }

    /// The window may close unless a conversion or save is running.
    pub fn can_close(&self) -> bool {
        !self.converting && !self.saving
    }
}

/// Input to the controller.
#[derive(Debug)]
pub enum Event {
    Startup,
    /// Open button, drop-zone click or Ctrl+O.
    OpenFileRequested,
    /// A path from the file dialog, a drop or the command line.
    OpenPath(PathBuf),
    VideoLoaded {
        token: u64,
        video: Box<VideoFile>,
    },
    LoadUnsupported {
        token: u64,
        path: PathBuf,
    },
    LoadFailed {
        token: u64,
        details: String,
    },
    /// Pointer on the scrubber at `ratio` of its width.
    TimelinePointer {
        ratio: f64,
        released: bool,
    },
    StepFrame(i32),
    FilmstripSelected(u64),
    /// "Use image…" button or Ctrl+I.
    OpenImageRequested,
    /// An image file to use as the thumbnail (from its file dialog).
    OpenImage(PathBuf),
    ImageLoaded {
        token: u64,
        path: PathBuf,
        fit: RgbImage,
        crop: RgbImage,
    },
    ImageLoadFailed {
        token: u64,
        details: String,
    },
    SetImageFit(ImageFit),
    /// "Use video frame": leave image mode.
    UseVideoFrame,
    FrameExtracted {
        generation: u64,
        image: RgbImage,
    },
    FrameExtractFailed {
        generation: u64,
        details: String,
    },
    /// Main part of the split button, "Save a copy" or Ctrl+S.
    SaveCopyClicked,
    /// "Overwrite original…" menu entry.
    OverwriteClicked,
    OverwriteWarningAnswered {
        choice: OverwriteChoice,
        suppress: bool,
    },
    CopyConflictAnswered(ConflictChoice),
    SaveProgress {
        stage: String,
        elapsed: Option<f64>,
    },
    SaveFinished(ApplyResult),
    SaveFailed(String),
    ConvertOfferAnswered(bool),
    ConvertDestinationChosen {
        source: PathBuf,
        destination: Option<PathBuf>,
    },
    ConvertProgress(String),
    ConvertFinished(PathBuf),
    ConvertFailed(String),
    UpdateIndicatorClicked,
    UpdateChecked(UpdateInfo),
    UpdateCheckFailed(String),
    ReleaseOpenFailed(String),
    PathClicked,
    ShowShortcuts,
    ShowReencodeReason,
    ShowAbout,
    /// OK/Close of an informational or error dialog.
    DialogDismissed,
    UnhandledError(ErrorReport),
}

/// Side effects for the UI glue to perform.
#[derive(Debug)]
pub enum Effect {
    PickVideoFile {
        start_dir: PathBuf,
    },
    PickImageFile {
        start_dir: PathBuf,
    },
    /// Read an image file and compose it into a `width`x`height` frame.
    LoadImage {
        path: PathBuf,
        width: u32,
        height: u32,
        token: u64,
    },
    LoadVideo {
        path: PathBuf,
        token: u64,
    },
    PersistLastOpenDirectory(PathBuf),
    ExtractFrame {
        video: Arc<VideoFile>,
        position_ms: u64,
        generation: u64,
    },
    SetFilmstripVideo(Option<Arc<VideoFile>>),
    SelectFilmstripPosition(u64),
    Save(SaveJob),
    PickConvertDestination {
        source: PathBuf,
        suggested: PathBuf,
    },
    Convert {
        source: PathBuf,
        destination: PathBuf,
    },
    CheckUpdates,
    OpenReleaseUrl(String),
    RevealInFolder(PathBuf),
    ResizeWindow(f32, f32),
}

pub struct Controller {
    pub state: State,
    writer: Arc<dyn ThumbnailWriter>,
    has_update_checker: bool,
    rng: StdRng,
    load_token: u64,
    image_token: u64,
    fx: Vec<Effect>,
}

impl Controller {
    pub fn new(
        writer: Arc<dyn ThumbnailWriter>,
        last_open_directory: PathBuf,
        has_update_checker: bool,
        rng: StdRng,
    ) -> Self {
        Self {
            state: State {
                video: None,
                video_generation: 0,
                current_frame: None,
                current_frame_position_ms: None,
                candidate: None,
                thumbnail: None,
                cover: None,
                image_fit: ImageFit::default(),
                image_directory: None,
                pending_image: None,
                reencode_reason: None,
                timeline: Timeline::default(),
                saving: false,
                converting: false,
                busy: false,
                copy_destination: None,
                skip_overwrite_warning: false,
                frame_extract_active: false,
                frame_extract_target_ms: None,
                loading_path: None,
                last_open_directory,
                save_progress: SaveProgress::default(),
                last_save_duration: None,
                save_tooltip: SAVE_TOOLTIP.to_owned(),
                update: UpdateIndicator::not_checked(has_update_checker),
                update_check_active: false,
                dialogs: Vec::new(),
            },
            writer,
            has_update_checker,
            rng,
            load_token: 0,
            image_token: 0,
            fx: Vec::new(),
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Startup => self.check_for_updates(),
            Event::OpenFileRequested => self.open_file_dialog(),
            Event::OpenPath(path) => self.load_video(path),
            Event::VideoLoaded { token, video } => self.on_video_loaded(token, *video),
            Event::LoadUnsupported { token, path } => self.offer_conversion(token, path),
            Event::LoadFailed { token, details } => self.on_load_error(token, &details),
            Event::TimelinePointer { ratio, released } => self.on_timeline_pointer(ratio, released),
            Event::StepFrame(direction) => self.step_selected_frame(direction),
            Event::FilmstripSelected(ms) => self.on_filmstrip_position_selected(ms),
            Event::OpenImageRequested => self.open_image_dialog(),
            Event::OpenImage(path) => self.open_image(path),
            Event::ImageLoaded {
                token,
                path,
                fit,
                crop,
            } => self.on_image_loaded(token, path, fit, crop),
            Event::ImageLoadFailed { token, details } => self.on_image_error(token, &details),
            Event::SetImageFit(mode) => {
                if !self.state.saving {
                    self.state.image_fit = mode;
                    if let Some(cover) = &mut self.state.cover {
                        cover.mode = mode;
                    }
                }
            }
            Event::UseVideoFrame => {
                if !self.state.saving {
                    self.state.cover = None;
                }
            }
            Event::FrameExtracted { generation, image } => {
                self.on_frame_extracted(generation, image)
            }
            Event::FrameExtractFailed {
                generation,
                details,
            } => self.on_extract_error(generation, &details),
            Event::SaveCopyClicked => self.on_apply_clicked(),
            Event::OverwriteClicked => self.on_overwrite_clicked(),
            Event::OverwriteWarningAnswered { choice, suppress } => {
                self.on_overwrite_answered(choice, suppress)
            }
            Event::CopyConflictAnswered(choice) => self.on_conflict_answered(choice),
            Event::SaveProgress { stage, elapsed } => {
                self.state
                    .save_progress
                    .update(&stage, elapsed, Instant::now())
            }
            Event::SaveFinished(result) => self.on_apply_done(result),
            Event::SaveFailed(details) => self.on_apply_error(&details),
            Event::ConvertOfferAnswered(yes) => self.on_convert_answered(yes),
            Event::ConvertDestinationChosen {
                source,
                destination,
            } => self.start_conversion(source, destination),
            Event::ConvertProgress(label) => {
                if let Some(Dialog::Converting { label: current }) = self
                    .state
                    .dialogs
                    .iter_mut()
                    .rev()
                    .find(|d| matches!(d, Dialog::Converting { .. }))
                {
                    *current = label;
                }
            }
            Event::ConvertFinished(path) => {
                self.finish_conversion();
                self.load_video(path);
            }
            Event::ConvertFailed(details) => {
                self.finish_conversion();
                self.show_error(build_operation_error_report(
                    Operation::Convert,
                    &details,
                    None,
                    None,
                ));
            }
            Event::UpdateIndicatorClicked => match self.state.update.release_url.clone() {
                Some(url) => self.fx.push(Effect::OpenReleaseUrl(url)),
                None => self.check_for_updates(),
            },
            Event::UpdateChecked(info) => {
                self.state.update_check_active = false;
                self.state.update.set_info(&info);
            }
            Event::UpdateCheckFailed(message) => {
                self.state.update_check_active = false;
                self.state.update.set_failed(&message);
            }
            Event::ReleaseOpenFailed(url) => self.state.dialogs.push(Dialog::Info {
                title: "Couldn't open the release page".to_owned(),
                text: format!(
                    "Open this address in your browser to download the new version:\n{url}"
                ),
            }),
            Event::PathClicked => {
                if let Some(video) = &self.state.video {
                    self.fx.push(Effect::RevealInFolder(video.path.clone()));
                }
            }
            Event::ShowShortcuts => {
                if self.state.dialogs.is_empty() {
                    self.state.dialogs.push(Dialog::Info {
                        title: "Keyboard shortcuts".to_owned(),
                        text: shortcuts_text(),
                    });
                }
            }
            Event::ShowReencodeReason => {
                if let Some(reason) = &self.state.reencode_reason {
                    self.state.dialogs.push(Dialog::Info {
                        title: "Video re-encoding".to_owned(),
                        text: format!(
                            "Saving this thumbnail will re-encode the video, which can take \
longer and change its video codec.\n\nReason: {reason}"
                        ),
                    });
                }
            }
            Event::ShowAbout => {
                if self.state.dialogs.is_empty() {
                    self.state.dialogs.push(Dialog::About);
                }
            }
            Event::DialogDismissed => {
                if !matches!(
                    self.state.top_dialog(),
                    Some(Dialog::Converting { .. }) | None
                ) {
                    self.state.dialogs.pop();
                }
            }
            Event::UnhandledError(report) => {
                let showing = self.state.dialogs.iter().any(|d| {
                    matches!(
                        d,
                        Dialog::Error {
                            unhandled: true,
                            ..
                        }
                    )
                });
                if !showing {
                    self.state.dialogs.push(Dialog::Error {
                        report,
                        unhandled: true,
                    });
                }
            }
        }
        std::mem::take(&mut self.fx)
    }

    fn modal_open(&self) -> bool {
        !self.state.dialogs.is_empty()
    }

    fn show_error(&mut self, report: ErrorReport) {
        self.state.dialogs.push(Dialog::Error {
            report,
            unhandled: false,
        });
    }

    fn pop_dialog(&mut self, matches: impl Fn(&Dialog) -> bool) -> Option<Dialog> {
        if self.state.top_dialog().is_some_and(matches) {
            self.state.dialogs.pop()
        } else {
            None
        }
    }

    fn reencode_reason(&self, video: &VideoFile) -> Option<String> {
        crate::workers::guarded(|| self.writer.reencode_reason(video)).unwrap_or_else(|err| {
            log::error!("reencode_reason failed: {err}");
            None
        })
    }

    // -- loading ------------------------------------------------------------

    fn open_file_dialog(&mut self) {
        if self.state.saving || self.modal_open() {
            return;
        }
        self.fx.push(Effect::PickVideoFile {
            start_dir: self.state.last_open_directory.clone(),
        });
    }

    fn load_video(&mut self, path: PathBuf) {
        if self.state.converting || self.state.saving {
            return;
        }
        // A dropped picture becomes the thumbnail of the open video.
        if is_image_path(&path) {
            self.open_image(path);
            return;
        }
        if let Some(directory) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            self.state.last_open_directory = directory.to_owned();
            self.fx
                .push(Effect::PersistLastOpenDirectory(directory.to_owned()));
        }
        self.state.busy = true;
        self.state.loading_path = Some(path.clone());
        self.load_token += 1;
        self.fx.push(Effect::LoadVideo {
            path,
            token: self.load_token,
        });
    }

    fn on_video_loaded(&mut self, token: u64, video: VideoFile) {
        if token != self.load_token {
            return;
        }
        let s = &mut self.state;
        self.fx.push(Effect::ResizeWindow(
            LOADED_WINDOW_SIZE.0,
            LOADED_WINDOW_SIZE.1,
        ));
        let video = Arc::new(video);
        s.video = Some(Arc::clone(&video));
        s.video_generation += 1;
        s.cover = None;
        self.image_token += 1;
        s.copy_destination = None;
        s.current_frame = None;
        s.current_frame_position_ms = None;
        s.timeline.set_duration(video.duration_ms);
        s.loading_path = None;
        self.fx
            .push(Effect::SetFilmstripVideo(Some(Arc::clone(&video))));
        // Preview reset.
        s.save_progress.finish();
        s.candidate = None;
        s.last_save_duration = None;
        s.thumbnail = video.existing_thumbnail.clone().map(Arc::new);
        let reason = self.reencode_reason(&video);
        let s = &mut self.state;
        s.reencode_reason = reason;
        let initial = initial_position_ms(&video, &mut self.rng);
        s.timeline.set_position(initial);
        s.busy = false;
        self.on_scrub(self.state.timeline.position_ms());
        if let Some(image) = self.state.pending_image.take() {
            self.open_image(image);
        }
    }

    fn offer_conversion(&mut self, token: u64, path: PathBuf) {
        if token != self.load_token {
            return;
        }
        self.state.busy = false;
        self.state.loading_path = None;
        self.state.pending_image = None;
        self.state.dialogs.push(Dialog::ConvertOffer { path });
    }

    fn on_load_error(&mut self, token: u64, details: &str) {
        if token != self.load_token {
            return;
        }
        let s = &mut self.state;
        s.loading_path = None;
        s.video = None;
        s.video_generation += 1;
        s.current_frame = None;
        s.candidate = None;
        s.cover = None;
        s.pending_image = None;
        self.image_token += 1;
        s.busy = false;
        self.fx.push(Effect::SetFilmstripVideo(None));
        self.show_error(build_operation_error_report(
            Operation::Load,
            details,
            None,
            None,
        ));
    }

    // -- image as thumbnail -------------------------------------------------

    fn explain_image_needs_video(&mut self) {
        if self.state.dialogs.is_empty() {
            self.state.dialogs.push(Dialog::Info {
                title: IMAGE_NEEDS_VIDEO_TITLE.to_owned(),
                text: IMAGE_NEEDS_VIDEO_TEXT.to_owned(),
            });
        }
    }

    fn open_image_dialog(&mut self) {
        if self.state.saving || self.state.converting || self.modal_open() {
            return;
        }
        let Some(video) = &self.state.video else {
            self.explain_image_needs_video();
            return;
        };
        let start_dir = self
            .state
            .image_directory
            .clone()
            .or_else(|| video.path.parent().map(Path::to_owned))
            .unwrap_or_else(|| self.state.last_open_directory.clone());
        self.fx.push(Effect::PickImageFile { start_dir });
    }

    fn open_image(&mut self, path: PathBuf) {
        if self.state.converting || self.state.saving {
            return;
        }
        if self.state.loading_path.is_some() {
            // E.g. `framecover clip.mp4 cover.jpg`: use it once the video is open.
            self.state.pending_image = Some(path);
            return;
        }
        let Some(video) = &self.state.video else {
            self.explain_image_needs_video();
            return;
        };
        if let Some(directory) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            self.state.image_directory = Some(directory.to_owned());
        }
        self.image_token += 1;
        self.fx.push(Effect::LoadImage {
            path,
            width: video.width,
            height: video.height,
            token: self.image_token,
        });
    }

    fn on_image_loaded(&mut self, token: u64, path: PathBuf, fit: RgbImage, crop: RgbImage) {
        if token != self.image_token || self.state.saving {
            return;
        }
        self.state.cover = Some(CoverImage {
            path,
            fit: Arc::new(fit),
            crop: Arc::new(crop),
            mode: self.state.image_fit,
        });
    }

    fn on_image_error(&mut self, token: u64, details: &str) {
        if token != self.image_token {
            return;
        }
        self.show_error(build_operation_error_report(
            Operation::Image,
            details,
            None,
            None,
        ));
    }

    // -- conversion ---------------------------------------------------------

    fn on_convert_answered(&mut self, yes: bool) {
        let Some(Dialog::ConvertOffer { path }) =
            self.pop_dialog(|d| matches!(d, Dialog::ConvertOffer { .. }))
        else {
            return;
        };
        if yes {
            let suggested = suggest_conversion_path(&path, Path::exists);
            self.fx.push(Effect::PickConvertDestination {
                source: path,
                suggested,
            });
        }
    }

    fn start_conversion(&mut self, source: PathBuf, destination: Option<PathBuf>) {
        let Some(destination) = destination else {
            return;
        };
        if self.state.converting || self.state.saving {
            return;
        }
        self.state.converting = true;
        self.state.busy = true;
        self.state.dialogs.push(Dialog::Converting {
            label: CONVERTING_INITIAL_LABEL.to_owned(),
        });
        self.fx.push(Effect::Convert {
            source,
            destination,
        });
    }

    fn finish_conversion(&mut self) {
        self.state
            .dialogs
            .retain(|d| !matches!(d, Dialog::Converting { .. }));
        self.state.converting = false;
        self.state.busy = false;
    }

    // -- updates ------------------------------------------------------------

    fn check_for_updates(&mut self) {
        if !self.has_update_checker || self.state.update_check_active {
            return;
        }
        self.state.update_check_active = true;
        self.state.update.checking();
        self.fx.push(Effect::CheckUpdates);
    }

    // -- frame selection ----------------------------------------------------

    fn on_timeline_pointer(&mut self, ratio: f64, _released: bool) {
        if !self.state.timeline_enabled() {
            return;
        }
        let Some(position) = self.state.timeline.position_for_ratio(ratio) else {
            return;
        };
        // Choosing a point on the timeline goes back to using a frame.
        self.state.cover = None;
        self.state.timeline.set_position(position);
        // Follow the pointer while dragging: requests coalesce in `on_scrub`,
        // so the frame updates as fast as frames can be decoded.
        self.on_scrub(position);
    }

    fn on_scrub(&mut self, offset_ms: u64) {
        let s = &mut self.state;
        let Some(video) = &s.video else { return };
        if s.saving {
            return;
        }
        self.fx.push(Effect::SelectFilmstripPosition(offset_ms));
        if s.frame_extract_active {
            // The running request is compared with the timeline when it
            // finishes and re-issued for the latest position if needed.
            return;
        }
        s.frame_extract_active = true;
        s.frame_extract_target_ms = Some(offset_ms);
        s.busy = true;
        self.fx.push(Effect::ExtractFrame {
            video: Arc::clone(video),
            position_ms: offset_ms,
            generation: s.video_generation,
        });
    }

    fn step_selected_frame(&mut self, direction: i32) {
        let s = &mut self.state;
        let Some(video) = &s.video else { return };
        if s.saving || !s.dialogs.is_empty() {
            return;
        }
        s.cover = None;
        // While a frame is being decoded, let a held key queue at most one
        // further step: the display then advances frame by frame and the
        // frame shown when the key is released is (almost) the one selected.
        if s.frame_extract_active && s.frame_extract_target_ms != Some(s.timeline.position_ms()) {
            return;
        }
        if let Some(next) = step_position(video, s.timeline.position_ms(), direction) {
            s.timeline.set_position(next);
            self.on_scrub(next);
        }
    }

    fn on_filmstrip_position_selected(&mut self, offset_ms: u64) {
        if self.state.saving {
            return;
        }
        self.state.cover = None;
        self.state.timeline.set_position(offset_ms);
        self.on_scrub(self.state.timeline.position_ms());
    }

    /// Consume the running request; `true` if the result is for another
    /// video (it is dropped and the current position requested instead).
    fn retry_if_other_video(&mut self, generation: u64) -> bool {
        self.state.frame_extract_active = false;
        if generation != self.state.video_generation {
            self.state.frame_extract_target_ms = None;
            self.on_scrub(self.state.timeline.position_ms());
            return true;
        }
        false
    }

    fn on_frame_extracted(&mut self, generation: u64, image: RgbImage) {
        if self.retry_if_other_video(generation) {
            return;
        }
        let s = &mut self.state;
        let current = s.timeline.position_ms();
        // Show the decoded frame at the position it was requested for, even if
        // the timeline has moved on meanwhile: that is what makes dragging and
        // held arrow keys animate. Then catch up with the latest position.
        let position = s.frame_extract_target_ms.take().unwrap_or(current);
        let image = Arc::new(image);
        s.current_frame = Some(Arc::clone(&image));
        s.current_frame_position_ms = Some(position);
        s.candidate = Some((image, position));
        s.busy = false;
        if position != current {
            self.on_scrub(current);
        }
    }

    fn on_extract_error(&mut self, generation: u64, details: &str) {
        let requested = self.state.frame_extract_target_ms.take();
        if self.retry_if_other_video(generation) {
            return;
        }
        let current = self.state.timeline.position_ms();
        if requested.is_some_and(|r| r != current) {
            // The failed position was already left behind; try the new one.
            self.on_scrub(current);
            return;
        }
        self.state.busy = false;
        let position = self.state.timeline.position_ms();
        self.show_error(build_operation_error_report(
            Operation::Extract,
            details,
            None,
            Some(position),
        ));
    }

    // -- saving -------------------------------------------------------------

    fn can_start_save(&self) -> bool {
        self.state.video.is_some()
            && self.state.selected_thumbnail().is_some()
            && !self.state.saving
    }

    fn on_apply_clicked(&mut self) {
        if !self.can_start_save() {
            return;
        }
        if self.state.copy_destination.is_none() {
            let source = &self.state.video.as_ref().expect("checked").path;
            self.state.copy_destination = Some(default_copy_path(source));
        }
        let destination = self.state.copy_destination.clone();
        self.start_apply(destination, false);
    }

    fn on_overwrite_clicked(&mut self) {
        if !self.can_start_save() {
            return;
        }
        if !self.state.skip_overwrite_warning {
            self.state.dialogs.push(Dialog::OverwriteWarning);
            return;
        }
        self.start_apply(None, false);
    }

    fn on_overwrite_answered(&mut self, choice: OverwriteChoice, suppress: bool) {
        if self
            .pop_dialog(|d| matches!(d, Dialog::OverwriteWarning))
            .is_none()
        {
            return;
        }
        if choice == OverwriteChoice::Cancel {
            return;
        }
        self.state.skip_overwrite_warning = suppress;
        match choice {
            OverwriteChoice::SaveCopy => self.on_apply_clicked(),
            _ => self.start_apply(None, false),
        }
    }

    fn start_apply(&mut self, destination: Option<PathBuf>, overwrite: bool) {
        let s = &mut self.state;
        let (Some(video), Some((thumbnail, position_ms))) = (&s.video, s.selected_thumbnail())
        else {
            return;
        };
        if s.saving || s.frame_extract_active {
            return;
        }
        let job = SaveJob {
            video: Arc::clone(video),
            thumbnail,
            position_ms,
            destination,
            overwrite,
        };
        s.saving = true;
        s.save_progress.start();
        s.last_save_duration = None;
        s.busy = true;
        self.fx.push(Effect::Save(job));
    }

    fn finish_save_state(&mut self) {
        self.state.saving = false;
        self.state.busy = false;
        self.state.save_progress.finish();
    }

    fn on_apply_done(&mut self, result: ApplyResult) {
        self.finish_save_state();
        if result.error_code == Some(ApplyError::DestinationExists) {
            if let Some(destination) = self.state.copy_destination.clone() {
                self.state.dialogs.push(Dialog::CopyConflict {
                    destination,
                    details: result.error_message,
                });
            }
            return;
        }
        if !result.success {
            self.state.last_save_duration = None;
            let details = result
                .error_message
                .as_deref()
                .unwrap_or("No diagnostic details were provided.");
            self.show_error(build_operation_error_report(
                Operation::Save,
                details,
                result.error_code,
                None,
            ));
            return;
        }
        self.state.last_save_duration = Some(result.elapsed_ms as f64 / 1000.0);
        // On success the writer's message is an informational note, e.g. that
        // WebM can't hold cover art; show it with the save result.
        let note = result.error_message.as_deref().filter(|n| !n.is_empty());
        if let Some(output) = &result.output_path {
            // Keep editing the source so repeated saves do not prepend previews.
            self.state.thumbnail = self.state.selected_thumbnail().map(|(image, _)| image);
            self.state.save_tooltip = format!("Saved copy: {}", output.display());
            if let Some(note) = note {
                self.state.save_tooltip.push('\n');
                self.state.save_tooltip.push_str(note);
            }
            return;
        }
        self.state.save_tooltip = note.unwrap_or(SAVE_TOOLTIP).to_owned();
        let (Some(video), Some((thumbnail, saved_position))) =
            (self.state.video.clone(), self.state.selected_thumbnail())
        else {
            return;
        };
        let shift = result.duration_shift_ms;
        let (mut updated, _) = apply_duration_shift(&video, &thumbnail, saved_position, shift);
        if self.state.cover.is_some() {
            // An image preview has no frame position (the writer dropped the
            // old one); the marker tag identifies it as inserted.
            updated.thumbnail_frame_number = None;
        }
        // The frame under the timeline moved with the rest of the video.
        let position = self
            .state
            .current_frame_position_ms
            .map(|p| (p + shift).min(updated.duration_ms));
        let updated = Arc::new(updated);
        let reason = self.reencode_reason(&updated);
        let s = &mut self.state;
        s.video = Some(Arc::clone(&updated));
        s.current_frame_position_ms = position;
        s.thumbnail = Some(thumbnail);
        s.reencode_reason = reason;
        if shift > 0 {
            s.timeline.set_duration(updated.duration_ms);
            s.timeline.set_position(position.unwrap_or(0));
            self.fx.push(Effect::SetFilmstripVideo(Some(updated)));
            self.on_scrub(position.unwrap_or(0));
        }
    }

    fn on_conflict_answered(&mut self, choice: ConflictChoice) {
        if self
            .pop_dialog(|d| matches!(d, Dialog::CopyConflict { .. }))
            .is_none()
        {
            return;
        }
        let Some(destination) = self.state.copy_destination.clone() else {
            return;
        };
        match choice {
            ConflictChoice::Overwrite => self.start_apply(Some(destination), true),
            ConflictChoice::AnotherSuffix => {
                self.state.copy_destination = Some(default_copy_path(&destination));
                self.on_apply_clicked();
            }
            ConflictChoice::Cancel => {}
        }
    }

    fn on_apply_error(&mut self, details: &str) {
        self.finish_save_state();
        self.state.last_save_duration = None;
        self.show_error(build_operation_error_report(
            Operation::Save,
            details,
            None,
            None,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filmstrip::tests::video;
    use crate::services::fakes::FakeWriter;
    use rand::SeedableRng;

    struct Harness {
        c: Controller,
        writer: Arc<FakeWriter>,
        fx: Vec<Effect>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_checker(false)
        }

        fn with_checker(checker: bool) -> Self {
            let writer = Arc::new(FakeWriter::default());
            let c = Controller::new(
                writer.clone(),
                PathBuf::from("/home"),
                checker,
                StdRng::seed_from_u64(7),
            );
            Self {
                c,
                writer,
                fx: Vec::new(),
            }
        }

        fn send(&mut self, event: Event) -> &Vec<Effect> {
            self.fx = self.c.handle(event);
            &self.fx
        }

        fn s(&self) -> &State {
            &self.c.state
        }

        /// Open `/fake/video.mp4` (5 s) and complete the initial extraction.
        fn loaded(&mut self) {
            self.load(video(5000));
            self.complete_extract();
        }

        fn load(&mut self, v: VideoFile) {
            self.send(Event::OpenPath(v.path.clone()));
            let token = self.c.load_token;
            self.send(Event::VideoLoaded {
                token,
                video: Box::new(v),
            });
        }

        fn complete_extract(&mut self) {
            let generation = self.s().video_generation;
            self.send(Event::FrameExtracted {
                generation,
                image: RgbImage::new(32, 32),
            });
        }

        fn extracts(&self) -> Vec<u64> {
            self.fx
                .iter()
                .filter_map(|e| match e {
                    Effect::ExtractFrame { position_ms, .. } => Some(*position_ms),
                    _ => None,
                })
                .collect()
        }

        fn save_job(&self) -> Option<&SaveJob> {
            self.fx.iter().find_map(|e| match e {
                Effect::Save(job) => Some(job),
                _ => None,
            })
        }
    }

    #[test]
    fn initial_state_shows_start_screen() {
        let h = Harness::new();
        assert!(!h.s().editor_visible());
        assert!(!h.s().save_enabled());
        assert!(!h.s().timeline_enabled());
        assert_eq!(
            h.s().drop_zone_text(),
            (DROP_TITLE.into(), DROP_HINT.into())
        );
        assert!(!h.s().update.visible);
    }

    #[test]
    fn open_requests_file_dialog_in_last_directory() {
        let mut h = Harness::new();
        assert!(matches!(&h.send(Event::OpenFileRequested)[..],
            [Effect::PickVideoFile { start_dir }] if start_dir == Path::new("/home")));
    }

    #[test]
    fn load_remembers_directory_and_shows_loading_path() {
        let mut h = Harness::new();
        let fx = h.send(Event::OpenPath(PathBuf::from("/videos/a.mp4")));
        assert!(matches!(&fx[0], Effect::PersistLastOpenDirectory(d) if d == Path::new("/videos")));
        assert!(
            matches!(&fx[1], Effect::LoadVideo { path, .. } if path == Path::new("/videos/a.mp4"))
        );
        assert_eq!(h.s().last_open_directory, PathBuf::from("/videos"));
        assert_eq!(h.s().drop_zone_text().0, LOADING_TITLE);
        assert!(h.s().drop_zone_text().1.contains("a.mp4"));
        assert!(h.s().busy);
    }

    #[test]
    fn video_loaded_updates_ui_and_requests_frame() {
        let mut h = Harness::new();
        h.load(video(5000));
        assert!(h.s().editor_visible());
        assert!(matches!(h.fx[0], Effect::ResizeWindow(1280.0, 720.0)));
        assert!(
            h.fx.iter()
                .any(|e| matches!(e, Effect::SetFilmstripVideo(Some(_))))
        );
        assert_eq!(h.s().drop_zone_text().0, DROP_TITLE);
        assert_eq!(h.s().timeline.duration_ms(), 5000);
        assert_eq!(h.extracts(), [h.s().timeline.position_ms()]);
        assert!(!h.s().save_enabled());
        assert_eq!(h.s().thumbnail, None);
    }

    #[test]
    fn stale_load_results_are_ignored() {
        let mut h = Harness::new();
        h.send(Event::OpenPath("/fake/a.mp4".into()));
        h.send(Event::OpenPath("/fake/b.mp4".into()));
        h.send(Event::VideoLoaded {
            token: 1,
            video: Box::new(video(5000)),
        });
        assert!(!h.s().editor_visible());
    }

    #[test]
    fn restores_saved_thumbnail_position() {
        let mut h = Harness::new();
        h.load(VideoFile {
            thumbnail_position_ms: Some(1234),
            ..video(5000)
        });
        assert_eq!(h.s().timeline.position_ms(), 1234);
    }

    #[test]
    fn restores_from_frame_number_when_no_timestamp() {
        let mut h = Harness::new();
        h.load(VideoFile {
            thumbnail_frame_number: Some(10),
            ..video(5000)
        });
        assert_eq!(h.s().timeline.position_ms(), 400);
    }

    #[test]
    fn randomizes_in_middle_third_when_no_metadata() {
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..200 {
            let p = initial_position_ms(&video(9000), &mut rng);
            assert!((3000..=6000).contains(&p), "{p}");
        }
        assert_eq!(initial_position_ms(&video(0), &mut rng), 0);
        assert_eq!(
            initial_position_ms(
                &VideoFile {
                    thumbnail_position_ms: Some(99_999),
                    ..video(5000)
                },
                &mut rng
            ),
            5000
        );
    }

    #[test]
    fn existing_thumbnail_is_shown() {
        let mut h = Harness::new();
        h.load(VideoFile {
            existing_thumbnail: Some(RgbImage::new(4, 4)),
            ..video(5000)
        });
        assert!(h.s().thumbnail.is_some());
    }

    #[test]
    fn reencode_warning_tracks_loaded_video() {
        let mut h = Harness::new();
        *h.writer.reencode_reason.lock().unwrap() = Some("HEVC".into());
        h.loaded();
        assert_eq!(h.s().reencode_reason.as_deref(), Some("HEVC"));
        h.send(Event::ShowReencodeReason);
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Info { text, .. }) if text.contains("Reason: HEVC"))
        );
    }

    #[test]
    fn load_error_shows_dialog_and_start_screen() {
        let mut h = Harness::new();
        h.loaded();
        h.send(Event::OpenPath("/fake/broken.mp4".into()));
        let token = h.c.load_token;
        h.send(Event::LoadFailed {
            token,
            details: "boom".into(),
        });
        assert!(!h.s().editor_visible());
        assert!(
            h.fx.iter()
                .any(|e| matches!(e, Effect::SetFilmstripVideo(None)))
        );
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Error { report, .. })
            if report.title == "Could not open this video" && report.stack_trace == "boom")
        );
        assert_eq!(h.s().drop_zone_text().0, DROP_TITLE);
    }

    #[test]
    fn frame_extracted_enables_save() {
        let mut h = Harness::new();
        h.loaded();
        let pos = h.s().timeline.position_ms();
        assert!(h.s().save_enabled());
        assert_eq!(h.s().current_frame_position_ms, Some(pos));
        assert_eq!(
            h.s().frame_title(),
            format!("Frame ({})", format_position(pos))
        );
    }

    #[test]
    fn scrub_without_video_does_nothing() {
        let mut h = Harness::new();
        assert!(
            h.send(Event::TimelinePointer {
                ratio: 0.5,
                released: false
            })
            .is_empty()
        );
        assert!(h.send(Event::StepFrame(1)).is_empty());
    }

    #[test]
    fn extract_error_shows_frame_dialog() {
        let mut h = Harness::new();
        h.load(video(5000));
        let generation = h.s().video_generation;
        h.send(Event::FrameExtractFailed {
            generation,
            details: "decode fail".into(),
        });
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Error { report, .. })
            if report.title.starts_with("Could not read this frame at"))
        );
        assert!(!h.s().busy);
    }

    #[test]
    fn stale_results_retry_current_position_without_dialog() {
        let mut h = Harness::new();
        h.load(video(5000));
        // Move while the first extraction is running.
        h.send(Event::FilmstripSelected(3000));
        assert!(h.extracts().is_empty(), "coalesced while busy");
        let generation = h.s().video_generation;
        h.send(Event::FrameExtractFailed {
            generation,
            details: "x".into(),
        });
        assert!(h.s().dialogs.is_empty());
        assert_eq!(h.extracts(), [3000]);
        h.send(Event::FilmstripSelected(2000));
        h.send(Event::FrameExtracted {
            generation,
            image: RgbImage::new(1, 1),
        });
        // The late frame is shown at the position it was decoded for, and the
        // latest position is requested next.
        assert_eq!(h.s().current_frame_position_ms, Some(3000));
        assert_eq!(
            h.s().frame_title(),
            format!("Frame ({})", format_position(3000))
        );
        assert_eq!(h.extracts(), [2000]);
        assert!(!h.s().save_enabled(), "saving waits for the latest frame");
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(2000));
        assert!(h.extracts().is_empty());
        assert!(h.s().save_enabled());
    }

    #[test]
    fn result_for_previous_video_is_dropped() {
        let mut h = Harness::new();
        h.load(video(5000));
        let old = h.s().video_generation;
        h.load(VideoFile {
            path: "/fake/other.mp4".into(),
            ..video(8000)
        });
        h.send(Event::FrameExtracted {
            generation: old,
            image: RgbImage::new(1, 1),
        });
        assert!(h.s().current_frame.is_none());
        assert_eq!(h.extracts(), [h.s().timeline.position_ms()]);
    }

    #[test]
    fn step_moves_by_one_frame_and_clamps() {
        let mut h = Harness::new();
        h.loaded();
        h.send(Event::FilmstripSelected(1000));
        h.complete_extract();
        h.send(Event::StepFrame(1));
        assert_eq!(h.s().timeline.position_ms(), 1040);
        assert_eq!(h.extracts(), [1040]);
        // A held key queues at most one step while a frame is decoding, so
        // every frame is shown in turn.
        for _ in 0..4 {
            h.send(Event::StepFrame(1));
        }
        assert_eq!(h.s().timeline.position_ms(), 1080);
        assert_eq!(h.s().frame_extract_target_ms, Some(1040));
        assert!(h.extracts().is_empty());
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(1040));
        assert_eq!(h.extracts(), [1080], "the queued step is requested next");
        h.send(Event::StepFrame(1));
        assert_eq!(h.s().timeline.position_ms(), 1120);
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(1080));
        assert_eq!(h.extracts(), [1120]);
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(1120));
        assert!(h.extracts().is_empty());

        let v = video(5000);
        assert_eq!(step_position(&v, 0, -1), None);
        assert_eq!(step_position(&v, 20, -1), Some(0));
        assert_eq!(step_position(&v, 5000, 1), None);
        assert_eq!(step_position(&v, 4990, 1), Some(5000));
    }

    #[test]
    fn timeline_drag_updates_frame_continuously() {
        let mut h = Harness::new();
        h.loaded();
        h.send(Event::TimelinePointer {
            ratio: 0.5,
            released: false,
        });
        assert_eq!(h.s().timeline.position_ms(), 2500);
        assert_eq!(h.extracts(), [2500], "extracts while dragging");
        // Further movement coalesces while that frame decodes...
        for ratio in [0.52, 0.54, 0.56] {
            h.send(Event::TimelinePointer {
                ratio,
                released: false,
            });
        }
        assert!(h.extracts().is_empty());
        // ...then the decoded frame is shown and the latest position follows.
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(2500));
        assert_eq!(h.extracts(), [2800]);
        h.send(Event::TimelinePointer {
            ratio: 0.6,
            released: true,
        });
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(2800));
        assert_eq!(h.extracts(), [3000]);
        h.complete_extract();
        assert_eq!(h.s().current_frame_position_ms, Some(3000));
        assert!(h.extracts().is_empty());
    }

    #[test]
    fn filmstrip_selection_moves_timeline_and_extracts() {
        let mut h = Harness::new();
        h.loaded();
        h.send(Event::FilmstripSelected(4000));
        assert_eq!(h.s().timeline.position_ms(), 4000);
        assert_eq!(h.extracts(), [4000]);
        assert!(
            h.fx.iter()
                .any(|e| matches!(e, Effect::SelectFilmstripPosition(4000)))
        );
    }

    // -- saving --------------------------------------------------------------

    fn selected(h: &mut Harness) {
        h.loaded();
        h.send(Event::FilmstripSelected(1000));
        h.complete_extract();
    }

    #[test]
    fn save_copy_uses_default_name_and_rejects_repeated_start() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::SaveCopyClicked);
        let job = h.save_job().unwrap();
        assert_eq!(
            job.destination.as_deref(),
            Some(Path::new("/fake/video-thumbnail.mp4"))
        );
        assert!(!job.overwrite);
        assert_eq!(job.position_ms, Some(1000));
        assert_eq!(h.s().save_button_text(), SAVING_BUTTON_TEXT);
        assert!(!h.s().save_enabled());
        assert!(h.s().save_progress.visible);
        assert!(h.send(Event::SaveCopyClicked).is_empty());
        assert!(h.send(Event::OverwriteClicked).is_empty());
        assert!(h.send(Event::OpenPath("/fake/other.mp4".into())).is_empty());
        assert!(h.send(Event::FilmstripSelected(4000)).is_empty());
        assert_eq!(h.s().timeline.position_ms(), 1000);
        assert!(!h.s().can_close());
    }

    #[test]
    fn save_needs_video_and_frame() {
        let mut h = Harness::new();
        assert!(h.send(Event::SaveCopyClicked).is_empty());
        h.load(video(5000));
        assert!(h.send(Event::SaveCopyClicked).is_empty());
    }

    #[test]
    fn save_during_extraction_does_not_use_stale_frame() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::FilmstripSelected(2000));
        h.send(Event::SaveCopyClicked);
        assert!(h.save_job().is_none());
    }

    #[test]
    fn save_progress_is_shown_and_cleared() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::SaveCopyClicked);
        h.send(Event::SaveProgress {
            stage: "Encoding video".into(),
            elapsed: None,
        });
        let text = h.s().save_progress.text(Instant::now());
        assert!(
            text.starts_with("> Encoding video: ") && text.ends_with("(running)"),
            "{text}"
        );
        h.send(Event::SaveProgress {
            stage: "Encoding video".into(),
            elapsed: Some(2.0),
        });
        assert_eq!(
            h.s().save_progress.text(Instant::now()),
            "Encoding video: 2.0 s"
        );
        h.send(Event::SaveFinished(ApplyResult {
            output_path: Some("/fake/video-thumbnail.mp4".into()),
            elapsed_ms: 2345,
            ..ApplyResult::ok()
        }));
        assert!(!h.s().save_progress.visible);
        assert_eq!(h.s().save_button_text(), SAVE_BUTTON_TEXT);
        assert!(h.s().save_enabled());
        assert_eq!(h.s().saved_label().as_deref(), Some("Saved in 2.3s"));
    }

    #[test]
    fn successful_copy_preserves_source_timeline_and_metadata() {
        let mut h = Harness::new();
        selected(&mut h);
        let source = h.s().video.clone().unwrap();
        h.send(Event::SaveCopyClicked);
        h.send(Event::SaveFinished(ApplyResult {
            output_path: Some("/fake/video-thumbnail.mp4".into()),
            duration_shift_ms: 40,
            ..ApplyResult::ok()
        }));
        let video = h.s().video.clone().unwrap();
        assert!(Arc::ptr_eq(&video, &source));
        assert!(video.existing_thumbnail.is_none());
        assert_eq!(video.duration_ms, 5000);
        assert_eq!(h.s().timeline.position_ms(), 1000);
        assert_eq!(h.s().current_frame_position_ms, Some(1000));
        assert!(h.s().save_tooltip.contains("video-thumbnail.mp4"));
        assert!(h.s().thumbnail.is_some());
        assert!(h.s().dialogs.is_empty(), "success is silent");
    }

    #[test]
    fn success_note_is_shown_in_the_save_tooltip() {
        let note = "WebM files can't hold cover art.";
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::SaveCopyClicked);
        h.send(Event::SaveFinished(ApplyResult {
            output_path: Some("/fake/video-thumbnail.webm".into()),
            error_message: Some(note.into()),
            ..ApplyResult::ok()
        }));
        assert!(h.s().save_tooltip.ends_with(note), "{}", h.s().save_tooltip);
        assert!(h.s().dialogs.is_empty());

        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::OverwriteClicked);
        h.send(Event::OverwriteWarningAnswered {
            choice: OverwriteChoice::Overwrite,
            suppress: false,
        });
        h.send(Event::SaveFinished(ApplyResult {
            error_message: Some(note.into()),
            ..ApplyResult::ok()
        }));
        assert_eq!(h.s().save_tooltip, note);
    }

    #[test]
    fn in_place_save_applies_duration_shift_and_reloads() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::OverwriteClicked);
        h.send(Event::OverwriteWarningAnswered {
            choice: OverwriteChoice::Overwrite,
            suppress: false,
        });
        assert!(h.save_job().unwrap().destination.is_none());
        *h.writer.reencode_reason.lock().unwrap() = Some("attached picture".into());
        h.send(Event::SaveFinished(ApplyResult {
            duration_shift_ms: 40,
            ..ApplyResult::ok()
        }));
        let video = h.s().video.clone().unwrap();
        assert_eq!(video.duration_ms, 5040);
        assert_eq!(video.thumbnail_position_ms, Some(1040));
        assert_eq!(video.thumbnail_frame_number, Some(26));
        assert!(video.existing_thumbnail.is_some());
        assert_eq!(h.s().timeline.duration_ms(), 5040);
        assert_eq!(h.s().timeline.position_ms(), 1040);
        assert_eq!(h.s().current_frame_position_ms, Some(1040));
        assert!(
            h.fx.iter()
                .any(|e| matches!(e, Effect::SetFilmstripVideo(Some(_))))
        );
        assert_eq!(h.extracts(), [1040]);
        assert_eq!(h.s().reencode_reason.as_deref(), Some("attached picture"));
    }

    #[test]
    fn in_place_save_without_shift_keeps_timeline() {
        let mut h = Harness::new();
        selected(&mut h);
        h.c.state.skip_overwrite_warning = true;
        h.send(Event::OverwriteClicked);
        h.send(Event::SaveFinished(ApplyResult::ok()));
        assert!(h.extracts().is_empty());
        assert_eq!(h.s().timeline.duration_ms(), 5000);
        assert!(h.s().video.as_ref().unwrap().existing_thumbnail.is_some());
    }

    #[test]
    fn duration_shift_is_clamped() {
        let (v, p) = apply_duration_shift(&video(1000), &RgbImage::new(1, 1), Some(1000), 40);
        assert_eq!((v.duration_ms, p), (1040, Some(1040)));
        let (v, p) = apply_duration_shift(
            &VideoFile {
                thumbnail_frame_number: Some(3),
                ..video(1000)
            },
            &RgbImage::new(1, 1),
            None,
            0,
        );
        assert_eq!((v.thumbnail_frame_number, p), (Some(3), None));
    }

    #[test]
    fn save_failure_shows_typed_error_and_restores_button() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::SaveCopyClicked);
        h.send(Event::SaveFinished(ApplyResult::failure(
            ApplyError::FfmpegError,
            "ffmpeg failed",
        )));
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Error { report, .. })
            if report.title == "Could not save the thumbnail" && report.description.contains("Try another frame"))
        );
        assert!(h.s().save_enabled());
        assert_eq!(h.s().saved_label(), None);
    }

    #[test]
    fn worker_crash_during_save_shows_error() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::SaveCopyClicked);
        h.send(Event::SaveFailed("catastrophic fail".into()));
        assert!(matches!(h.s().top_dialog(), Some(Dialog::Error { .. })));
        assert!(!h.s().saving);
        assert!(h.s().save_enabled());
    }

    #[test]
    fn overwrite_warning_choices() {
        for (choice, expected) in [
            (
                OverwriteChoice::SaveCopy,
                Some(Some(PathBuf::from("/fake/video-thumbnail.mp4"))),
            ),
            (OverwriteChoice::Overwrite, Some(None)),
            (OverwriteChoice::Cancel, None),
        ] {
            let mut h = Harness::new();
            selected(&mut h);
            assert!(h.send(Event::OverwriteClicked).is_empty());
            assert_eq!(h.s().top_dialog(), Some(&Dialog::OverwriteWarning));
            h.send(Event::OverwriteWarningAnswered {
                choice,
                suppress: false,
            });
            assert_eq!(
                h.save_job().map(|j| j.destination.clone()),
                expected,
                "{choice:?}"
            );
            assert!(h.s().dialogs.is_empty());
        }
    }

    #[test]
    fn warning_suppression_is_session_only_and_cancel_does_not_suppress() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::OverwriteClicked);
        h.send(Event::OverwriteWarningAnswered {
            choice: OverwriteChoice::Cancel,
            suppress: true,
        });
        assert!(!h.s().skip_overwrite_warning);
        h.send(Event::OverwriteClicked);
        h.send(Event::OverwriteWarningAnswered {
            choice: OverwriteChoice::Overwrite,
            suppress: true,
        });
        assert!(h.s().skip_overwrite_warning);
        h.send(Event::SaveFinished(ApplyResult::ok()));
        h.send(Event::OverwriteClicked);
        assert!(h.s().dialogs.is_empty());
        assert!(h.save_job().unwrap().destination.is_none());
        assert!(!Harness::new().s().skip_overwrite_warning);
    }

    #[test]
    fn copy_conflict_choices() {
        for (choice, expected) in [
            (
                ConflictChoice::AnotherSuffix,
                Some(("/fake/video-thumbnail-thumbnail.mp4", false)),
            ),
            (
                ConflictChoice::Overwrite,
                Some(("/fake/video-thumbnail.mp4", true)),
            ),
            (ConflictChoice::Cancel, None),
        ] {
            let mut h = Harness::new();
            selected(&mut h);
            h.send(Event::SaveCopyClicked);
            h.send(Event::SaveFinished(ApplyResult::failure(
                ApplyError::DestinationExists,
                "Concurrent output preserved at /fake/recovery.mp4",
            )));
            match h.s().top_dialog() {
                Some(Dialog::CopyConflict {
                    destination,
                    details,
                }) => {
                    assert_eq!(destination, Path::new("/fake/video-thumbnail.mp4"));
                    assert!(details.as_deref().unwrap().contains("recovery.mp4"));
                    assert!(Dialog::conflict_text(destination).contains("add another suffix"));
                }
                other => panic!("{other:?}"),
            }
            h.send(Event::CopyConflictAnswered(choice));
            let got = h
                .save_job()
                .map(|j| (j.destination.clone().unwrap(), j.overwrite));
            assert_eq!(
                got,
                expected.map(|(p, o)| (PathBuf::from(p), o)),
                "{choice:?}"
            );
        }
    }

    #[test]
    fn new_video_resets_copy_destination() {
        let mut h = Harness::new();
        selected(&mut h);
        h.c.state.copy_destination = Some("/fake/video-thumbnail-thumbnail.mp4".into());
        h.load(VideoFile {
            path: "/fake/other.mov".into(),
            ..video(5000)
        });
        assert_eq!(h.s().copy_destination, None);
    }

    #[test]
    fn copy_path_naming() {
        assert_eq!(
            default_copy_path(Path::new("/v/a.b.mp4")),
            PathBuf::from("/v/a.b-thumbnail.mp4")
        );
        assert_eq!(
            default_copy_path(Path::new("/v/noext")),
            PathBuf::from("/v/noext-thumbnail")
        );
    }

    // -- image as thumbnail --------------------------------------------------

    impl Harness {
        fn image_requests(&self) -> Vec<(PathBuf, u32, u32, u64)> {
            self.fx
                .iter()
                .filter_map(|e| match e {
                    Effect::LoadImage {
                        path,
                        width,
                        height,
                        token,
                    } => Some((path.clone(), *width, *height, *token)),
                    _ => None,
                })
                .collect()
        }

        /// Complete the pending image load with a 2x2 "fit" and a 3x3 "crop"
        /// composition (told apart by size).
        fn complete_image(&mut self) {
            let [(path, ..)] = &self.image_requests()[..] else {
                panic!("expected one image request: {:?}", self.fx);
            };
            let path = path.clone();
            let token = self.c.image_token;
            self.send(Event::ImageLoaded {
                token,
                path,
                fit: RgbImage::new(2, 2),
                crop: RgbImage::new(3, 3),
            });
        }

        fn dropped_image(&mut self) {
            self.send(Event::OpenPath("/pics/Cover Photo.JPG".into()));
            self.complete_image();
        }
    }

    #[test]
    fn dropped_image_becomes_the_source_composed_to_the_video_size() {
        let mut h = Harness::new();
        selected(&mut h);
        let dir_before = h.s().last_open_directory.clone();
        h.send(Event::OpenPath("/pics/Cover Photo.JPG".into()));
        assert_eq!(
            h.image_requests(),
            [(
                PathBuf::from("/pics/Cover Photo.JPG"),
                1920,
                1080,
                h.c.image_token
            )]
        );
        assert!(
            !h.fx.iter().any(|e| matches!(e, Effect::LoadVideo { .. })),
            "an image is not opened as a video"
        );
        assert_eq!(h.s().last_open_directory, dir_before);
        assert!(h.s().cover.is_none(), "frame mode until the image is read");
        h.complete_image();
        assert_eq!(h.s().frame_title(), "Image (Cover Photo.JPG)");
        assert_eq!(h.s().panel_image().unwrap().dimensions(), (2, 2), "Fit");
        assert!(h.s().save_enabled());
    }

    #[test]
    fn image_button_opens_a_picker_then_loads_the_choice() {
        let mut h = Harness::new();
        selected(&mut h);
        assert!(matches!(&h.send(Event::OpenImageRequested)[..],
            [Effect::PickImageFile { start_dir }] if start_dir == Path::new("/fake")));
        // Whatever the extension, a picked file is read as an image.
        h.send(Event::OpenImage("/pics/cover.heic".into()));
        assert_eq!(h.image_requests().len(), 1);
        h.complete_image();
        assert_eq!(h.s().frame_title(), "Image (cover.heic)");
        // The picker then starts in the image's folder.
        assert!(matches!(&h.send(Event::OpenImageRequested)[..],
            [Effect::PickImageFile { start_dir }] if start_dir == Path::new("/pics")));
    }

    #[test]
    fn image_without_video_explains_what_to_do() {
        let mut h = Harness::new();
        assert!(h.send(Event::OpenPath("/pics/a.png".into())).is_empty());
        assert!(matches!(h.s().top_dialog(),
            Some(Dialog::Info { title, .. }) if title == IMAGE_NEEDS_VIDEO_TITLE));
        assert!(!h.s().busy);
        h.send(Event::DialogDismissed);
        assert!(h.send(Event::OpenImageRequested).is_empty());
        assert!(matches!(h.s().top_dialog(), Some(Dialog::Info { .. })));
    }

    #[test]
    fn image_given_while_a_video_loads_is_used_once_it_is_open() {
        let mut h = Harness::new();
        h.send(Event::OpenPath("/fake/video.mp4".into()));
        assert!(h.send(Event::OpenPath("/pics/a.png".into())).is_empty());
        assert!(h.s().dialogs.is_empty());
        let token = h.c.load_token;
        h.send(Event::VideoLoaded {
            token,
            video: Box::new(video(5000)),
        });
        assert_eq!(h.image_requests().len(), 1);
        assert_eq!(h.extracts().len(), 1, "the frame is still decoded");
        h.complete_image();
        assert_eq!(h.s().frame_title(), "Image (a.png)");

        // Dropped if the video can't be opened.
        let mut h = Harness::new();
        h.send(Event::OpenPath("/fake/broken.mp4".into()));
        h.send(Event::OpenPath("/pics/a.png".into()));
        let token = h.c.load_token;
        h.send(Event::LoadFailed {
            token,
            details: "boom".into(),
        });
        assert_eq!(h.s().pending_image, None);
    }

    #[test]
    fn dropped_video_still_opens_and_leaves_image_mode() {
        let mut h = Harness::new();
        selected(&mut h);
        h.dropped_image();
        h.send(Event::OpenPath("/fake/other.MOV".into()));
        assert!(h.fx.iter().any(
            |e| matches!(e, Effect::LoadVideo { path, .. } if path == Path::new("/fake/other.MOV"))
        ));
        h.load(VideoFile {
            path: "/fake/other.MOV".into(),
            ..video(5000)
        });
        assert!(h.s().cover.is_none());
        assert!(h.s().frame_title().starts_with("Frame"));
    }

    #[test]
    fn fit_and_crop_switch_the_composed_image() {
        let mut h = Harness::new();
        selected(&mut h);
        h.dropped_image();
        h.send(Event::SetImageFit(ImageFit::Crop));
        assert_eq!(h.s().panel_image().unwrap().dimensions(), (3, 3));
        h.send(Event::SaveCopyClicked);
        assert_eq!(h.save_job().unwrap().thumbnail.dimensions(), (3, 3));
        h.send(Event::SaveFinished(ApplyResult {
            output_path: Some("/fake/video-thumbnail.mp4".into()),
            ..ApplyResult::ok()
        }));
        assert_eq!(h.s().thumbnail.as_ref().unwrap().dimensions(), (3, 3));
        h.send(Event::SetImageFit(ImageFit::Fit));
        assert_eq!(h.s().panel_image().unwrap().dimensions(), (2, 2));
        // The choice carries over to the next image.
        h.send(Event::SetImageFit(ImageFit::Crop));
        h.dropped_image();
        assert_eq!(h.s().cover.as_ref().unwrap().mode, ImageFit::Crop);
    }

    #[test]
    fn timeline_filmstrip_arrows_and_button_return_to_the_frame() {
        let mut h = Harness::new();
        selected(&mut h);
        let frame_title = h.s().frame_title();
        h.dropped_image();
        h.send(Event::UseVideoFrame);
        assert!(h.s().cover.is_none());
        assert_eq!(h.s().frame_title(), frame_title);
        assert_eq!(h.s().panel_image().unwrap().dimensions(), (32, 32));

        h.dropped_image();
        h.send(Event::TimelinePointer {
            ratio: 0.5,
            released: false,
        });
        assert!(h.s().cover.is_none());
        assert_eq!(h.extracts(), [2500]);
        h.complete_extract();

        h.dropped_image();
        h.send(Event::FilmstripSelected(4000));
        assert!(h.s().cover.is_none());
        h.complete_extract();

        h.dropped_image();
        h.send(Event::StepFrame(-1));
        assert!(h.s().cover.is_none());
        assert_eq!(h.extracts(), [3960]);
    }

    #[test]
    fn image_save_carries_the_composed_image_without_a_position() {
        let mut h = Harness::new();
        selected(&mut h);
        h.dropped_image();
        h.send(Event::SaveCopyClicked);
        let job = h.save_job().unwrap();
        assert_eq!(job.thumbnail.dimensions(), (2, 2));
        assert_eq!(job.position_ms, None);
        // The source can't change while saving.
        h.send(Event::UseVideoFrame);
        h.send(Event::SetImageFit(ImageFit::Crop));
        h.send(Event::OpenPath("/pics/b.png".into()));
        assert!(h.image_requests().is_empty());
        let cover = h.s().cover.as_ref().unwrap();
        assert_eq!(cover.mode, ImageFit::Fit);
    }

    #[test]
    fn in_place_image_save_shifts_the_timeline_and_stays_in_image_mode() {
        let mut h = Harness::new();
        h.load(VideoFile {
            thumbnail_frame_number: Some(25),
            thumbnail_position_ms: Some(1000),
            ..video(5000)
        });
        h.complete_extract();
        h.dropped_image();
        h.c.state.skip_overwrite_warning = true;
        h.send(Event::OverwriteClicked);
        let job = h.save_job().unwrap();
        assert_eq!((job.destination.clone(), job.position_ms), (None, None));
        h.send(Event::SaveFinished(ApplyResult {
            duration_shift_ms: 40,
            ..ApplyResult::ok()
        }));
        let video = h.s().video.clone().unwrap();
        assert_eq!(video.duration_ms, 5040);
        assert_eq!(video.thumbnail_position_ms, None);
        assert_eq!(video.thumbnail_frame_number, None);
        assert_eq!(
            video.existing_thumbnail.as_ref().unwrap().dimensions(),
            (2, 2)
        );
        assert_eq!(h.s().timeline.duration_ms(), 5040);
        assert_eq!(h.s().timeline.position_ms(), 1040);
        assert_eq!(h.s().current_frame_position_ms, Some(1040));
        assert_eq!(h.extracts(), [1040], "the frame under the timeline moved");
        assert!(h.s().cover.is_some(), "still in image mode");
        assert_eq!(h.s().panel_image().unwrap().dimensions(), (2, 2));
        assert_eq!(h.s().thumbnail.as_ref().unwrap().dimensions(), (2, 2));
        h.complete_extract();
        assert!(h.s().cover.is_some());
        assert!(h.s().save_enabled());
    }

    #[test]
    fn unreadable_image_shows_error_and_keeps_the_mode() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::OpenPath("/pics/broken.png".into()));
        let token = h.c.image_token;
        h.send(Event::ImageLoadFailed {
            token,
            details: "Could not read the image".into(),
        });
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Error { report, .. })
            if report.title == "Could not use this image"
                && report.stack_trace == "Could not read the image")
        );
        assert!(h.s().cover.is_none());
        assert!(h.s().frame_title().starts_with("Frame ("));

        // In image mode a failure keeps the current image.
        h.send(Event::DialogDismissed);
        h.dropped_image();
        h.send(Event::OpenPath("/pics/broken.png".into()));
        let token = h.c.image_token;
        h.send(Event::ImageLoadFailed {
            token,
            details: "x".into(),
        });
        assert_eq!(h.s().frame_title(), "Image (Cover Photo.JPG)");
    }

    #[test]
    fn stale_image_results_are_ignored() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::OpenPath("/pics/a.png".into()));
        let stale = h.c.image_token;
        h.load(VideoFile {
            path: "/fake/other.mp4".into(),
            ..video(5000)
        });
        h.send(Event::ImageLoaded {
            token: stale,
            path: "/pics/a.png".into(),
            fit: RgbImage::new(2, 2),
            crop: RgbImage::new(3, 3),
        });
        assert!(h.s().cover.is_none());
        h.send(Event::ImageLoadFailed {
            token: stale,
            details: "x".into(),
        });
        assert!(h.s().dialogs.is_empty());
    }

    #[test]
    fn image_paths_are_recognised_by_extension() {
        assert!(is_image_path(Path::new("/a/b.JPEG")));
        assert!(is_image_path(Path::new("b.webp")));
        assert!(!is_image_path(Path::new("b.mp4")));
        assert!(!is_image_path(Path::new("png")));
    }

    // -- conversion ----------------------------------------------------------

    fn unsupported(h: &mut Harness, path: &str) {
        h.send(Event::OpenPath(path.into()));
        let token = h.c.load_token;
        h.send(Event::LoadUnsupported {
            token,
            path: path.into(),
        });
    }

    #[test]
    fn unsupported_load_offers_conversion() {
        let mut h = Harness::new();
        unsupported(&mut h, "/fake/clip.wmv");
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::ConvertOffer { path }) if path == Path::new("/fake/clip.wmv"))
        );
        assert!(!h.s().busy);
        assert_eq!(h.s().drop_zone_text().0, DROP_TITLE);
        h.send(Event::ConvertOfferAnswered(true));
        assert!(
            matches!(&h.fx[..], [Effect::PickConvertDestination { suggested, .. }] if suggested == Path::new("/fake/clip.mp4"))
        );
    }

    #[test]
    fn declining_or_cancelling_conversion_does_nothing() {
        let mut h = Harness::new();
        unsupported(&mut h, "/fake/clip.wmv");
        assert!(h.send(Event::ConvertOfferAnswered(false)).is_empty());
        assert!(h.s().dialogs.is_empty());
        assert!(
            h.send(Event::ConvertDestinationChosen {
                source: "/fake/clip.wmv".into(),
                destination: None
            })
            .is_empty()
        );
        assert!(!h.s().converting);
    }

    #[test]
    fn successful_conversion_opens_mp4() {
        let mut h = Harness::new();
        h.send(Event::ConvertDestinationChosen {
            source: "/fake/a.wmv".into(),
            destination: Some("/fake/a.mp4".into()),
        });
        assert!(
            matches!(&h.fx[..], [Effect::Convert { destination, .. }] if destination == Path::new("/fake/a.mp4"))
        );
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Converting { label }) if label == CONVERTING_INITIAL_LABEL)
        );
        h.send(Event::ConvertProgress("Encoding H.264".into()));
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Converting { label }) if label == "Encoding H.264")
        );
        // The progress dialog cannot be dismissed, and blocks other loads.
        h.send(Event::DialogDismissed);
        assert!(h.s().converting && !h.s().can_close());
        assert!(h.send(Event::OpenPath("/fake/other.mp4".into())).is_empty());
        h.send(Event::ConvertFinished("/fake/a.mp4".into()));
        assert!(h.s().dialogs.is_empty());
        assert!(h.fx.iter().any(
            |e| matches!(e, Effect::LoadVideo { path, .. } if path == Path::new("/fake/a.mp4"))
        ));
    }

    #[test]
    fn conversion_error_restores_ui_and_keeps_current_video() {
        let mut h = Harness::new();
        selected(&mut h);
        h.send(Event::ConvertDestinationChosen {
            source: "/fake/a.wmv".into(),
            destination: Some("/fake/a.mp4".into()),
        });
        h.send(Event::ConvertFailed("ffmpeg exploded".into()));
        assert!(!h.s().converting && !h.s().busy);
        assert!(h.s().editor_visible());
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Error { report, .. }) if report.title == "Could not convert this video")
        );
    }

    #[test]
    fn conversion_name_avoids_existing_files() {
        let src = Path::new("/v/clip.wmv");
        assert_eq!(
            suggest_conversion_path(src, |_| false),
            PathBuf::from("/v/clip.mp4")
        );
        assert_eq!(
            suggest_conversion_path(src, |_| true),
            PathBuf::from("/v/clip-converted.mp4")
        );
        assert_eq!(
            suggest_conversion_path(Path::new("/v/x.mp4"), |_| false),
            PathBuf::from("/v/x-converted.mp4")
        );
    }

    // -- updates and misc ----------------------------------------------------

    fn info(status: UpdateStatus) -> UpdateInfo {
        UpdateInfo {
            status,
            current_version: "0.1.0".into(),
            latest_version: Some("0.2.0".into()),
            release_url: "https://github.com/fgagarin/framecover/releases/tag/v0.2.0".into(),
        }
    }

    #[test]
    fn checks_automatically_on_startup_and_ignores_concurrent_checks() {
        let mut h = Harness::with_checker(true);
        assert!(h.s().update.visible);
        assert_eq!(h.s().update.text, "Updates not checked yet");
        assert!(matches!(
            &h.send(Event::Startup)[..],
            [Effect::CheckUpdates]
        ));
        assert_eq!(h.s().update.text, "Checking for updates…");
        assert!(!h.s().update.enabled);
        assert!(h.send(Event::UpdateIndicatorClicked).is_empty());
        h.send(Event::UpdateChecked(info(UpdateStatus::UpToDate)));
        assert_eq!(h.s().update.text, "You have the latest version");
        assert_eq!(h.s().update.dot, Dot::Latest);
        assert!(matches!(
            &h.send(Event::UpdateIndicatorClicked)[..],
            [Effect::CheckUpdates]
        ));
    }

    #[test]
    fn no_checker_means_no_check() {
        let mut h = Harness::new();
        assert!(h.send(Event::Startup).is_empty());
    }

    #[test]
    fn available_update_opens_release_without_rechecking() {
        let mut h = Harness::with_checker(true);
        h.send(Event::Startup);
        h.send(Event::UpdateChecked(info(UpdateStatus::UpdateAvailable)));
        assert_eq!(h.s().update.text, "Version 0.2.0 is available");
        assert_eq!(h.s().update.dot, Dot::Update);
        assert!(h.s().update.tooltip.starts_with("You have 0.1.0."));
        assert!(
            matches!(&h.send(Event::UpdateIndicatorClicked)[..], [Effect::OpenReleaseUrl(u)] if u.ends_with("v0.2.0"))
        );
        h.send(Event::ReleaseOpenFailed("https://x/rel".into()));
        assert!(
            matches!(h.s().top_dialog(), Some(Dialog::Info { text, .. }) if text.ends_with("https://x/rel"))
        );
    }

    #[test]
    fn failed_check_explains_how_to_retry() {
        let mut h = Harness::with_checker(true);
        h.send(Event::Startup);
        h.send(Event::UpdateCheckFailed("You are offline.".into()));
        assert_eq!(h.s().update.text, "Couldn't check for updates");
        assert_eq!(h.s().update.dot, Dot::Error);
        assert_eq!(
            h.s().update.tooltip,
            "You are offline.\nClick this indicator to check for updates."
        );
        assert!(matches!(
            &h.send(Event::UpdateIndicatorClicked)[..],
            [Effect::CheckUpdates]
        ));
    }

    #[test]
    fn test_build_newer_than_release() {
        let mut h = Harness::with_checker(true);
        h.send(Event::UpdateChecked(info(UpdateStatus::NewerThanRelease)));
        assert_eq!(
            h.s().update.text,
            "Test build · newer than the latest release"
        );
        assert!(
            h.s()
                .update
                .tooltip
                .starts_with("Latest published release: 0.2.0.")
        );
    }

    #[test]
    fn path_click_reveals_video() {
        let mut h = Harness::new();
        assert!(h.send(Event::PathClicked).is_empty());
        h.loaded();
        assert!(
            matches!(&h.send(Event::PathClicked)[..], [Effect::RevealInFolder(p)] if p == Path::new("/fake/video.mp4"))
        );
    }

    #[test]
    fn shortcuts_dialog_lists_open_save_and_help() {
        let mut h = Harness::new();
        h.send(Event::ShowShortcuts);
        match h.s().top_dialog() {
            Some(Dialog::Info { title, text }) => {
                assert_eq!(title, "Keyboard shortcuts");
                for needle in ["+O", "+S", "+I", "Left / Right", "Shift+/"] {
                    assert!(text.contains(needle));
                }
            }
            other => panic!("{other:?}"),
        }
        // Shortcuts are inert while a dialog is open.
        assert!(h.send(Event::OpenFileRequested).is_empty());
        h.send(Event::DialogDismissed);
        assert!(h.s().dialogs.is_empty());
    }

    #[test]
    fn unhandled_errors_do_not_stack() {
        let mut h = Harness::new();
        h.send(Event::UnhandledError(ErrorReport::new("a", "trace a")));
        h.send(Event::UnhandledError(ErrorReport::new("b", "trace b")));
        assert_eq!(h.s().dialogs.len(), 1);
        h.send(Event::DialogDismissed);
        h.send(Event::UnhandledError(ErrorReport::new("c", "trace c")));
        assert_eq!(h.s().dialogs.len(), 1);
    }
}
