//! Glue between the Slint window and the [`Controller`].
//!
//! * Slint callbacks and worker results become controller [`Event`]s.
//! * Returned [`Effect`]s are executed here (threads, native dialogs, …).
//! * After every event the window properties are re-rendered from `State`.
//!
//! Workers run on plain threads and hand results back with
//! `slint::invoke_from_event_loop`; the [`App`] itself lives in a
//! thread-local on the UI thread.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::RgbImage;
use rand::SeedableRng;
use rand::rngs::StdRng;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{
    ComponentHandle, Image, ModelRc, Rgb8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel,
};

use framecover_core::imaging::{IMAGE_EXTENSIONS, ImageFit};

use crate::controller::{
    CONFLICT_TITLE, CONVERT_TEXT, CONVERT_TITLE, CONVERTING_TITLE, ConflictChoice, Controller,
    Dialog, Dot, Effect, Event, OVERWRITE_TEXT, OVERWRITE_TITLE, OverwriteChoice, VIDEO_EXTENSIONS,
    is_image_path,
};
use crate::error_report::{
    ErrorReport, build_google_search_url, build_issue_url, format_panic_report,
};
use crate::filmstrip::{FilmJob, Filmstrip, TARGET_TILE_WIDTH, TILE_HEIGHT};
use crate::frame_worker::FrameWorker;
use crate::services::Services;
use crate::settings::Settings;
use crate::window_fit;
use crate::workers::{self, LoadOutcome, panic_message, spawn_guarded};
use crate::{AppWindow, DialogData, DialogKind, FilmTile};

pub const AUTHOR_NAME: &str = "Fedor Gagarin";
pub const PROJECT_URL: &str = "https://fgagarin.github.io/framecover/";

/// Filmstrip jobs wait until preview frame requests have paused this long…
const FILM_YIELD_QUIET: Duration = Duration::from_millis(150);
/// …but no longer than this, so a long scrub cannot stall the filmstrip.
const FILM_YIELD_MAX: Duration = Duration::from_secs(2);

/// Messages delivered to the UI thread.
enum Msg {
    Event(Event),
    FilmTile {
        generation: u64,
        frame_index: u64,
        image: Option<SharedPixelBuffer<Rgb8Pixel>>,
    },
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Deliver a controller event to the UI thread (callable from any thread).
pub fn post(event: Event) {
    post_msg(Msg::Event(event));
}

fn post_msg(msg: Msg) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = APP.with(|a| a.borrow().clone()) {
            app.receive(msg);
        }
    });
}

struct App {
    window: AppWindow,
    controller: RefCell<Controller>,
    services: Services,
    /// Decodes the preview frames (one persistent source per video).
    frames: FrameWorker,
    settings: RefCell<Settings>,
    filmstrip: RefCell<Filmstrip<Image>>,
    progress_timer: Timer,
    frame_cache: RefCell<Option<Arc<RgbImage>>>,
    thumb_cache: RefCell<Option<Arc<RgbImage>>>,
    /// `(stack depth, kind)` of the dialog rendered last.
    last_dialog: Cell<(usize, DialogKind)>,
    drop_taken: Cell<bool>,
}

/// Install the panic hook, build the window and run the event loop.
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    install_panic_hook();
    crate::services::set_app_user_model_id();
    crate::embedded::install();

    let services = Services::production();
    let settings = Settings::load();
    let window = AppWindow::new().expect("could not create the main window");
    let controller = Controller::new(
        Arc::clone(&services.writer),
        settings.last_open_directory(),
        services.update_checker.is_some(),
        StdRng::from_os_rng(),
    );
    let app = Rc::new(App {
        window,
        controller: RefCell::new(controller),
        frames: FrameWorker::spawn(Arc::clone(&services.frame_sources)),
        services,
        settings: RefCell::new(settings),
        filmstrip: RefCell::new(Filmstrip::default()),
        progress_timer: Timer::default(),
        frame_cache: RefCell::new(None),
        thumb_cache: RefCell::new(None),
        last_dialog: Cell::new((0, DialogKind::None)),
        drop_taken: Cell::new(false),
    });
    APP.with(|a| *a.borrow_mut() = Some(Rc::clone(&app)));

    app.bind();
    app.render();
    app.dispatch(Event::Startup);
    // `framecover video [image]`: an image after the video becomes its
    // thumbnail source once the video is open.
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    if let Some(path) = args.next() {
        app.dispatch(Event::OpenPath(path));
    }
    if let Some(image) = args.next().filter(|p| is_image_path(p)) {
        app.dispatch(Event::OpenPath(image));
    }

    if let Err(err) = app.window.run() {
        log::error!("event loop failed: {err}");
    }
    APP.with(|a| a.borrow_mut().take());
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = panic_message(info.payload());
        let location = info.location().map(|l| l.to_string());
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let report = format_panic_report(&message, location.as_deref(), &backtrace);
        log::error!("Unhandled panic: {}", report.stack_trace);
        if workers::in_guarded_section() {
            workers::record_panic(report.stack_trace);
        } else {
            post(Event::UnhandledError(report));
        }
        if cfg!(debug_assertions) {
            default_hook(info);
        }
    }));
}

/// Report for a panic caught by [`workers::guarded`] (`details` is the
/// formatted trace recorded by the hook).
fn report_from_details(details: String) -> ErrorReport {
    let message = details.lines().nth(1).unwrap_or("").trim();
    let summary = if message.is_empty() {
        "panicked".to_owned()
    } else {
        format!("panicked: {message}")
    };
    ErrorReport::new(summary, details)
}

fn to_pixel_buffer(img: &RgbImage) -> SharedPixelBuffer<Rgb8Pixel> {
    SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height())
}

/// Physical rectangle where the window may be placed: the work area (without
/// the taskbar) of the monitor under the window's centre on Windows, the
/// whole current monitor elsewhere.
fn screen_work_area(
    window: &winit::window::Window,
    position: (i32, i32),
    size: (u32, u32),
) -> Option<window_fit::Rect> {
    let centre = (
        position.0.saturating_add((size.0 / 2) as i32),
        position.1.saturating_add((size.1 / 2) as i32),
    );
    #[cfg(windows)]
    {
        let _ = window;
        window_fit::work_area(centre)
    }
    #[cfg(not(windows))]
    {
        let _ = centre;
        let monitor = window.current_monitor()?;
        let (origin, extent) = (monitor.position(), monitor.size());
        Some(window_fit::Rect {
            left: origin.x,
            top: origin.y,
            right: origin.x.saturating_add(extent.width as i32),
            bottom: origin.y.saturating_add(extent.height as i32),
        })
    }
}

fn to_image(img: &RgbImage) -> Image {
    Image::from_rgb8(to_pixel_buffer(img))
}

/// Footer update dot colour.
fn dot_color(dot: Dot) -> slint::Color {
    let hex = match dot {
        Dot::Idle => 0x9ca3af,
        Dot::Latest => 0x3b82f6,
        Dot::Update => 0x22c55e,
        Dot::Error => 0xf59e0b,
    };
    slint::Color::from_argb_encoded(0xff00_0000 | hex)
}

/// Footer update text colour: only states that need attention are coloured;
/// the rest use the footer's grey (`transparent`).
fn status_color(dot: Dot) -> slint::Color {
    match dot {
        Dot::Idle | Dot::Latest => slint::Color::from_argb_encoded(0),
        Dot::Update => slint::Color::from_argb_encoded(0xff16_a34a),
        Dot::Error => slint::Color::from_argb_encoded(0xffd9_7706),
    }
}

impl App {
    fn receive(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Event(event) => self.dispatch(event),
            Msg::FilmTile {
                generation,
                frame_index,
                image,
            } => {
                let (jobs, changed) = self.filmstrip.borrow_mut().on_result(
                    generation,
                    frame_index,
                    image.map(Image::from_rgb8),
                );
                self.run_film_jobs(jobs);
                if changed {
                    self.render_filmstrip();
                }
            }
        }
    }

    fn dispatch(self: &Rc<Self>, event: Event) {
        let outcome = workers::guarded(|| {
            let effects = self.controller.borrow_mut().handle(event);
            self.render();
            for effect in effects {
                self.execute(effect);
            }
        });
        if let Err(details) = outcome {
            post(Event::UnhandledError(report_from_details(details)));
        }
    }

    fn bind(self: &Rc<Self>) {
        let w = &self.window;
        w.set_version(framecover_core::VERSION.into());
        w.set_project_url(PROJECT_URL.into());

        macro_rules! on {
            ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = Rc::downgrade(self);
                w.$setter(move |$($arg),*| {
                    if let Some($app) = weak.upgrade() {
                        $body;
                    }
                });
            }};
        }

        on!(on_open_file, |app| app.dispatch(Event::OpenFileRequested));
        on!(on_path_clicked, |app| app.dispatch(Event::PathClicked));
        on!(on_open_image, |app| app.dispatch(Event::OpenImageRequested));
        on!(on_use_video_frame, |app| app.dispatch(Event::UseVideoFrame));
        on!(on_set_image_crop, |app, crop| app.dispatch(
            Event::SetImageFit(if crop { ImageFit::Crop } else { ImageFit::Fit })
        ));
        on!(on_save_copy, |app| app.dispatch(Event::SaveCopyClicked));
        on!(on_overwrite, |app| app.dispatch(Event::OverwriteClicked));
        on!(on_show_reencode_reason, |app| app
            .dispatch(Event::ShowReencodeReason));
        on!(on_step_frame, |app, direction| app
            .dispatch(Event::StepFrame(direction)));
        on!(on_show_shortcuts, |app| app.dispatch(Event::ShowShortcuts));
        on!(on_show_about, |app| app.dispatch(Event::ShowAbout));
        on!(on_update_clicked, |app| app
            .dispatch(Event::UpdateIndicatorClicked));
        on!(on_dialog_dismiss, |app| app
            .dispatch(Event::DialogDismissed));
        on!(on_convert_answer, |app, yes| app
            .dispatch(Event::ConvertOfferAnswered(yes)));
        on!(on_open_project_page, |app| {
            app.services.desktop.open_url(PROJECT_URL);
        });
        on!(on_timeline_pointer, |app, ratio, released| app.dispatch(
            Event::TimelinePointer {
                ratio: f64::from(ratio),
                released
            }
        ));
        on!(on_overwrite_answer, |app, choice, suppress| {
            let choice = match choice {
                0 => OverwriteChoice::SaveCopy,
                1 => OverwriteChoice::Overwrite,
                _ => OverwriteChoice::Cancel,
            };
            app.dispatch(Event::OverwriteWarningAnswered { choice, suppress })
        });
        on!(on_conflict_answer, |app, choice| {
            let choice = match choice {
                0 => ConflictChoice::AnotherSuffix,
                1 => ConflictChoice::Overwrite,
                _ => ConflictChoice::Cancel,
            };
            app.dispatch(Event::CopyConflictAnswered(choice))
        });
        on!(on_error_find_cause, |app| app
            .open_error_link(build_google_search_url));
        on!(on_error_report_problem, |app| app
            .open_error_link(build_issue_url));
        on!(on_film_clicked, |app, x| {
            let position = app.filmstrip.borrow().position_at(x);
            if let Some(ms) = position {
                app.dispatch(Event::FilmstripSelected(ms));
            }
        });
        on!(on_film_resized, |app, width| {
            let jobs = app.filmstrip.borrow_mut().set_width(width);
            app.run_film_jobs(jobs);
            app.render_filmstrip();
        });

        // The window cannot close while a conversion or save is running.
        let weak = Rc::downgrade(self);
        w.window().on_close_requested(move || match weak.upgrade() {
            Some(app) if !app.controller.borrow().state.can_close() => {
                slint::CloseRequestResponse::KeepWindowShown
            }
            _ => slint::CloseRequestResponse::HideWindow,
        });

        self.bind_file_drops();
    }

    /// OS file drag-and-drop. Slint 1.18's `DropArea` only receives drags
    /// started inside Slint, so external file drops are taken from the
    /// underlying winit window events (supported on Windows, macOS and X11;
    /// winit has no drag-and-drop on Wayland yet).
    fn bind_file_drops(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window
            .window()
            .on_winit_window_event(move |_window, event| {
                let Some(app) = weak.upgrade() else {
                    return EventResult::Propagate;
                };
                match event {
                    winit::event::WindowEvent::HoveredFile(_) => {
                        if !app.window.get_drag_hover() {
                            app.drop_taken.set(false);
                            app.window.set_drag_hover(true);
                        }
                    }
                    winit::event::WindowEvent::HoveredFileCancelled => {
                        app.window.set_drag_hover(false)
                    }
                    winit::event::WindowEvent::DroppedFile(path) => {
                        app.window.set_drag_hover(false);
                        // Several files dropped at once arrive as several
                        // events; like the Qt version, open only the first.
                        if !app.drop_taken.replace(true) {
                            post(Event::OpenPath(path.clone()));
                        }
                    }
                    _ => return EventResult::Propagate,
                }
                EventResult::PreventDefault
            });
    }

    fn open_error_link(&self, build: fn(&ErrorReport) -> String) {
        let url = match self.controller.borrow().state.top_dialog() {
            Some(Dialog::Error { report, .. }) => build(report),
            _ => return,
        };
        self.services.desktop.open_url(&url);
    }

    // -- effects --------------------------------------------------------------

    fn execute(self: &Rc<Self>, effect: Effect) {
        match effect {
            Effect::PickVideoFile { start_dir } => self.pick_video_file(start_dir),
            Effect::LoadVideo { path, token } => {
                let services = self.services.clone();
                spawn_guarded(
                    "load-video",
                    move || workers::run_load(&services, &path),
                    move |outcome| {
                        post(match outcome {
                            Ok(LoadOutcome::Loaded(video)) => Event::VideoLoaded { token, video },
                            Ok(LoadOutcome::Unsupported(path)) => {
                                Event::LoadUnsupported { token, path }
                            }
                            Ok(LoadOutcome::Failed(details)) | Err(details) => {
                                Event::LoadFailed { token, details }
                            }
                        })
                    },
                );
            }
            Effect::PickImageFile { start_dir } => self.pick_image_file(start_dir),
            Effect::LoadImage {
                path,
                width,
                height,
                token,
            } => spawn_guarded(
                "load-image",
                move || {
                    workers::run_load_image(&path, width, height)
                        .map(|(fit, crop)| (path, fit, crop))
                },
                move |outcome| {
                    post(match outcome.and_then(|r| r) {
                        Ok((path, fit, crop)) => Event::ImageLoaded {
                            token,
                            path,
                            fit,
                            crop,
                        },
                        Err(details) => Event::ImageLoadFailed { token, details },
                    })
                },
            ),
            Effect::PersistLastOpenDirectory(dir) => {
                self.settings.borrow_mut().set_last_open_directory(&dir)
            }
            Effect::ExtractFrame {
                video,
                position_ms,
                generation,
            } => self.frames.request(
                video,
                position_ms,
                generation,
                Box::new(move |outcome| {
                    post(match outcome {
                        Ok(image) => Event::FrameExtracted { generation, image },
                        Err(details) => Event::FrameExtractFailed {
                            generation,
                            details,
                        },
                    })
                }),
            ),
            Effect::SetFilmstripVideo(video) => {
                let jobs = self.filmstrip.borrow_mut().set_video(video);
                self.run_film_jobs(jobs);
                self.render_filmstrip();
            }
            Effect::SelectFilmstripPosition(ms) => {
                self.filmstrip.borrow_mut().set_selected_position(ms);
                self.render_filmstrip();
            }
            Effect::Save(job) => {
                let services = self.services.clone();
                let frames = self.frames.releaser();
                spawn_guarded(
                    "save-thumbnail",
                    move || {
                        if job.destination.is_none() {
                            // The preview decoder keeps the file open, which
                            // would block replacing it on Windows.
                            frames.release();
                        }
                        workers::run_apply(&services, &job, &mut |stage, elapsed| {
                            post(Event::SaveProgress {
                                stage: stage.to_owned(),
                                elapsed,
                            })
                        })
                    },
                    move |outcome| {
                        post(match outcome {
                            Ok(result) => Event::SaveFinished(result),
                            Err(details) => Event::SaveFailed(details),
                        })
                    },
                );
                self.start_progress_timer();
            }
            Effect::PickConvertDestination { source, suggested } => {
                self.pick_convert_destination(source, suggested)
            }
            Effect::Convert {
                source,
                destination,
            } => {
                let converter = Arc::clone(&self.services.converter);
                spawn_guarded(
                    "convert-video",
                    move || {
                        workers::run_convert(converter.as_ref(), &source, &destination, &mut |m| {
                            post(Event::ConvertProgress(m.to_owned()))
                        })
                    },
                    move |outcome| {
                        post(match outcome.and_then(|r| r) {
                            Ok(path) => Event::ConvertFinished(path),
                            Err(details) => Event::ConvertFailed(details),
                        })
                    },
                );
            }
            Effect::CheckUpdates => {
                let Some(checker) = self.services.update_checker.clone() else {
                    return;
                };
                spawn_guarded(
                    "update-check",
                    move || workers::run_update_check(checker.as_ref(), framecover_core::VERSION),
                    move |outcome| {
                        post(match outcome {
                            Ok(Ok(info)) => Event::UpdateChecked(info),
                            Ok(Err(message)) => Event::UpdateCheckFailed(message),
                            Err(_) => Event::UpdateCheckFailed(
                                workers::UNEXPECTED_UPDATE_FAILURE.to_owned(),
                            ),
                        })
                    },
                );
            }
            Effect::OpenReleaseUrl(url) => {
                if !self.services.desktop.open_url(&url) {
                    self.dispatch(Event::ReleaseOpenFailed(url));
                }
            }
            Effect::RevealInFolder(path) => self.services.desktop.reveal_in_file_manager(&path),
            Effect::ResizeWindow(width, height) => self.grow_window(width, height),
        }
    }

    /// Resize the window to `width`x`height` logical pixels, shrinking and
    /// moving it as needed so it stays inside its monitor's work area.
    fn grow_window(&self, width: f32, height: f32) {
        let window = self.window.window();
        if window.is_maximized() || window.is_fullscreen() {
            return;
        }
        let scale = window.scale_factor();
        let desired = (
            (width * scale).round() as u32,
            (height * scale).round() as u32,
        );
        let placement = window
            .with_winit_window(|w| {
                let (outer, inner) = (w.outer_size(), w.inner_size());
                let frame = (
                    outer.width.saturating_sub(inner.width),
                    outer.height.saturating_sub(inner.height),
                );
                let position = w.outer_position().ok()?;
                let work =
                    screen_work_area(w, (position.x, position.y), (outer.width, outer.height))?;
                Some(window_fit::fit(
                    desired,
                    frame,
                    (position.x, position.y),
                    work,
                ))
            })
            .flatten();
        match placement {
            Some(p) => {
                window.set_size(slint::PhysicalSize::new(p.inner.0, p.inner.1));
                window.set_position(slint::PhysicalPosition::new(
                    p.outer_position.0,
                    p.outer_position.1,
                ));
            }
            None => window.set_size(slint::LogicalSize::new(width, height)),
        }
    }

    fn pick_video_file(self: &Rc<Self>, start_dir: PathBuf) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Open video")
            .set_directory(start_dir)
            .add_filter("Video files", VIDEO_EXTENSIONS)
            .add_filter("All files", &["*"])
            .set_parent(&self.window.window().window_handle());
        let weak = Rc::downgrade(self);
        let spawned = slint::spawn_local(async move {
            let picked = dialog.pick_file().await;
            if let (Some(file), Some(app)) = (picked, weak.upgrade()) {
                app.dispatch(Event::OpenPath(file.path().to_owned()));
            }
        });
        if let Err(err) = spawned {
            log::error!("could not show the file dialog: {err}");
        }
    }

    fn pick_image_file(self: &Rc<Self>, start_dir: PathBuf) {
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Use image as thumbnail")
            .set_directory(start_dir)
            .add_filter("Images", IMAGE_EXTENSIONS)
            .add_filter("All files", &["*"])
            .set_parent(&self.window.window().window_handle());
        let weak = Rc::downgrade(self);
        let spawned = slint::spawn_local(async move {
            let picked = dialog.pick_file().await;
            if let (Some(file), Some(app)) = (picked, weak.upgrade()) {
                app.dispatch(Event::OpenImage(file.path().to_owned()));
            }
        });
        if let Err(err) = spawned {
            log::error!("could not show the file dialog: {err}");
        }
    }

    fn pick_convert_destination(self: &Rc<Self>, source: PathBuf, suggested: PathBuf) {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Save converted MP4")
            .add_filter("MP4 video", &["mp4"])
            .set_parent(&self.window.window().window_handle());
        if let Some(dir) = suggested.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        let weak = Rc::downgrade(self);
        let spawned = slint::spawn_local(async move {
            let destination = dialog.save_file().await.map(|f| f.path().to_owned());
            if let Some(app) = weak.upgrade() {
                app.dispatch(Event::ConvertDestinationChosen {
                    source,
                    destination,
                });
            }
        });
        if let Err(err) = spawned {
            log::error!("could not show the save dialog: {err}");
        }
    }

    fn run_film_jobs(&self, jobs: Vec<FilmJob>) {
        for job in jobs {
            let extractor = Arc::clone(&self.services.extractor);
            let activity = self.frames.activity();
            let (generation, frame_index) = (job.generation, job.frame_index);
            spawn_guarded(
                "filmstrip",
                move || {
                    // Let the preview decode first while the user scrubs.
                    activity.wait_until_quiet(FILM_YIELD_QUIET, FILM_YIELD_MAX);
                    let frame = extractor.extract(&job.video, job.position_ms).ok()?;
                    // Keep 2x the tile size for HiDPI screens.
                    let (w, h) = framecover_core::imaging::fit_within(
                        frame.width(),
                        frame.height(),
                        (TARGET_TILE_WIDTH * 2.0) as u32,
                        (TILE_HEIGHT * 2.0) as u32,
                    );
                    let small = image::imageops::thumbnail(&frame, w, h);
                    Some(to_pixel_buffer(&small))
                },
                move |outcome| {
                    post_msg(Msg::FilmTile {
                        generation,
                        frame_index,
                        image: outcome.ok().flatten(),
                    })
                },
            );
        }
    }

    fn start_progress_timer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.progress_timer
            .start(TimerMode::Repeated, Duration::from_millis(200), move || {
                if let Some(app) = weak.upgrade() {
                    let c = app.controller.borrow();
                    if c.state.save_progress.visible {
                        app.window
                            .set_progress_text(c.state.save_progress.text(Instant::now()).into());
                    } else {
                        app.progress_timer.stop();
                    }
                }
            });
    }

    // -- rendering ------------------------------------------------------------

    fn render_filmstrip(&self) {
        let strip = self.filmstrip.borrow();
        let tiles: Vec<FilmTile> = strip
            .tiles()
            .into_iter()
            .map(|t| FilmTile {
                x: t.x,
                width: t.width,
                image: t.image.cloned().unwrap_or_default(),
                has_image: t.image.is_some(),
                selected: t.selected,
            })
            .collect();
        self.window
            .set_film_tiles(ModelRc::new(VecModel::from(tiles)));
    }

    fn cached_image(
        cache: &RefCell<Option<Arc<RgbImage>>>,
        image: Option<&Arc<RgbImage>>,
    ) -> Option<Image> {
        let mut cache = cache.borrow_mut();
        match image {
            None => {
                *cache = None;
                None
            }
            Some(img) if cache.as_ref().is_some_and(|c| Arc::ptr_eq(c, img)) => None,
            Some(img) => {
                *cache = Some(Arc::clone(img));
                Some(to_image(img))
            }
        }
    }

    fn render(&self) {
        let c = self.controller.borrow();
        let s = &c.state;
        let w = &self.window;

        w.set_editor_visible(s.editor_visible());
        let (title, hint) = s.drop_zone_text();
        w.set_drop_title(title.into());
        w.set_drop_hint(hint.into());
        w.set_drops_accepted(s.drops_accepted());

        w.set_update_visible(s.update.visible);
        w.set_update_text(s.update.text.as_str().into());
        w.set_update_tooltip(s.update.tooltip.as_str().into());
        w.set_update_enabled(s.update.enabled);
        w.set_update_color(status_color(s.update.dot));
        w.set_update_dot_color(dot_color(s.update.dot));

        w.set_video_path(
            s.video
                .as_ref()
                .map(|v| v.path.display().to_string())
                .unwrap_or_default()
                .into(),
        );
        w.set_frame_title(s.frame_title().into());
        w.set_has_frame(s.panel_image().is_some());
        if let Some(image) = Self::cached_image(&self.frame_cache, s.panel_image()) {
            w.set_frame_image(image);
        }
        w.set_image_mode(s.cover.is_some());
        w.set_image_crop(s.cover.as_ref().is_some_and(|c| c.mode == ImageFit::Crop));
        w.set_has_thumb(s.thumbnail.is_some());
        if let Some(image) = Self::cached_image(&self.thumb_cache, s.thumbnail.as_ref()) {
            w.set_thumb_image(image);
        }
        w.set_progress_visible(s.save_progress.visible);
        w.set_progress_text(s.save_progress.text(Instant::now()).into());
        w.set_save_enabled(s.save_enabled());
        w.set_save_text(s.save_button_text().into());
        w.set_saving(s.saving);
        w.set_save_tooltip(s.save_tooltip.as_str().into());
        w.set_reencode_visible(s.reencode_reason.is_some());
        w.set_saved_label(s.saved_label().unwrap_or_default().into());

        w.set_timeline_enabled(s.timeline_enabled());
        w.set_timeline_has_duration(s.timeline.duration_ms() > 0);
        w.set_timeline_fill(s.timeline.fill_ratio());
        w.set_timeline_label(s.timeline.label().into());

        let data = dialog_data(s.top_dialog());
        let identity = (s.dialogs.len(), data.kind);
        w.set_dialog(data);
        if identity != self.last_dialog.replace(identity) {
            w.invoke_reset_dialog_state();
        }
    }
}

fn dialog_data(dialog: Option<&Dialog>) -> DialogData {
    let (kind, title, text, details) = match dialog {
        None => (
            DialogKind::None,
            String::new(),
            String::new(),
            String::new(),
        ),
        Some(Dialog::Info { title, text }) => {
            (DialogKind::Info, title.clone(), text.clone(), String::new())
        }
        Some(Dialog::ConvertOffer { .. }) => (
            DialogKind::ConvertOffer,
            CONVERT_TITLE.into(),
            CONVERT_TEXT.into(),
            String::new(),
        ),
        Some(Dialog::Converting { label }) => (
            DialogKind::Converting,
            CONVERTING_TITLE.into(),
            label.clone(),
            String::new(),
        ),
        Some(Dialog::OverwriteWarning) => (
            DialogKind::OverwriteWarning,
            OVERWRITE_TITLE.into(),
            OVERWRITE_TEXT.into(),
            String::new(),
        ),
        Some(Dialog::CopyConflict {
            destination,
            details,
        }) => (
            DialogKind::CopyConflict,
            CONFLICT_TITLE.into(),
            Dialog::conflict_text(destination),
            details.clone().unwrap_or_default(),
        ),
        Some(Dialog::About) => (
            DialogKind::About,
            "About FrameCover".into(),
            about_text(),
            String::new(),
        ),
        Some(Dialog::Error { report, .. }) => (
            DialogKind::Error,
            report.title.clone(),
            report.description.clone(),
            report.stack_trace.clone(),
        ),
    };
    DialogData {
        kind,
        title: title.into(),
        text: text.into(),
        details: details.into(),
    }
}

fn about_text() -> String {
    format!(
        "FrameCover {}\n\
Pick any frame of a video and make it the file's thumbnail.\n\n\
by {AUTHOR_NAME} · MIT License\n{PROJECT_URL}\n\n\
The user interface is made with Slint, used under the Slint Royalty-free License 2.0.\n\n\
Video processing is done by FFmpeg (ffmpeg.org), a separate program bundled with \
FrameCover and licensed under the GNU GPL v3. Licence text, build sources and the \
written source offer: https://github.com/fgagarin/framecover/blob/main/licenses/FFMPEG-NOTICE.txt",
        framecover_core::VERSION
    )
}
