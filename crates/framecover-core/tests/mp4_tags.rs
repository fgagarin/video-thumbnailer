//! MP4 freeform/covr tag round-trips and atom preservation.

mod common;
mod support;

use std::path::Path;

use framecover_core::mp4_tags::{
    PREVIEW_FROM_FRAME, PREVIEW_FROM_IMAGE, read_preview_source, read_thumbnail_source, write_covr,
    write_thumbnail_source,
};
use mp4ameta::{DataIdent, Tag};

fn encoder_tag(path: &Path) -> Option<String> {
    Tag::read_from_path(path)
        .ok()?
        .strings_of(&mp4ameta::ident::ENCODER)
        .next()
        .map(str::to_owned)
}

fn check_roundtrip(path: &Path) {
    let packets_before = support::video_packets(path);
    let audio_before = support::audio_packets(path);
    let encoder_before = encoder_tag(path);
    assert!(encoder_before.is_some(), "ffmpeg writes ©too");
    assert_eq!(read_thumbnail_source(path), (None, None));

    write_thumbnail_source(path, 42, 1680).unwrap();
    assert_eq!(read_thumbnail_source(path), (Some(42), Some(1680)));

    let jpeg = framecover_core::imaging::cover_jpeg(&common::sample_image()).unwrap();
    write_covr(path, &jpeg).unwrap();
    // Overwrite with new values: still one item each.
    write_thumbnail_source(path, 7, 280).unwrap();
    assert_eq!(read_thumbnail_source(path), (Some(7), Some(280)));

    let tag = Tag::read_from_path(path).unwrap();
    let artworks: Vec<_> = tag.artworks().collect();
    assert_eq!(artworks.len(), 1);
    assert_eq!(artworks[0].fmt, mp4ameta::ImgFmt::Jpeg);
    assert_eq!(artworks[0].data, jpeg.as_slice());
    let ident = DataIdent::freeform("com.apple.iTunes", "video-thumbnailer-frame-number");
    let data: Vec<_> = tag.data_of(&ident).collect();
    assert_eq!(data, vec![&mp4ameta::Data::Utf8("7".into())]);

    // Other atoms survive and media is untouched (chunk offsets fixed up).
    assert_eq!(encoder_tag(path), encoder_before);
    assert_eq!(support::video_packets(path), packets_before);
    assert_eq!(support::audio_packets(path), audio_before);
}

#[test]
fn roundtrip_preserves_media_with_moov_at_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    check_roundtrip(&path);
}

#[test]
fn roundtrip_preserves_media_with_faststart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast.mp4");
    let mut extra = common::H264_AAC.to_vec();
    extra.extend(["-movflags", "+faststart"]);
    common::generate_video(&path, 2, 160, 120, &extra);
    check_roundtrip(&path);
}

#[test]
fn preview_marker_tracks_image_and_frame_covers() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    assert_eq!(read_preview_source(&path), None);
    let jpeg = framecover_core::imaging::cover_jpeg(&common::sample_image()).unwrap();

    // A frame preview: cover, then its position.
    write_covr(&path, &jpeg).unwrap();
    write_thumbnail_source(&path, 7, 280).unwrap();
    assert_eq!(
        read_preview_source(&path).as_deref(),
        Some(PREVIEW_FROM_FRAME)
    );
    assert_eq!(read_thumbnail_source(&path), (Some(7), Some(280)));

    // Replaced by an image preview: marked, and the old position is gone so
    // it is not restored on the next load.
    write_covr(&path, &jpeg).unwrap();
    assert_eq!(
        read_preview_source(&path).as_deref(),
        Some(PREVIEW_FROM_IMAGE)
    );
    assert_eq!(read_thumbnail_source(&path), (None, None));
    assert_eq!(read_preview_source(&dir.path().join("missing.mp4")), None);
}

#[test]
fn quicktime_mov_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_for_format(dir.path(), "mov");
    // mp4ameta's *default* ReadConfig fails on QuickTime `mp4a` v1 sample
    // descriptions; mp4_tags only reads the item list, which works.
    assert!(Tag::read_from_path(&path).is_err());
    assert_eq!(read_thumbnail_source(&path), (None, None));
    let packets = support::video_packets(&path);
    write_thumbnail_source(&path, 3, 120).unwrap();
    assert_eq!(read_thumbnail_source(&path), (Some(3), Some(120)));
    assert_eq!(support::video_packets(&path), packets);
}

#[test]
fn unreadable_file_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("junk.mp4");
    std::fs::write(&path, b"not an mp4").unwrap();
    assert_eq!(read_thumbnail_source(&path), (None, None));
    assert_eq!(
        read_thumbnail_source(&dir.path().join("missing.mp4")),
        (None, None)
    );
    assert!(write_thumbnail_source(&path, 1, 2).is_err());
}

#[test]
fn invalid_values_read_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let mut tag = Tag::read_from_path(&path).unwrap();
    tag.set_data(
        DataIdent::freeform("com.apple.iTunes", "video-thumbnailer-frame-number"),
        mp4ameta::Data::Utf8("-3".into()),
    );
    tag.set_data(
        DataIdent::freeform("com.apple.iTunes", "video-thumbnailer-position-ms"),
        mp4ameta::Data::Reserved(b"120".to_vec()),
    );
    tag.write_to_path(&path).unwrap();
    assert_eq!(read_thumbnail_source(&path), (None, Some(120)));
}

fn run_python(python: &std::ffi::OsStr, script: &str, args: &[&Path]) {
    let out = std::process::Command::new(python)
        .args(["-c", script])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Interop with the Python implementation's mutagen writer/reader. Runs only
/// when `FRAMECOVER_MUTAGEN_PYTHON` names a Python with mutagen installed
/// (e.g. the repo's `.venv/Scripts/python.exe` after `uv sync --extra dev`).
#[test]
fn mutagen_interop() {
    let Some(python) = std::env::var_os("FRAMECOVER_MUTAGEN_PYTHON") else {
        eprintln!("FRAMECOVER_MUTAGEN_PYTHON not set; skipping mutagen interop");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let jpeg = framecover_core::imaging::cover_jpeg(&common::sample_image()).unwrap();
    let jpeg_path = dir.path().join("cover.jpg");
    std::fs::write(&jpeg_path, &jpeg).unwrap();

    // Rust writes, mutagen reads.
    let rust_written = common::sample_video(dir.path());
    write_covr(&rust_written, &jpeg).unwrap();
    write_thumbnail_source(&rust_written, 42, 1680).unwrap();
    let script = r#"
import sys
from mutagen.mp4 import MP4, AtomDataType
t = MP4(sys.argv[1]).tags
fn = t["----:com.apple.iTunes:video-thumbnailer-frame-number"][0]
pm = t["----:com.apple.iTunes:video-thumbnailer-position-ms"][0]
assert bytes(fn) == b"42" and fn.dataformat == AtomDataType.UTF8, fn
assert bytes(pm) == b"1680" and pm.dataformat == AtomDataType.UTF8, pm
cover = t["covr"]
assert len(cover) == 1 and cover[0].imageformat == 13
assert bytes(cover[0]) == open(sys.argv[2], "rb").read()
assert "\xa9too" in t
"#;
    run_python(&python, script, &[&rust_written, &jpeg_path]);

    // mutagen writes (exactly like thumbnail_metadata.py), Rust reads.
    let sub = dir.path().join("py");
    std::fs::create_dir(&sub).unwrap();
    let py_written = common::sample_video(&sub);
    let packets = support::video_packets(&py_written);
    let script = r#"
import sys
from mutagen.mp4 import MP4, MP4Cover, MP4FreeForm
tags = MP4(sys.argv[1]); tags.tags = tags.tags or {}
tags.tags["covr"] = [MP4Cover(open(sys.argv[2], "rb").read(), imageformat=13)]
tags.save()
tags = MP4(sys.argv[1]); tags.tags = tags.tags or {}
tags.tags["----:com.apple.iTunes:video-thumbnailer-frame-number"] = [MP4FreeForm(b"25")]
tags.tags["----:com.apple.iTunes:video-thumbnailer-position-ms"] = [MP4FreeForm(b"1000")]
tags.save()
"#;
    run_python(&python, script, &[&py_written, &jpeg_path]);
    assert_eq!(read_thumbnail_source(&py_written), (Some(25), Some(1000)));
    // Rust can rewrite a mutagen-written file without damaging it.
    write_thumbnail_source(&py_written, 26, 1040).unwrap();
    assert_eq!(read_thumbnail_source(&py_written), (Some(26), Some(1040)));
    let tag = Tag::read_from_path(&py_written).unwrap();
    assert_eq!(tag.artwork().unwrap().data, jpeg.as_slice());
    assert_eq!(support::video_packets(&py_written), packets);
    let script = r#"
import sys
from mutagen.mp4 import MP4
t = MP4(sys.argv[1]).tags
assert bytes(t["----:com.apple.iTunes:video-thumbnailer-frame-number"][0]) == b"26"
assert len(t["covr"]) == 1
"#;
    run_python(&python, script, &[&py_written]);
}
