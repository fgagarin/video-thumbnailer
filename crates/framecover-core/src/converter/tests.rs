//! Port of `tests/unit/test_video_converter.py` (FFmpeg/PyAV mocked through
//! the [`Backend`] seam).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::process::{ExitStatus, Output};

use super::*;

fn status_ok() -> ExitStatus {
    #[cfg(unix)]
    {
        std::os::unix::process::ExitStatusExt::from_raw(0)
    }
    #[cfg(windows)]
    {
        std::os::windows::process::ExitStatusExt::from_raw(0)
    }
}

fn ok_output(stdout: &str) -> Result<Output> {
    Ok(Output {
        status: status_ok(),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    })
}

fn ffmpeg_error() -> Result<Output> {
    Err(Error::Ffmpeg {
        code: 1,
        stderr: "unsupported codec".into(),
    })
}

type RunFn = Box<dyn FnMut(&[OsString]) -> Result<Output>>;

struct Fake {
    playable: bool,
    remux_blocker: Option<String>,
    locate: std::result::Result<(), String>,
    runs: RefCell<VecDeque<RunFn>>,
    default_run: RefCell<Option<RunFn>>,
    concat: RefCell<VecDeque<Result<Option<String>>>>,
    concat_default: Option<String>,
    any_video: RefCell<VecDeque<Result<bool>>>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            playable: true,
            remux_blocker: None,
            locate: Ok(()),
            runs: RefCell::default(),
            default_run: RefCell::new(None),
            concat: RefCell::default(),
            concat_default: None,
            any_video: RefCell::default(),
            calls: RefCell::default(),
        }
    }
}

impl Fake {
    fn script(self, runs: Vec<RunFn>) -> Self {
        *self.runs.borrow_mut() = runs.into();
        self
    }
    fn commands(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }
    fn last(&self) -> Vec<String> {
        self.calls.borrow().last().cloned().unwrap()
    }
}

fn ret(r: impl Fn() -> Result<Output> + 'static) -> RunFn {
    Box::new(move |_| r())
}

impl Backend for Fake {
    fn inspect_source(&self, _path: &Path) -> Result<SourceCheck> {
        Ok(SourceCheck {
            playable: self.playable,
            remux_blocker: self.remux_blocker.clone(),
            ..SourceCheck::default()
        })
    }
    fn locate_ffmpeg(&self) -> std::result::Result<(), String> {
        self.locate.clone()
    }
    fn run(&self, args: Vec<OsString>) -> Result<Output> {
        self.calls.borrow_mut().push(
            args.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        );
        if let Some(mut f) = self.runs.borrow_mut().pop_front() {
            return f(&args);
        }
        if let Some(f) = self.default_run.borrow_mut().as_mut() {
            return f(&args);
        }
        ok_output("")
    }
    fn concat_reason(&self, _path: &Path) -> Result<Option<String>> {
        self.concat
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(self.concat_default.clone()))
    }
    fn has_any_video(&self, _path: &Path) -> Result<bool> {
        self.any_video.borrow_mut().pop_front().unwrap_or(Ok(true))
    }
}

struct Ctx {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    source: PathBuf,
    target: PathBuf,
}

fn ctx() -> Ctx {
    let tmp = tempfile::tempdir().unwrap();
    let dir = canonicalize(tmp.path()).unwrap();
    let source = dir.join("source.m2ts");
    std::fs::write(&source, b"original video").unwrap();
    Ctx {
        target: dir.join("output.mp4"),
        source,
        dir,
        _dir: tmp,
    }
}

impl Ctx {
    fn convert(&self, fake: &Fake, overwrite: bool) -> Result<PathBuf> {
        convert_with(fake, &self.source, &self.target, overwrite, None)
    }
    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(&self.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".output-") && n.ends_with(".mp4"))
            .collect()
    }
}

fn write_output() -> RunFn {
    Box::new(|args| {
        std::fs::write(args.last().unwrap(), b"converted").unwrap();
        ok_output("")
    })
}

fn after(args: &[String], flag: &str) -> String {
    let i = args.iter().position(|a| a == flag).unwrap();
    args[i + 1].clone()
}

const X264_AAC: &str = " V....D libx264\n A..... aac\n";

#[test]
fn remux_first_and_preserve_source() {
    let c = ctx();
    let fake = Fake::default().script(vec![write_output()]);
    let mut progress = Vec::new();
    let mut cb = |s: &str| progress.push(s.to_owned());
    let out = convert_with(&fake, &c.source, &c.target, false, Some(&mut cb)).unwrap();
    assert_eq!(out, c.target);
    let command = fake.last();
    assert_eq!(after(&command, "-c"), "copy");
    assert_eq!(after(&command, "-map"), "0:V:0");
    assert!(command.iter().any(|a| a == "0:a?"));
    assert!(command.iter().any(|a| a == "-nostdin"));
    assert_eq!(std::fs::read(&c.target).unwrap(), b"converted");
    assert_eq!(std::fs::read(&c.source).unwrap(), b"original video");
    assert_eq!(progress, ["Remuxing to MP4 without re-encoding"]);
    assert!(c.leftovers().is_empty());
}

#[test]
fn falls_back_to_available_encoder() {
    let c = ctx();
    let fake = Fake::default().script(vec![
        ret(ffmpeg_error),
        ret(|| ok_output(X264_AAC)),
        write_output(),
    ]);
    let mut progress = Vec::new();
    let mut cb = |s: &str| progress.push(s.to_owned());
    convert_with(&fake, &c.source, &c.target, false, Some(&mut cb)).unwrap();
    let commands = fake.commands();
    assert_eq!(commands[1], ["-hide_banner", "-encoders"]);
    assert_eq!(after(&commands[2], "-c:v"), "libx264");
    assert_eq!(after(&commands[2], "-c:a"), "aac");
    assert!(commands[2].iter().any(|a| a == "yuv420p"));
    assert_eq!(after(&commands[2], "-profile:v"), "high");
    assert_eq!(progress.last().unwrap(), "Encoding MP4 with libx264 / AAC");
}

#[test]
fn remuxable_but_concat_incompatible_video_is_normalized() {
    let c = ctx();
    let fake = Fake::default().script(vec![
        ret(|| ok_output("")),
        ret(|| ok_output(X264_AAC)),
        write_output(),
    ]);
    *fake.concat.borrow_mut() =
        vec![Ok(Some("MPEG-2 cannot be concatenated".into())), Ok(None)].into();
    c.convert(&fake, false).unwrap();
    assert!(fake.last().iter().any(|a| a == "libx264"));
}

#[test]
fn source_that_cannot_qualify_is_encoded_without_remux() {
    let c = ctx();
    let fake = Fake {
        remux_blocker: Some("video codec is MPEG-2 video.".into()),
        ..Fake::default()
    }
    .script(vec![ret(|| ok_output(X264_AAC)), write_output()]);
    let mut progress = Vec::new();
    let mut cb = |s: &str| progress.push(s.to_owned());
    convert_with(&fake, &c.source, &c.target, false, Some(&mut cb)).unwrap();
    let commands = fake.commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0], ["-hide_banner", "-encoders"]);
    assert_eq!(after(&commands[1], "-c:v"), "libx264");
    assert!(!commands.iter().any(|c| c.iter().any(|a| a == "copy")));
    assert_eq!(progress, ["Encoding MP4 with libx264 / AAC"]);
    assert_eq!(std::fs::read(&c.target).unwrap(), b"converted");
}

#[test]
fn incompatible_encoded_output_is_not_published() {
    let c = ctx();
    std::fs::write(&c.target, b"existing").unwrap();
    let fake = Fake {
        concat_default: Some("invalid frame rate".into()),
        ..Fake::default()
    }
    .script(vec![
        ret(ffmpeg_error),
        ret(|| ok_output(X264_AAC)),
        write_output(),
    ]);
    let err = c.convert(&fake, true).unwrap_err();
    assert!(
        matches!(&err, Error::Conversion(m) if m.contains("incompatible output")),
        "{err}"
    );
    assert_eq!(std::fs::read(&c.target).unwrap(), b"existing");
    assert!(c.leftovers().is_empty());
}

#[test]
fn incompatible_encoder_output_tries_next_encoder() {
    let c = ctx();
    let fake = Fake::default().script(vec![
        ret(ffmpeg_error),
        ret(|| ok_output(" V....D libx264\n V....D h264_nvenc\n A..... aac\n")),
        write_output(),
        write_output(),
    ]);
    *fake.concat.borrow_mut() = vec![Ok(Some("unsupported profile".into())), Ok(None)].into();
    c.convert(&fake, false).unwrap();
    assert!(fake.last().iter().any(|a| a == "h264_nvenc"));
}

#[test]
fn unusable_encoder_falls_back_to_next_available() {
    let c = ctx();
    let fake = Fake::default().script(vec![
        ret(ffmpeg_error),
        ret(|| ok_output(" V....D h264_nvenc\n V....D h264_qsv\n A..... aac\n")),
        ret(ffmpeg_error),
        write_output(),
    ]);
    c.convert(&fake, false).unwrap();
    let commands = fake.commands();
    assert!(commands[2].iter().any(|a| a == "h264_nvenc"));
    assert!(commands[3].iter().any(|a| a == "h264_qsv"));
    assert_eq!(after(&commands[3], "-b:v"), "5M");
}

#[test]
fn missing_codecs_reports_error_without_output() {
    for encoders in [" V....D libx264\n", " A..... aac\n", ""] {
        let c = ctx();
        let fake = Fake::default().script(vec![
            ret(ffmpeg_error),
            Box::new(move |_| ok_output(encoders)),
        ]);
        let err = c.convert(&fake, false).unwrap_err();
        assert!(
            matches!(&err, Error::Conversion(m) if m.contains("H.264/AAC")),
            "{err}"
        );
        assert!(
            err.to_string()
                .ends_with("Conversion reason: exit code 1: unsupported codec")
        );
        assert!(!c.target.exists());
        assert_eq!(std::fs::read(&c.source).unwrap(), b"original video");
        assert!(c.leftovers().is_empty());
    }
}

#[test]
fn failed_encoding_preserves_existing_destination() {
    let c = ctx();
    std::fs::write(&c.target, b"existing").unwrap();
    let fake = Fake::default().script(vec![
        ret(ffmpeg_error),
        ret(|| ok_output(X264_AAC)),
        ret(ffmpeg_error),
    ]);
    let err = c.convert(&fake, true).unwrap_err();
    assert_eq!(
        err.to_string(),
        "All available H.264 encoders failed:\nlibx264: exit code 1: unsupported codec"
    );
    assert_eq!(std::fs::read(&c.target).unwrap(), b"existing");
    assert!(c.leftovers().is_empty());
}

#[test]
fn encoder_crash_without_stderr_reports_exit_code() {
    let c = ctx();
    let fake = Fake::default().script(vec![
        ret(ffmpeg_error),
        ret(|| ok_output(X264_AAC)),
        ret(|| {
            Err(Error::Ffmpeg {
                code: -11,
                stderr: String::new(),
            })
        }),
    ]);
    let err = c.convert(&fake, false).unwrap_err();
    assert!(err.to_string().contains("libx264: exit code -11"), "{err}");
    assert!(!c.target.exists());
    assert!(c.leftovers().is_empty());
}

#[test]
fn encoder_listing_failure_is_conversion_error() {
    let c = ctx();
    let fake = Fake::default().script(vec![ret(ffmpeg_error), ret(ffmpeg_error)]);
    let err = c.convert(&fake, false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "FFmpeg failed (exit 1): exit code 1: unsupported codec"
    );
}

#[test]
fn existing_destination_requires_approval() {
    let c = ctx();
    std::fs::write(&c.target, b"existing").unwrap();
    let fake = Fake::default();
    assert!(matches!(
        c.convert(&fake, false),
        Err(Error::DestinationExists(_))
    ));
    assert!(fake.commands().is_empty());
    assert_eq!(std::fs::read(&c.target).unwrap(), b"existing");
}

#[test]
fn approved_overwrite_publishes_after_success() {
    let c = ctx();
    std::fs::write(&c.target, b"existing").unwrap();
    let target = c.target.clone();
    let fake = Fake::default().script(vec![Box::new(move |args| {
        assert_eq!(std::fs::read(&target).unwrap(), b"existing");
        std::fs::write(args.last().unwrap(), b"converted").unwrap();
        ok_output("")
    })]);
    c.convert(&fake, true).unwrap();
    assert_eq!(std::fs::read(&c.target).unwrap(), b"converted");
}

#[test]
fn cannot_replace_source_even_with_approval() {
    for alias in [false, true] {
        let c = ctx();
        let target = if alias {
            std::fs::hard_link(&c.source, &c.target).unwrap();
            c.target.clone()
        } else {
            c.source.clone()
        };
        let fake = Fake::default();
        let err = convert_with(&fake, &c.source, &target, true, None).unwrap_err();
        assert!(
            matches!(&err, Error::InvalidInput(m) if m.contains("source file")),
            "{err}"
        );
        assert!(fake.commands().is_empty());
        assert_eq!(std::fs::read(&c.source).unwrap(), b"original video");
    }
}

#[test]
fn rejects_non_mp4_output() {
    let c = ctx();
    let fake = Fake::default();
    let err = convert_with(
        &fake,
        &c.source,
        &c.target.with_extension("avi"),
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "The destination must have an .mp4 extension."
    );
    assert!(fake.commands().is_empty());
}

#[test]
fn rejects_audio_only_input() {
    let c = ctx();
    let fake = Fake {
        playable: false,
        ..Fake::default()
    };
    assert!(matches!(
        c.convert(&fake, false),
        Err(Error::NoVideoStream(_))
    ));
    assert!(fake.commands().is_empty());
    assert!(!c.target.exists());
}

#[test]
fn ffmpeg_missing_reports_actionable_error() {
    let c = ctx();
    let fake = Fake {
        locate: Err("not found".into()),
        ..Fake::default()
    };
    let err = c.convert(&fake, false).unwrap_err();
    assert_eq!(err.to_string(), "FFmpeg is unavailable: not found");
    assert!(fake.commands().is_empty());
}

#[test]
fn invalid_converted_output_is_not_published() {
    let c = ctx();
    let fake = Fake::default();
    *fake.any_video.borrow_mut() = vec![Ok(false)].into();
    let err = c.convert(&fake, false).unwrap_err();
    assert_eq!(err.to_string(), "FFmpeg produced no playable video.");
    assert!(!c.target.exists());
    assert!(c.leftovers().is_empty());
}

#[test]
fn unreadable_converted_output_reports_domain_error() {
    let c = ctx();
    let fake = Fake::default();
    *fake.any_video.borrow_mut() = vec![Err(Error::Other("invalid output".into()))].into();
    let err = c.convert(&fake, false).unwrap_err();
    assert_eq!(err.to_string(), "Invalid converted MP4: invalid output");
    assert!(!c.target.exists());
    assert!(c.leftovers().is_empty());
}

#[test]
fn concurrently_created_destination_is_not_overwritten() {
    let c = ctx();
    let target = c.target.clone();
    let fake = Fake::default().script(vec![Box::new(move |_| {
        std::fs::write(&target, b"other process").unwrap();
        ok_output("")
    })]);
    assert!(matches!(
        c.convert(&fake, false),
        Err(Error::DestinationExists(_))
    ));
    assert_eq!(std::fs::read(&c.target).unwrap(), b"other process");
}

#[test]
fn encoder_parsing_matches_python() {
    let text =
        "Encoders:\n V..... = Video\n ------\n V....D libx264  libx264 H.264\n A....D aac   AAC\n";
    assert_eq!(parse_encoders(text), ["=", "libx264", "aac"]);
}

#[test]
fn default_conversion_path_rules() {
    let c = ctx();
    assert_eq!(default_conversion_path(&c.source), c.dir.join("source.mp4"));
    let mp4 = c.dir.join("clip.mp4");
    assert_eq!(
        default_conversion_path(&mp4),
        c.dir.join("clip-converted.mp4")
    );
    std::fs::write(c.dir.join("source.mp4"), b"x").unwrap();
    assert_eq!(
        default_conversion_path(&c.source),
        c.dir.join("source-converted.mp4")
    );
}
