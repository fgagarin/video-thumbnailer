//! Compare the Rust writer / save_copy / converter with the Python reference
//! results in `tests/golden/` (see its README for the comparison rules).
//!
//! The frame is extracted with ffmpeg (`support::extract_frame`) and videos are
//! loaded with `support::load`, since the loader and frame extractor are ported
//! separately; their pixel output does not affect anything compared here.
//! Writer ffmpeg argv are compared exactly (no wrapper flags are added: the
//! writer calls ffmpeg with precisely the Python argv).

mod common;
mod support;

use std::path::{Path, PathBuf};

use framecover_core::converter::FfmpegVideoConverter;
use framecover_core::save_copy::{default_copy_path, save_copy};
use framecover_core::thumbnail_writer::testing::{RecordedCommand, record_ffmpeg_commands};
use framecover_core::thumbnail_writer::{
    FormatDispatchThumbnailWriter, ThumbnailWriter, concat_ineligibility_reason,
};
use framecover_core::{ApplyError, ApplyResult, VideoFormat};
use serde_json::{Value, json};

const POSITION_MS: u64 = 2000;
const SECOND_POSITION_MS: u64 = 3000;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn load_golden(case: &str) -> Option<(Value, Value)> {
    let dir = golden_dir();
    let summary = std::fs::read_to_string(dir.join(format!("{case}.json"))).ok()?;
    let commands = std::fs::read_to_string(dir.join(format!("{case}.commands.json"))).ok()?;
    Some((
        serde_json::from_str(&summary).unwrap(),
        serde_json::from_str(&commands).unwrap(),
    ))
}

// ---------------------------------------------------------------------------
// Normalisation (mirrors `Normaliser` in scripts/golden/generate_golden.py)
// ---------------------------------------------------------------------------

struct Norm {
    work_forms: Vec<String>,
}

fn strip_verbatim(s: &str) -> String {
    s.strip_prefix(r"\\?\").unwrap_or(s).to_owned()
}

impl Norm {
    fn new(work: &Path) -> Self {
        let mut forms = Vec::new();
        for p in [
            work.to_path_buf(),
            std::fs::canonicalize(work).unwrap_or_else(|_| work.to_path_buf()),
        ] {
            let s = strip_verbatim(&p.to_string_lossy());
            forms.push(s.clone());
            forms.push(s.replace('\\', "/"));
            forms.push(format!(r"\\?\{s}"));
        }
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        forms.dedup();
        Self { work_forms: forms }
    }

    fn apply(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for form in &self.work_forms {
            text = text.replace(form.as_str(), "<WORK>");
        }
        let text = text.replace("<WORK>\\", "<WORK>/");
        random_parts(&text)
    }
}

/// Replace tempfile's 8 random characters after the known prefixes.
fn random_parts(text: &str) -> String {
    let b = text.as_bytes();
    let is_rand = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let is_end = |i: usize| i >= b.len() || matches!(b[i], b'.' | b'\\' | b'/' | b'\'' | b'"');
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let mut prefix_len = None;
        let rest = &text[i..];
        if let Some(after) = rest.strip_prefix(".vt_") {
            let letters = after.bytes().take_while(u8::is_ascii_lowercase).count();
            if letters > 0 && after.as_bytes().get(letters) == Some(&b'_') {
                prefix_len = Some(4 + letters + 1);
            } else {
                prefix_len = Some(4);
            }
        } else {
            for p in [".save-", ".receipt-", ".backup-"] {
                if rest.starts_with(p) {
                    prefix_len = Some(p.len());
                }
            }
            if prefix_len.is_none()
                && rest.starts_with('-')
                && rest.len() >= 13
                && rest.as_bytes()[1..9].iter().all(|&c| is_rand(c))
                && rest[9..].starts_with(".mp4")
            {
                prefix_len = Some(1);
            }
        }
        if let Some(n) = prefix_len {
            let start = i + n;
            if start + 8 <= b.len()
                && b[start..start + 8].iter().all(|&c| is_rand(c))
                && is_end(start + 8)
            {
                out.push_str(&text[i..start]);
                out.push_str("XXXXXXXX");
                i = start + 8;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn commands_json(cmds: &[RecordedCommand], norm: &Norm) -> Vec<Value> {
    cmds.iter()
        .map(|c| {
            let mut argv = vec![Value::from("<FFMPEG>")];
            argv.extend(c.args.iter().map(|a| Value::from(norm.apply(a))));
            let mut entry = json!({"argv": argv, "returncode": c.returncode});
            if let Some(m) = &c.concat_manifest {
                entry["concat_manifest"] = m.iter().map(|l| norm.apply(l)).collect();
            }
            entry
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Summaries (same shape as the Python script's)
// ---------------------------------------------------------------------------

fn ms(v: Option<&Value>) -> Value {
    v.and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| json!((f * 1000.0).round_ties_even() as i64))
        .unwrap_or(Value::Null)
}

fn probe_summary(path: &Path) -> Value {
    let out = support::ffprobe()
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    let raw: Value = serde_json::from_slice(&out.stdout).unwrap();
    let fmt = &raw["format"];
    let streams: Vec<Value> = raw["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            json!({
                "index": s["index"],
                "codec_type": s["codec_type"],
                "codec_name": s["codec_name"],
                "profile": s.get("profile").cloned().unwrap_or(Value::Null),
                "pix_fmt": s.get("pix_fmt").cloned().unwrap_or(Value::Null),
                "width": s.get("width").cloned().unwrap_or(Value::Null),
                "height": s.get("height").cloned().unwrap_or(Value::Null),
                "time_base": s["time_base"],
                "avg_frame_rate": s["avg_frame_rate"],
                "sample_rate": s.get("sample_rate").cloned().unwrap_or(Value::Null),
                "channels": s.get("channels").cloned().unwrap_or(Value::Null),
                "nb_frames": s.get("nb_frames").and_then(Value::as_str)
                    .and_then(|n| n.parse::<i64>().ok()).map_or(Value::Null, Value::from),
                "start_time_ms": ms(s.get("start_time")),
                "duration_ms": ms(s.get("duration")),
                "attached_pic": s["disposition"]["attached_pic"],
                "tags_filename": s["tags"].get("filename").cloned().unwrap_or(Value::Null),
                "tags_mimetype": s["tags"].get("mimetype").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({
        "format_name": fmt["format_name"],
        "duration_ms": ms(fmt.get("duration")),
        "nb_streams": fmt["nb_streams"],
        "major_brand": fmt["tags"].get("major_brand").cloned().unwrap_or(Value::Null),
        "streams": streams,
    })
}

fn mp4_tags_summary(path: &Path) -> Value {
    // Item list only: mp4ameta's default config fails on QuickTime `mp4a` v1.
    let cfg = mp4ameta::ReadConfig {
        read_meta_items: true,
        read_image_data: true,
        ..mp4ameta::ReadConfig::NONE
    };
    let Ok(tag) = mp4ameta::Tag::read_with_path(path, &cfg) else {
        return json!({"error": "unreadable"});
    };
    let freeform = |name: &'static str| {
        tag.strings_of(&mp4ameta::DataIdent::freeform("com.apple.iTunes", name))
            .next()
            .map_or(Value::Null, Value::from)
    };
    let covr: Vec<_> = tag.artworks().collect();
    let covr_info = covr.first().map_or(Value::Null, |img| {
        let decoded = framecover_core::imaging::decode_rgb(img.data).unwrap();
        json!({
            "count": covr.len(),
            "imageformat": match img.fmt { mp4ameta::ImgFmt::Jpeg => 13, mp4ameta::ImgFmt::Png => 14, mp4ameta::ImgFmt::Bmp => 27 },
            "width": decoded.width(),
            "height": decoded.height(),
        })
    });
    let mut keys: Vec<String> = tag
        .data()
        .map(|(ident, _)| ident.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    keys.sort();
    json!({
        "frame_number": freeform("video-thumbnailer-frame-number"),
        "position_ms": freeform("video-thumbnailer-position-ms"),
        "covr": covr_info,
        "keys": keys,
    })
}

fn format_value(f: VideoFormat) -> &'static str {
    match f {
        VideoFormat::Mp4 => "mp4",
        VideoFormat::Mov => "mov",
        VideoFormat::Mkv => "mkv",
        VideoFormat::Avi => "avi",
        VideoFormat::Webm => "webm",
        VideoFormat::Flv => "flv",
        VideoFormat::Unsupported => "unsupported",
    }
}

fn error_value(code: Option<ApplyError>) -> Value {
    code.map_or(Value::Null, |c| {
        Value::from(match c {
            ApplyError::FileNotWritable => "file_not_writable",
            ApplyError::FileLocked => "file_locked",
            ApplyError::UnsupportedFormat => "unsupported_format",
            ApplyError::NoVideoStream => "no_video_stream",
            ApplyError::FfmpegError => "ffmpeg_error",
            ApplyError::DiskFull => "disk_full",
            ApplyError::DestinationExists => "destination_exists",
            ApplyError::Unexpected => "unexpected",
        })
    })
}

fn describe(path: &Path) -> Value {
    let video = support::load(path);
    let thumb = video.existing_thumbnail.as_ref();
    let mut info = json!({
        "file_name": path.file_name().unwrap().to_string_lossy(),
        "probe": probe_summary(path),
        "load": {
            "format": format_value(video.format),
            "duration_ms": video.duration_ms,
            "width": video.width,
            "height": video.height,
            "frame_step_ms": video.frame_step_ms,
            "has_existing_thumbnail": thumb.is_some(),
            "existing_thumbnail_size": thumb.map_or(Value::Null, |t| json!([t.width(), t.height()])),
            "thumbnail_frame_number": video.thumbnail_frame_number,
            "thumbnail_position_ms": video.thumbnail_position_ms,
        },
        "reencode_reason": FormatDispatchThumbnailWriter.reencode_reason(&video),
    });
    let ext = path.extension().unwrap().to_string_lossy().to_lowercase();
    if matches!(ext.as_str(), "mp4" | "mov" | "m4v") {
        info["mp4_tags"] = mp4_tags_summary(path);
    }
    info
}

fn dir_set(dir: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

struct Run {
    norm: Norm,
    commands: Vec<Value>,
}

impl Run {
    fn save(&mut self, path: &Path, position_ms: u64) -> Value {
        let video = support::load(path);
        let frame = support::extract_frame(path, position_ms);
        let destination = default_copy_path(path);
        let before = dir_set(path.parent().unwrap());
        let (result, cmds): (ApplyResult, _) = record_ffmpeg_commands(|| {
            save_copy(
                &FormatDispatchThumbnailWriter,
                &video,
                &frame,
                &destination,
                false,
                Some(position_ms),
                None,
            )
        });
        self.commands.extend(commands_json(&cmds, &self.norm));
        let after = dir_set(path.parent().unwrap());
        let mut new: Vec<String> = after
            .difference(&before)
            .map(|n| self.norm.apply(n))
            .collect();
        new.sort();
        let mut step = json!({
            "extracted_frame_size": [frame.width(), frame.height()],
            "destination": destination.file_name().unwrap().to_string_lossy(),
            "result": {
                "success": result.success,
                "error_code": error_value(result.error_code),
                "error_message": result.error_message.as_deref().map(|m| self.norm.apply(m)),
                "duration_shift_ms": result.duration_shift_ms,
                "output_path": result.output_path.as_deref().map(|p| self.norm.apply(&p.to_string_lossy())),
            },
            "new_files_in_directory": new,
        });
        if result.success && destination.exists() {
            step["output"] = describe(&destination);
            let receipt = framecover_core::save_copy::receipt_path(&destination);
            let data: Value =
                serde_json::from_str(&std::fs::read_to_string(&receipt).unwrap()).unwrap();
            let mut keys: Vec<String> = data.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            step["receipt"] = json!({
                "file_name": receipt.file_name().unwrap().to_string_lossy(),
                "keys": keys,
                "version": data["version"],
            });
        }
        step
    }

    fn convert(&mut self, path: &Path) -> Value {
        let destination = path.with_file_name(format!(
            "{}-converted.mp4",
            path.file_stem().unwrap().to_string_lossy()
        ));
        let mut stages = Vec::new();
        let (outcome, cmds) = record_ffmpeg_commands(|| {
            let mut cb = |s: &str| stages.push(s.to_owned());
            FfmpegVideoConverter.convert(path, &destination, false, Some(&mut cb))
        });
        self.commands.extend(commands_json(&cmds, &self.norm));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut new: Vec<String> = dir_set(path.parent().unwrap())
            .into_iter()
            .filter(|n| *n != name)
            .map(|n| self.norm.apply(&n))
            .collect();
        new.sort();
        let mut step = json!({
            "returned_path": outcome.as_ref().ok().map(|p| self.norm.apply(&p.to_string_lossy())),
            "error": outcome.as_ref().err().map(|e| e.to_string()),
            "progress_stages": stages,
            "new_files_in_directory": new,
        });
        if outcome.is_ok() {
            step["output"] = describe(&destination);
            step["concat_ineligibility_reason"] =
                concat_ineligibility_reason(&destination).unwrap().into();
        }
        step
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

struct Diff {
    case: String,
    problems: Vec<String>,
}

impl Diff {
    fn eq(&mut self, what: &str, expected: &Value, actual: &Value) {
        if expected != actual {
            self.problems
                .push(format!("{what}: expected {expected}, got {actual}"));
        }
    }
    fn near(&mut self, what: &str, expected: &Value, actual: &Value, tol: i64) {
        match (expected.as_i64(), actual.as_i64()) {
            (Some(e), Some(a)) if (e - a).abs() <= tol => {}
            (None, None) => {}
            _ => self
                .problems
                .push(format!("{what}: expected {expected} ±{tol}, got {actual}")),
        }
    }
}

fn compare_description(d: &mut Diff, at: &str, expected: &Value, actual: &Value) {
    d.eq(
        &format!("{at}.file_name"),
        &expected["file_name"],
        &actual["file_name"],
    );
    d.eq(
        &format!("{at}.reencode_reason"),
        &expected["reencode_reason"],
        &actual["reencode_reason"],
    );
    let (el, al) = (&expected["load"], &actual["load"]);
    for key in [
        "format",
        "width",
        "height",
        "frame_step_ms",
        "has_existing_thumbnail",
        "existing_thumbnail_size",
        "thumbnail_frame_number",
        "thumbnail_position_ms",
    ] {
        d.eq(&format!("{at}.load.{key}"), &el[key], &al[key]);
    }
    d.near(
        &format!("{at}.load.duration_ms"),
        &el["duration_ms"],
        &al["duration_ms"],
        40,
    );
    let (ep, ap) = (&expected["probe"], &actual["probe"]);
    for key in ["format_name", "nb_streams", "major_brand"] {
        d.eq(&format!("{at}.probe.{key}"), &ep[key], &ap[key]);
    }
    d.near(
        &format!("{at}.probe.duration_ms"),
        &ep["duration_ms"],
        &ap["duration_ms"],
        40,
    );
    let empty = vec![];
    let es = ep["streams"].as_array().unwrap_or(&empty);
    let as_ = ap["streams"].as_array().unwrap_or(&empty);
    d.eq(
        &format!("{at}.probe.streams.len"),
        &es.len().into(),
        &as_.len().into(),
    );
    let mp4ish = matches!(el["format"].as_str(), Some("mp4" | "mov"));
    for (i, (e, a)) in es.iter().zip(as_).enumerate() {
        for key in [
            "codec_type",
            "codec_name",
            "profile",
            "pix_fmt",
            "width",
            "height",
            "time_base",
            "attached_pic",
            "tags_filename",
            "tags_mimetype",
            "sample_rate",
            "channels",
        ] {
            d.eq(&format!("{at}.probe.streams[{i}].{key}"), &e[key], &a[key]);
        }
        if mp4ish && e["codec_type"] == "video" {
            d.eq(
                &format!("{at}.probe.streams[{i}].nb_frames"),
                &e["nb_frames"],
                &a["nb_frames"],
            );
        }
        d.near(
            &format!("{at}.probe.streams[{i}].duration_ms"),
            &e["duration_ms"],
            &a["duration_ms"],
            40,
        );
        d.near(
            &format!("{at}.probe.streams[{i}].start_time_ms"),
            &e["start_time_ms"],
            &a["start_time_ms"],
            40,
        );
    }
    if !expected["mp4_tags"].is_null() {
        let (et, at_) = (&expected["mp4_tags"], &actual["mp4_tags"]);
        for key in ["frame_number", "position_ms", "keys"] {
            d.eq(&format!("{at}.mp4_tags.{key}"), &et[key], &at_[key]);
        }
        let covr = |v: &Value| {
            if v.is_null() {
                Value::Null
            } else {
                json!([v["count"], v["imageformat"], v["width"], v["height"]])
            }
        };
        d.eq(
            &format!("{at}.mp4_tags.covr"),
            &covr(&et["covr"]),
            &covr(&at_["covr"]),
        );
    }
}

fn compare_save(d: &mut Diff, at: &str, expected: &Value, actual: &Value) {
    d.eq(
        &format!("{at}.destination"),
        &expected["destination"],
        &actual["destination"],
    );
    d.eq(
        &format!("{at}.extracted_frame_size"),
        &expected["extracted_frame_size"],
        &actual["extracted_frame_size"],
    );
    let (er, ar) = (&expected["result"], &actual["result"]);
    for key in ["success", "error_code", "duration_shift_ms", "output_path"] {
        d.eq(&format!("{at}.result.{key}"), &er[key], &ar[key]);
    }
    if er["success"] == true {
        d.eq(
            &format!("{at}.result.error_message"),
            &er["error_message"],
            &ar["error_message"],
        );
    }
    d.eq(
        &format!("{at}.new_files_in_directory"),
        &expected["new_files_in_directory"],
        &actual["new_files_in_directory"],
    );
    if !expected["output"].is_null() {
        compare_description(
            d,
            &format!("{at}.output"),
            &expected["output"],
            &actual["output"],
        );
        d.eq(
            &format!("{at}.receipt"),
            &expected["receipt"],
            &actual["receipt"],
        );
    }
}

fn compare_commands(d: &mut Diff, expected: &[Value], actual: &[Value]) {
    d.eq("commands.len", &expected.len().into(), &actual.len().into());
    for (i, (e, a)) in expected.iter().zip(actual).enumerate() {
        d.eq(&format!("commands[{i}].argv"), &e["argv"], &a["argv"]);
        d.eq(
            &format!("commands[{i}].returncode"),
            &e["returncode"],
            &a["returncode"],
        );
        let em = e.get("concat_manifest").cloned().unwrap_or(Value::Null);
        let am = a.get("concat_manifest").cloned().unwrap_or(Value::Null);
        d.eq(&format!("commands[{i}].concat_manifest"), &em, &am);
    }
}

// ---------------------------------------------------------------------------
// Intentional deviations from the Python reference
// ---------------------------------------------------------------------------
//
// Bugs found in the Python version (golden README, "Behaviour worth knowing")
// were fixed after the port reached parity. The golden files stay as
// generated by Python; each fix below patches exactly the expected values it
// changes, so everything else is still compared against Python.

/// Set the expected value at a JSON pointer (it must exist in the golden).
fn set(golden: &mut Value, pointer: &str, value: Value) {
    *golden
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("golden has no {pointer}")) = value;
}

fn argv(cmds: &mut [Value], i: usize) -> &mut Vec<Value> {
    cmds[i]["argv"].as_array_mut().unwrap()
}

/// Insert `args` before the first occurrence of `anchor` in command `i`.
fn insert_before(cmds: &mut [Value], i: usize, anchor: &str, args: &[&str]) {
    let argv = argv(cmds, i);
    let at = argv
        .iter()
        .position(|a| a == anchor)
        .unwrap_or_else(|| panic!("command {i} has no {anchor}"));
    for (k, a) in args.iter().enumerate() {
        argv.insert(at + k, Value::from(*a));
    }
}

/// Replace the argument following `option` in command `i`.
fn set_option(cmds: &mut [Value], i: usize, option: &str, value: &str) {
    let argv = argv(cmds, i);
    let at = argv
        .iter()
        .position(|a| a == option)
        .unwrap_or_else(|| panic!("command {i} has no {option}"));
    argv[at + 1] = Value::from(value);
}

/// Fix 1, concat DTS repair: x264's B-frame delay makes the source start two
/// frames before the preview's DTS; the concat output now rewrites the first
/// two DTS (`setts`) instead of letting the muxer squash them.
const SETTS_B_FRAMES: &str = "setts=pts=PTS:dts='if(lt(N,2),STARTPTS+(0)-(2-N)*512,DTS)'";

fn apply_intentional_deviations(case: &str, golden: &mut Value, cmds: &mut Vec<Value>) {
    // Fix 1 (README behaviour 1, "frame rate drift"): concat outputs keep the
    // source's 25 fps, so the reloaded step is 40 ms (not 39) and the video
    // track is 5040 ms (125 frames + the preview, not 4960). On a re-save the
    // preview is encoded at 25/1 (not 1575/62), the manifest's duration is
    // 0.04 and the saved frame number is round(3000/40) = 75 (not 77).
    let concat_output = |golden: &mut Value, at: &str| {
        set(
            golden,
            &format!("{at}/output/load/frame_step_ms"),
            json!(40),
        );
        set(
            golden,
            &format!("{at}/output/load/duration_ms"),
            json!(5040),
        );
        set(
            golden,
            &format!("{at}/output/probe/streams/0/duration_ms"),
            json!(5040),
        );
    };
    match case {
        "save_mp4" => {
            concat_output(golden, "/save");
            insert_before(cmds, 1, "-disposition:v:1", &["-bsf:v:0", SETTS_B_FRAMES]);
        }
        "resave_mp4" => {
            concat_output(golden, "/first_save");
            concat_output(golden, "/second_save");
            set(
                golden,
                "/second_save/output/load/thumbnail_frame_number",
                json!(75),
            );
            set(
                golden,
                "/second_save/output/mp4_tags/frame_number",
                json!("75"),
            );
            insert_before(cmds, 1, "-disposition:v:1", &["-bsf:v:0", SETTS_B_FRAMES]);
            set_option(cmds, 2, "-framerate", "25/1");
            insert_before(cmds, 4, "-disposition:v:1", &["-bsf:v:0", SETTS_B_FRAMES]);
            set(golden_cmd_manifest(cmds, 4), "/1", json!("duration 0.04"));
        }
        _ => {}
    }

    // Fix 2 (README behaviour 4, default stream selection): MKV and AVI
    // remuxes map every input stream (`-map 0`), so extra audio, subtitles and
    // non-cover attachments survive. On MKV the previous cover-named
    // attachment is excluded explicitly (`-map -0:<i>`) instead of being lost
    // by accident, and the new attachment's metadata targets its own output
    // stream (`-metadata:s:<n>`) rather than every attachment (`s:t`).
    let mkv_attach = |cmds: &mut [Value], i: usize, removed: &[&str], out_index: usize| {
        let mut maps = vec!["-map", "0"];
        for r in removed {
            maps.extend(["-map", r]);
        }
        insert_before(cmds, i, "-attach", &maps);
        let spec = format!("-metadata:s:{out_index}");
        for a in argv(cmds, i).iter_mut().filter(|a| *a == "-metadata:s:t") {
            *a = Value::from(spec.as_str());
        }
    };
    match case {
        "save_mkv" => mkv_attach(cmds, 0, &[], 2),
        "resave_mkv" => {
            mkv_attach(cmds, 0, &[], 2);
            // The first save's cover.jpg is input stream 2.
            mkv_attach(cmds, 1, &["-0:2"], 2);
        }
        "save_mkv_two_audio" => {
            mkv_attach(cmds, 0, &[], 3);
            // Both audio tracks are kept: video, audio, audio, cover.
            let streams = golden
                .pointer_mut("/save/output/probe/streams")
                .unwrap()
                .as_array_mut()
                .unwrap();
            let second_audio = streams[1].clone();
            streams.insert(2, second_audio);
            for (i, s) in streams.iter_mut().enumerate() {
                s["index"] = json!(i);
            }
            set(golden, "/save/output/probe/nb_streams", json!(4));
        }
        "save_avi" => insert_before(cmds, 0, "-c", &["-map", "0"]),
        _ => {}
    }

    // Fix 3 (README behaviour 5): the informational message of containers
    // without cover art no longer claims a Linux XDG cache update (wrong on
    // Windows/macOS and for saved copies); see `no_cover_art_note`.
    let note = match case {
        "save_webm" => Some(VideoFormat::Webm),
        "save_flv" => Some(VideoFormat::Flv),
        "save_avi" => Some(VideoFormat::Avi),
        _ => None,
    };
    if let Some(format) = note {
        set(
            golden,
            "/save/result/error_message",
            json!(framecover_core::thumbnail_writer::no_cover_art_note(format)),
        );
    }

    // Fix 4 (README behaviour 8): the converter decides from the source probe
    // that MPEG-2 and WMV2 video can never be concat-compatible after a stream
    // copy, so it skips the remux (its command and progress stage) and
    // encodes directly. TS (H.264) still remuxes.
    if matches!(case, "convert_mpg" | "convert_wmv") {
        set(
            golden,
            "/convert/progress_stages",
            json!(["Encoding MP4 with libx264 / AAC"]),
        );
        // The remux was the first command.
        cmds.remove(0);
    }

    // Fix 5 (README behaviour 2): the full re-encode inserts the poster as an
    // extra first frame (a `concat` filter of a one-frame poster segment and
    // the source) instead of painting it over frame 0. Like the concat path,
    // a first save shifts everything by one frame (shift 40 ms, tags
    // 2040 ms / frame 51, 126 frames) with audio/subtitles taken from a second
    // `-itsoffset 0.04` input; a re-save trims the previous preview frame.
    let first_reencode = |golden: &mut Value, cmds: &mut [Value], i: usize, at: &str| {
        reencode_inserts_preview(cmds, i, false);
        set(golden, &format!("{at}/result/duration_shift_ms"), json!(40));
        set(
            golden,
            &format!("{at}/output/load/thumbnail_frame_number"),
            json!(51),
        );
        set(
            golden,
            &format!("{at}/output/load/thumbnail_position_ms"),
            json!(2040),
        );
        set(
            golden,
            &format!("{at}/output/probe/streams/0/nb_frames"),
            json!(126),
        );
        set(
            golden,
            &format!("{at}/output/mp4_tags/frame_number"),
            json!("51"),
        );
        set(
            golden,
            &format!("{at}/output/mp4_tags/position_ms"),
            json!("2040"),
        );
    };
    match case {
        "save_mov" | "save_mp4_with_thumb" | "save_mp4_yuv444p" => {
            first_reencode(golden, cmds, 0, "/save");
        }
        "resave_mov" => {
            first_reencode(golden, cmds, 0, "/first_save");
            reencode_inserts_preview(cmds, 1, true);
            set(
                golden,
                "/second_save/output/probe/streams/0/nb_frames",
                json!(126),
            );
        }
        _ => {}
    }

    // Fix 6 (README behaviour 3): MOV takes the concat fast path too and
    // stays QuickTime (`-f mov`, brand `qt  `, no `©too` item since the mov
    // muxer does not write one), with the cover written as a `covr` item
    // (the mov muxer drops attached_pic streams). Its commands are those of
    // the same-shaped MP4 case (with the fixes above) on `.mov` paths, the
    // final mux being `-f mov`; no `reencode_reason` any more.
    let mov_like = match case {
        "save_mov" => Some(("save_mp4", vec!["/save"])),
        "resave_mov" => Some(("resave_mp4", vec!["/first_save", "/second_save"])),
        _ => None,
    };
    if let Some((mp4_case, saves)) = mov_like {
        set(golden, "/fixture/reencode_reason", Value::Null);
        for at in saves {
            set(golden, &format!("{at}/output/reencode_reason"), Value::Null);
            set(
                golden,
                &format!("{at}/output/probe/major_brand"),
                json!("qt  "),
            );
            let keys = golden
                .pointer_mut(&format!("{at}/output/mp4_tags/keys"))
                .unwrap()
                .as_array_mut()
                .unwrap();
            keys.retain(|k| k != "©too");
        }
        let (mut mp4_golden, mp4_cmds) = load_golden(mp4_case).unwrap();
        let mut mp4_cmds: Vec<Value> = mp4_cmds["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c.get("marker").is_none())
            .cloned()
            .collect();
        apply_intentional_deviations(mp4_case, &mut mp4_golden, &mut mp4_cmds);
        let as_mov = |s: &str| {
            s.replace(
                ".sample-thumbnail.save-XXXXXXXX.mp4",
                ".sample-thumbnail.save-XXXXXXXX.mov",
            )
            .replace(
                ".sample-thumbnail-thumbnail.save-XXXXXXXX.mp4",
                ".sample-thumbnail-thumbnail.save-XXXXXXXX.mov",
            )
        };
        for cmd in &mut mp4_cmds {
            let is_concat = cmd.get("concat_manifest").is_some();
            let argv = cmd["argv"].as_array_mut().unwrap();
            for a in argv.iter_mut() {
                *a = json!(as_mov(a.as_str().unwrap()));
            }
            if is_concat {
                let n = argv.len();
                assert_eq!(argv[n - 3], "-f");
                argv[n - 2] = json!("mov");
            }
            if let Some(m) = cmd.get_mut("concat_manifest") {
                for line in m.as_array_mut().unwrap() {
                    *line = json!(as_mov(line.as_str().unwrap()));
                }
            }
        }
        *cmds = mp4_cmds;
    }

    // Fix 7: an inserted preview also carries the `video-thumbnailer-preview`
    // marker item (`frame` here), so previews made from an image, which have
    // no position tags, are still recognised and replaced on re-save.
    for at in ["/save", "/first_save", "/second_save"] {
        let Some(keys) = golden
            .pointer_mut(&format!("{at}/output/mp4_tags/keys"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        if keys
            .iter()
            .any(|k| k == "----:com.apple.iTunes:video-thumbnailer-position-ms")
        {
            keys.push(json!("----:com.apple.iTunes:video-thumbnailer-preview"));
            keys.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
    }

    // Fix 8 (HDR / 10-bit / HEVC fast path): HEVC Main is concat-eligible.
    // The preview frame is encoded with libx265 (same profile, `hvc1` tag,
    // the source's colour range and chroma siting) and the source is stream
    // copied like `save_mp4` (DTS repair for x265's reorder delay; the
    // source's `hvc1` sample entry is restored afterwards, which runs no
    // ffmpeg command). Output: still HEVC Main, one frame and 40 ms longer,
    // tags 2040 ms / frame 51; no `reencode_reason`.
    if case == "save_mp4_hevc" {
        set(golden, "/fixture/reencode_reason", Value::Null);
        set(
            golden,
            "/save/output/probe/streams/0/codec_name",
            json!("hevc"),
        );
        set(
            golden,
            "/save/output/probe/streams/0/profile",
            json!("Main"),
        );
        set(golden, "/save/output/probe/streams/0/nb_frames", json!(126));
        set(
            golden,
            "/save/output/probe/streams/0/duration_ms",
            json!(5040),
        );
        set(golden, "/save/output/load/duration_ms", json!(5040));
        set(golden, "/save/result/duration_shift_ms", json!(40));
        set(
            golden,
            "/save/output/load/thumbnail_frame_number",
            json!(51),
        );
        set(
            golden,
            "/save/output/load/thumbnail_position_ms",
            json!(2040),
        );
        set(golden, "/save/output/mp4_tags/frame_number", json!("51"));
        set(golden, "/save/output/mp4_tags/position_ms", json!("2040"));
        let (_, mp4_cmds) = load_golden("save_mp4").unwrap();
        let mut hevc_cmds: Vec<Value> = mp4_cmds["commands"].as_array().unwrap().clone();
        let rename = |s: &str| s.replace(".sample-thumbnail.save-", ".hevc-thumbnail.save-");
        for cmd in &mut hevc_cmds {
            for a in cmd["argv"].as_array_mut().unwrap().iter_mut() {
                *a = json!(rename(a.as_str().unwrap()));
            }
            if let Some(m) = cmd.get_mut("concat_manifest") {
                for line in m.as_array_mut().unwrap() {
                    *line = json!(rename(line.as_str().unwrap()));
                }
            }
        }
        set_option(
            &mut hevc_cmds,
            0,
            "-vf",
            "scale=320:240:flags=lanczos,format=yuv420p",
        );
        set_option(&mut hevc_cmds, 0, "-c:v", "libx265");
        set_option(&mut hevc_cmds, 0, "-profile:v", "main");
        insert_before(
            &mut hevc_cmds,
            0,
            "-video_track_timescale",
            &[
                "-tag:v",
                "hvc1",
                "-color_range",
                "tv",
                "-chroma_sample_location",
                "left",
                "-x265-params",
                "log-level=error",
            ],
        );
        insert_before(
            &mut hevc_cmds,
            1,
            "-disposition:v:1",
            &["-bsf:v:0", SETTS_X265],
        );
        *cmds = hevc_cmds;
    }
}

/// Fix 8: x265's default reorder delay as repaired on the concat output.
const SETTS_X265: &str = "setts=pts=PTS:dts='if(lt(N,2),STARTPTS+(0)-(2-N)*512,DTS)'";

/// Fix 5: rewrite the Python overlay re-encode (command `i`) into the
/// preview-inserting one for the 320x240 25 fps fixtures.
fn reencode_inserts_preview(cmds: &mut [Value], i: usize, replace: bool) {
    let source = argv(cmds, i)[3].clone();
    let trim = if replace { "trim=start_frame=1," } else { "" };
    let graph = format!(
        "[1:v]scale=320:240:flags=lanczos,setsar=1/1,trim=end_frame=1[poster];\
         [0:0]{trim}setpts=PTS-STARTPTS,setsar=1/1[base];\
         [poster][base]concat=n=2:v=1:a=0[v]"
    );
    set_option(cmds, i, "-filter_complex", &graph);
    // Frame 1 (the first source frame) is a keyframe, so the preview can
    // later be stripped by stream copy.
    insert_before(
        cmds,
        i,
        "-c:v:1",
        &["-force_key_frames:v:0", "expr:lte(n,1)"],
    );
    let a = argv(cmds, i);
    let loop_at = a.iter().position(|v| v == "-loop").unwrap();
    a.insert(loop_at + 2, json!("-framerate"));
    a.insert(loop_at + 3, json!("25/1"));
    if !replace {
        let at = a.iter().position(|v| v == "-filter_complex").unwrap();
        for (k, v) in [json!("-itsoffset"), json!("0.04"), json!("-i"), source]
            .into_iter()
            .enumerate()
        {
            a.insert(at + k, v);
        }
        for v in a.iter_mut() {
            if v == "0:a?" {
                *v = json!("3:a?");
            } else if v == "0:s?" {
                *v = json!("3:s?");
            }
        }
    }
}

fn golden_cmd_manifest(cmds: &mut [Value], i: usize) -> &mut Value {
    &mut cmds[i]["concat_manifest"]
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

enum Kind {
    Save,
    Resave,
    Convert,
}

fn run_case(case: &str, kind: Kind, make: impl FnOnce(&Path) -> PathBuf) {
    let Some((mut golden, golden_cmds)) = load_golden(case) else {
        eprintln!("golden data for {case} missing; skipping");
        return;
    };
    let mut golden_cmds: Vec<Value> = golden_cmds["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("marker").is_none())
        .cloned()
        .collect();
    apply_intentional_deviations(case, &mut golden, &mut golden_cmds);
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path();
    let src = make(work);
    let mut run = Run {
        norm: Norm::new(work),
        commands: Vec::new(),
    };
    let mut d = Diff {
        case: case.to_owned(),
        problems: Vec::new(),
    };
    match kind {
        Kind::Save => {
            compare_description(&mut d, "fixture", &golden["fixture"], &describe(&src));
            let save = run.save(&src, POSITION_MS);
            compare_save(&mut d, "save", &golden["save"], &save);
        }
        Kind::Resave => {
            compare_description(&mut d, "fixture", &golden["fixture"], &describe(&src));
            let first = run.save(&src, POSITION_MS);
            compare_save(&mut d, "first_save", &golden["first_save"], &first);
            let copy = default_copy_path(&src);
            let second = run.save(&copy, SECOND_POSITION_MS);
            compare_save(&mut d, "second_save", &golden["second_save"], &second);
        }
        Kind::Convert => {
            let convert = run.convert(&src);
            let (e, a) = (&golden["convert"], &convert);
            for key in [
                "returned_path",
                "error",
                "progress_stages",
                "new_files_in_directory",
                "concat_ineligibility_reason",
            ] {
                d.eq(&format!("convert.{key}"), &e[key], &a[key]);
            }
            if !e["output"].is_null() {
                compare_description(&mut d, "convert.output", &e["output"], &a["output"]);
            }
        }
    }
    compare_commands(&mut d, &golden_cmds, &run.commands);
    assert!(
        d.problems.is_empty(),
        "golden mismatches for {}:\n  {}",
        d.case,
        d.problems.join("\n  ")
    );
}

fn for_format(ext: &'static str) -> impl FnOnce(&Path) -> PathBuf {
    move |dir| common::video_for_format(dir, ext)
}

fn custom(
    name: &'static str,
    extra: &'static [&'static str],
    w: u32,
    h: u32,
) -> impl FnOnce(&Path) -> PathBuf {
    move |dir| {
        let out = dir.join(name);
        common::generate_video(&out, 5, w, h, extra);
        out
    }
}

#[test]
fn save_mp4() {
    run_case("save_mp4", Kind::Save, for_format("mp4"));
}

#[test]
fn save_mov() {
    run_case("save_mov", Kind::Save, for_format("mov"));
}

#[test]
fn save_mkv() {
    run_case("save_mkv", Kind::Save, for_format("mkv"));
}

#[test]
fn save_avi() {
    run_case("save_avi", Kind::Save, for_format("avi"));
}

#[test]
fn save_flv() {
    run_case("save_flv", Kind::Save, for_format("flv"));
}

#[test]
fn save_webm() {
    run_case("save_webm", Kind::Save, for_format("webm"));
}

#[test]
fn save_mp4_with_thumb() {
    run_case("save_mp4_with_thumb", Kind::Save, |dir| {
        let out = common::video_with_thumb(dir);
        std::fs::remove_file(dir.join("base.mp4")).unwrap();
        std::fs::remove_file(dir.join("cover.jpg")).unwrap();
        out
    });
}

#[test]
fn resave_mp4() {
    run_case("resave_mp4", Kind::Resave, for_format("mp4"));
}

#[test]
fn resave_mov() {
    run_case("resave_mov", Kind::Resave, for_format("mov"));
}

#[test]
fn resave_mkv() {
    run_case("resave_mkv", Kind::Resave, for_format("mkv"));
}

#[test]
fn save_mkv_two_audio() {
    run_case("save_mkv_two_audio", Kind::Save, |dir| {
        let base = dir.join("base.mkv");
        common::generate_video(&base, 5, 320, 240, common::H264_AAC);
        let src = dir.join("two_audio.mkv");
        support::run_ffmpeg(&[
            "-y",
            "-i",
            base.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-map",
            "0:a",
            "-c",
            "copy",
            src.to_str().unwrap(),
        ]);
        std::fs::remove_file(&base).unwrap();
        src
    });
}

#[test]
fn save_mp4_yuv444p() {
    run_case(
        "save_mp4_yuv444p",
        Kind::Save,
        custom(
            "yuv444p.mp4",
            &["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv444p"],
            320,
            240,
        ),
    );
}

#[test]
fn save_mp4_hevc() {
    if !support::has_encoder("libx265") {
        eprintln!("libx265 unavailable; skipping");
        return;
    }
    run_case(
        "save_mp4_hevc",
        Kind::Save,
        custom(
            "hevc.mp4",
            &[
                "-c:v", "libx265", "-c:a", "aac", "-pix_fmt", "yuv420p", "-tag:v", "hvc1",
            ],
            320,
            240,
        ),
    );
}

#[test]
fn save_mp4_av1() {
    if !support::has_encoder("libaom-av1") {
        eprintln!("libaom-av1 unavailable; skipping");
        return;
    }
    run_case(
        "save_mp4_av1",
        Kind::Save,
        custom(
            "av1.mp4",
            &[
                "-c:v",
                "libaom-av1",
                "-cpu-used",
                "8",
                "-row-mt",
                "1",
                "-crf",
                "35",
                "-b:v",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
            ],
            160,
            120,
        ),
    );
}

#[test]
fn convert_ts() {
    run_case(
        "convert_ts",
        Kind::Convert,
        custom("sample.ts", common::H264_AAC, 320, 240),
    );
}

#[test]
fn convert_mpg() {
    run_case(
        "convert_mpg",
        Kind::Convert,
        custom(
            "sample.mpg",
            &["-c:v", "mpeg2video", "-c:a", "mp2"],
            320,
            240,
        ),
    );
}

#[test]
fn convert_wmv() {
    run_case(
        "convert_wmv",
        Kind::Convert,
        custom("sample.wmv", &["-c:v", "wmv2", "-c:a", "wmav2"], 320, 240),
    );
}

#[test]
fn normaliser_matches_python_placeholders() {
    assert_eq!(
        random_parts("a/.vt_cover_Ab3_9xYz.jpg"),
        "a/.vt_cover_XXXXXXXX.jpg"
    );
    assert_eq!(random_parts("a/.vt_Ab3_9xYz.tmp"), "a/.vt_XXXXXXXX.tmp");
    assert_eq!(
        random_parts("a/.s-thumbnail.save-Ab3_9xYz.mp4"),
        "a/.s-thumbnail.save-XXXXXXXX.mp4"
    );
    assert_eq!(
        random_parts("a/.sample-converted-Ab3_9xYz.mp4"),
        "a/.sample-converted-XXXXXXXX.mp4"
    );
    assert_eq!(random_parts("sample-thumbnail.mp4"), "sample-thumbnail.mp4");
}
