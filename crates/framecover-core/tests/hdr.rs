//! HDR and high-bit-depth sources: the concat fast path must keep the source
//! packets, colour description and HDR metadata, the inserted frame must be
//! encoded in the source's colour space, and HDR frames shown in the UI must
//! be tone-mapped (identically by the extractor and the frame stream).

mod support;

use std::path::{Path, PathBuf};

use framecover_core::frame_stream::{FfmpegFrameStream, FrameSource};
use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
use framecover_core::{ApplyResult, ffmpeg};
use image::RgbImage;
use serde_json::Value;

const MASTER_DISPLAY: &str = "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)";
const MAX_CLL: &str = "1000,400";

/// SDR test pattern → PQ or HLG BT.2020, SDR white at 203 cd/m² (BT.2408).
fn to_hdr(transfer: &str) -> String {
    format!(
        "format=gbrp,zscale=tin=bt709:pin=bt709:rin=pc:t={transfer}:p=bt2020:m=bt2020nc:r=tv:\
         npl=203,format=yuv420p10le"
    )
}

/// Encode `seconds` of `testsrc2` (+ a sine track) at 25 fps.
fn fixture(out: &Path, seconds: u32, size: &str, video: &[&str]) -> PathBuf {
    let src = format!("testsrc2=size={size}:rate=25:duration={seconds}");
    let sine = format!("sine=frequency=440:duration={seconds}");
    let mut args = vec![
        "-y", "-v", "error", "-f", "lavfi", "-i", &src, "-f", "lavfi", "-i", &sine,
    ];
    args.extend_from_slice(video);
    args.extend(["-c:a", "aac", out.to_str().unwrap()]);
    ffmpeg::ffmpeg(&args).expect("generate fixture");
    out.to_owned()
}

fn hdr10_hevc(dir: &Path, name: &str, tag: &str) -> PathBuf {
    let x265 = format!(
        "log-level=error:hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:\
         master-display={MASTER_DISPLAY}:max-cll={MAX_CLL}"
    );
    fixture(
        &dir.join(name),
        2,
        "320x240",
        &[
            "-vf",
            &to_hdr("smpte2084"),
            "-c:v",
            "libx265",
            "-tag:v",
            tag,
            "-x265-params",
            &x265,
        ],
    )
}

fn hlg_hevc(dir: &Path, name: &str) -> PathBuf {
    fixture(
        &dir.join(name),
        2,
        "320x240",
        &[
            "-vf",
            &to_hdr("arib-std-b67"),
            "-c:v",
            "libx265",
            "-tag:v",
            "hvc1",
            "-x265-params",
            "log-level=error",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "arib-std-b67",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ],
    )
}

fn ffprobe_json(args: &[&str], path: &Path) -> Value {
    let out = support::ffprobe()
        .args(["-v", "error", "-of", "json"])
        .args(args)
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed on {}", path.display());
    serde_json::from_slice(&out.stdout).unwrap()
}

fn video_stream(path: &Path) -> Value {
    ffprobe_json(&["-select_streams", "v:0", "-show_streams"], path)["streams"][0].clone()
}

/// Side data of the first decoded video frame (SEI / metadata OBUs).
fn first_frame_side_data(path: &Path) -> Value {
    let v = ffprobe_json(
        &[
            "-select_streams",
            "v:0",
            "-read_intervals",
            "%+#1",
            "-show_frames",
        ],
        path,
    );
    let mut list = v["frames"][0]["side_data_list"].clone();
    // x265 writes its version and settings as unregistered user data.
    if let Some(items) = list.as_array_mut() {
        items.retain(|d| d["side_data_type"] != "H.26[45] User Data Unregistered SEI message");
    }
    list
}

/// `(pts, dts)` of every video packet.
fn timestamps(path: &Path) -> Vec<(i64, i64)> {
    let v = ffprobe_json(
        &["-select_streams", "v:0", "-show_entries", "packet=pts,dts"],
        path,
    );
    v["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["pts"].as_i64().unwrap(), p["dts"].as_i64().unwrap()))
        .collect()
}

fn red() -> RgbImage {
    RgbImage::from_pixel(320, 240, image::Rgb([240, 20, 20]))
}

fn save(path: &Path, image: &RgbImage, position_ms: u64) -> ApplyResult {
    let video = FfprobeVideoLoader.load(path).unwrap();
    let result = FormatDispatchThumbnailWriter.write(&video, image, Some(position_ms), None);
    assert!(result.success, "{:?}", result.error_message);
    result
}

fn reencode_reason(path: &Path) -> Option<String> {
    FormatDispatchThumbnailWriter.reencode_reason(&FfprobeVideoLoader.load(path).unwrap())
}

fn copy_of(source: &Path, name: &str) -> PathBuf {
    let target = source.with_file_name(name);
    std::fs::copy(source, &target).unwrap();
    target
}

const COLOR_FIELDS: [&str; 9] = [
    "codec_name",
    "codec_tag_string",
    "profile",
    "pix_fmt",
    "color_range",
    "color_space",
    "color_transfer",
    "color_primaries",
    "side_data_list",
];

/// The fast path ran: one frame prepended, DTS strictly increasing, every
/// source packet copied unchanged (`skip` leading output packets differ:
/// the preview, plus for H.264 the first source keyframe, which gains
/// in-band parameter sets), the source decodes to the same frames, and the
/// stream-level colour description and HDR side data are the source's.
fn assert_fast_path_copy(source: &Path, output: &Path, skip: usize) {
    let (src, out) = (
        support::video_packets(source),
        support::video_packets(output),
    );
    assert_eq!(out.len(), src.len() + 1, "one frame prepended");
    assert_eq!(&out[skip..], &src[skip - 1..], "source packets re-encoded");
    // Decoding with the right parameter sets gives the source's pictures.
    let (src_frames, out_frames) = (support::video_frames(source), support::video_frames(output));
    assert_eq!(
        &out_frames[1..],
        &src_frames[..],
        "source frames decode differently"
    );

    let ts = timestamps(output);
    for w in ts.windows(2) {
        assert!(
            w[1].1 > w[0].1,
            "DTS not increasing: {:?} -> {:?}",
            w[0],
            w[1]
        );
    }
    assert_eq!(
        ts[0].0,
        ts.iter().map(|t| t.0).min().unwrap(),
        "preview not first"
    );

    let (sv, ov) = (video_stream(source), video_stream(output));
    for key in COLOR_FIELDS {
        assert_eq!(ov[key], sv[key], "stream {key} changed");
    }
    assert_eq!(ov["avg_frame_rate"], sv["avg_frame_rate"]);
}

/// Mean RGB of the first frame the extractor shows (tone-mapped for HDR).
fn shown_first_frame(path: &Path) -> [f64; 3] {
    let video = FfprobeVideoLoader.load(path).unwrap();
    support::mean(&FfmpegFrameExtractor.extract(&video, 0).unwrap())
}

fn assert_reddish([r, g, b]: [f64; 3]) {
    assert!(
        r > 150.0 && g < 90.0 && b < 90.0,
        "first frame mean {r} {g} {b}"
    );
}

#[test]
fn hdr10_hevc_mp4_takes_fast_path_and_keeps_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let source = hdr10_hevc(dir.path(), "hdr10.mp4", "hvc1");
    assert_eq!(reencode_reason(&source), None);
    let target = copy_of(&source, "target.mp4");
    let result = save(&target, &red(), 1000);
    assert_eq!(result.duration_shift_ms, 40);
    assert_fast_path_copy(&source, &target, 1);
    // The preview repeats the HDR10 SEI of the source.
    let sei = first_frame_side_data(&target);
    assert_eq!(sei, first_frame_side_data(&source));
    assert_eq!(sei.as_array().map(Vec::len), Some(2), "{sei}");
    assert_reddish(shown_first_frame(&target));
    assert_eq!(reencode_reason(&target), None);

    // A second save replaces the preview, still without re-encoding.
    let result = save(&target, &red(), 1500);
    assert_eq!(result.duration_shift_ms, 0);
    let (src, out) = (
        support::video_packets(&source),
        support::video_packets(&target),
    );
    assert_eq!(&out[1..], &src[..]);
    assert_eq!(
        &support::video_frames(&target)[1..],
        &support::video_frames(&source)[..]
    );
    let (sv, ov) = (video_stream(&source), video_stream(&target));
    for key in COLOR_FIELDS {
        assert_eq!(ov[key], sv[key], "stream {key} changed on re-save");
    }
    assert_reddish(shown_first_frame(&target));
}

#[test]
fn hlg_hevc_mov_keeps_hvc1_and_hlg() {
    let dir = tempfile::tempdir().unwrap();
    let source = hlg_hevc(dir.path(), "hlg.mov");
    assert_eq!(video_stream(&source)["codec_tag_string"], "hvc1");
    let target = copy_of(&source, "target.mov");
    let result = save(&target, &red(), 1000);
    assert_eq!(result.duration_shift_ms, 40);
    assert_fast_path_copy(&source, &target, 1);
    assert_reddish(shown_first_frame(&target));
    // The preview frame itself is HLG BT.2020 10-bit.
    let frame = ffprobe_json(
        &[
            "-select_streams",
            "v:0",
            "-read_intervals",
            "%+#1",
            "-show_entries",
            "frame=pix_fmt,color_transfer,color_primaries,color_space,color_range",
        ],
        &target,
    )["frames"][0]
        .clone();
    assert_eq!(frame["pix_fmt"], "yuv420p10le");
    assert_eq!(frame["color_transfer"], "arib-std-b67");
    assert_eq!(frame["color_primaries"], "bt2020");
    assert_eq!(frame["color_space"], "bt2020nc");
}

#[test]
fn hev1_sample_entry_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let source = hdr10_hevc(dir.path(), "hev1.mp4", "hev1");
    let target = copy_of(&source, "target.mp4");
    save(&target, &red(), 1000);
    assert_fast_path_copy(&source, &target, 1);
    assert_eq!(video_stream(&target)["codec_tag_string"], "hev1");
}

#[test]
fn sdr_hevc_main_takes_fast_path() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(
        &dir.path().join("hevc8.mp4"),
        2,
        "320x240",
        &[
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx265",
            "-tag:v",
            "hvc1",
            "-x265-params",
            "log-level=error",
        ],
    );
    assert_eq!(reencode_reason(&source), None);
    let target = copy_of(&source, "target.mp4");
    save(&target, &red(), 1000);
    assert_fast_path_copy(&source, &target, 1);
    assert_reddish(support::mean(&support::first_playable_frame(&target)));
}

#[test]
fn h264_high10_takes_fast_path() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(
        &dir.path().join("high10.mp4"),
        2,
        "320x240",
        &[
            "-vf",
            "format=yuv420p10le",
            "-c:v",
            "libx264",
            "-profile:v",
            "high10",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
    );
    assert_eq!(video_stream(&source)["profile"], "High 10");
    assert_eq!(reencode_reason(&source), None);
    let target = copy_of(&source, "target.mp4");
    let result = save(&target, &red(), 1000);
    assert_eq!(result.duration_shift_ms, 40);
    // H.264: the first source keyframe gains in-band parameter sets.
    assert_fast_path_copy(&source, &target, 2);
    let first = support::first_playable_frame(&target);
    let [r, g, b] = support::mean(&first);
    // SDR: the inserted frame decodes back to the chosen colour.
    assert!(
        (r - 240.0).abs() < 6.0 && g < 26.0 && b < 26.0,
        "{r} {g} {b}"
    );
}

#[test]
fn av1_main_10bit_takes_fast_path() {
    if !support::has_encoder("libaom-av1") {
        eprintln!("libaom-av1 unavailable; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(
        &dir.path().join("av1_10.mp4"),
        1,
        "160x120",
        &[
            "-vf",
            &to_hdr("smpte2084"),
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
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ],
    );
    assert_eq!(video_stream(&source)["pix_fmt"], "yuv420p10le");
    assert_eq!(reencode_reason(&source), None);
    let target = copy_of(&source, "target.mp4");
    let small_red = RgbImage::from_pixel(160, 120, image::Rgb([240, 20, 20]));
    let result = save(&target, &small_red, 500);
    assert_eq!(result.duration_shift_ms, 40);
    assert_fast_path_copy(&source, &target, 1);
    assert_reddish(shown_first_frame(&target));
}

#[test]
fn hevc_422_reencodes_to_main10_keeping_hlg() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(
        &dir.path().join("hlg422.mp4"),
        2,
        "320x240",
        &[
            "-vf",
            &to_hdr("arib-std-b67").replace("yuv420p10le", "yuv422p10le"),
            "-c:v",
            "libx265",
            "-x265-params",
            "log-level=error",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "arib-std-b67",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ],
    );
    let reason = reencode_reason(&source).expect("4:2:2 is not eligible");
    assert!(reason.contains("'yuv422p10le'"), "{reason}");
    let target = copy_of(&source, "target.mp4");
    let result = save(&target, &red(), 1000);
    assert_eq!(result.duration_shift_ms, 40);
    let v = video_stream(&target);
    assert_eq!(v["codec_name"], "hevc");
    assert_eq!(v["codec_tag_string"], "hvc1");
    assert_eq!(v["profile"], "Main 10");
    assert_eq!(v["pix_fmt"], "yuv420p10le");
    assert_eq!(v["color_transfer"], "arib-std-b67");
    assert_eq!(v["color_primaries"], "bt2020");
    assert_eq!(v["color_space"], "bt2020nc");
    assert_eq!(
        support::video_packets(&target).len(),
        support::video_packets(&source).len() + 1
    );
    assert_reddish(shown_first_frame(&target));
    // The re-encoded file now qualifies for the fast path.
    assert_eq!(reencode_reason(&target), None);
}

#[test]
fn dolby_vision_falls_back_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let base = hdr10_hevc(dir.path(), "base.mp4", "hvc1");
    // A `dvh1` sample entry marks Dolby Vision (no DV encoder in ffmpeg).
    let source = dir.path().join("dv.mp4");
    let tagged = ffmpeg::ffmpeg([
        "-y",
        "-v",
        "error",
        "-i",
        base.to_str().unwrap(),
        "-c",
        "copy",
        "-tag:v",
        "dvh1",
        "-strict",
        "unofficial",
        source.to_str().unwrap(),
    ]);
    if tagged.is_err() || video_stream(&source)["codec_tag_string"] != "dvh1" {
        eprintln!("ffmpeg cannot write a dvh1 sample entry; skipping");
        return;
    }
    let reason = reencode_reason(&source).expect("Dolby Vision is not eligible");
    assert!(reason.contains("Dolby Vision"), "{reason}");
}

// ---------------------------------------------------------------------------
// Converter
// ---------------------------------------------------------------------------

fn convert(source: &Path) -> (PathBuf, Vec<String>) {
    let target = source.with_file_name("converted.mp4");
    let mut stages = Vec::new();
    let mut cb = |s: &str| stages.push(s.to_owned());
    framecover_core::converter::FfmpegVideoConverter
        .convert(source, &target, false, Some(&mut cb))
        .unwrap();
    (target, stages)
}

#[test]
fn hdr_hevc_mkv_is_remuxed_as_hvc1() {
    let dir = tempfile::tempdir().unwrap();
    let mp4 = hdr10_hevc(dir.path(), "hdr10.mp4", "hvc1");
    let mkv = dir.path().join("hdr10.mkv");
    support::run_ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-i",
        mp4.to_str().unwrap(),
        "-c",
        "copy",
        mkv.to_str().unwrap(),
    ]);
    let (out, stages) = convert(&mkv);
    assert_eq!(stages, ["Remuxing to MP4 without re-encoding"]);
    let v = video_stream(&out);
    assert_eq!(v["codec_tag_string"], "hvc1");
    for key in COLOR_FIELDS
        .iter()
        .filter(|k| !matches!(**k, "codec_tag_string" | "side_data_list"))
    {
        assert_eq!(v[*key], video_stream(&mp4)[*key], "{key}");
    }
    assert_eq!(support::video_packets(&out), support::video_packets(&mp4));
    // And the result takes the fast path.
    assert_eq!(reencode_reason(&out), None);
}

#[test]
fn hdr_vp9_is_transcoded_to_hevc_main10() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(
        &dir.path().join("hdr_vp9.mkv"),
        1,
        "160x120",
        &[
            "-vf",
            &to_hdr("smpte2084"),
            "-c:v",
            "libvpx-vp9",
            "-profile:v",
            "2",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-b:v",
            "200k",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ],
    );
    let (out, stages) = convert(&source);
    assert_eq!(stages, ["Encoding MP4 with libx265 (10-bit) / AAC"]);
    let v = video_stream(&out);
    assert_eq!(v["codec_name"], "hevc");
    assert_eq!(v["profile"], "Main 10");
    assert_eq!(v["codec_tag_string"], "hvc1");
    assert_eq!(v["color_transfer"], "smpte2084");
    assert_eq!(v["color_primaries"], "bt2020");
    assert_eq!(reencode_reason(&out), None);
}

// ---------------------------------------------------------------------------
// Display side: tone mapping
// ---------------------------------------------------------------------------

fn luma(img: &RgbImage) -> Vec<f64> {
    img.pixels()
        .map(|p| 0.2126 * p.0[0] as f64 + 0.7152 * p.0[1] as f64 + 0.0722 * p.0[2] as f64)
        .collect()
}

fn mean_std(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

fn mean_abs_diff(a: &RgbImage, b: &RgbImage) -> f64 {
    let n = (a.width() * a.height() * 3) as f64;
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| (*x as f64 - *y as f64).abs())
        .sum::<f64>()
        / n
}

#[test]
fn hdr_frames_are_tone_mapped_not_washed_out() {
    let dir = tempfile::tempdir().unwrap();
    for (name, transfer) in [("pq.mp4", "smpte2084"), ("hlg.mp4", "arib-std-b67")] {
        // The same pattern in SDR (reference) and HDR.
        let sdr = fixture(
            &dir.path().join(format!("sdr_{name}")),
            1,
            "320x240",
            &["-pix_fmt", "yuv444p", "-c:v", "libx264", "-qp", "0"],
        );
        let hdr = fixture(
            &dir.path().join(name),
            1,
            "320x240",
            &[
                "-vf",
                &to_hdr(transfer).replace("yuv420p10le", "yuv444p10le"),
                "-c:v",
                "libx265",
                "-x265-params",
                "log-level=error:lossless=1",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                transfer,
                "-colorspace",
                "bt2020nc",
                "-color_range",
                "tv",
            ],
        );
        let reference = support::first_playable_frame(&sdr);
        let naive = support::first_playable_frame(&hdr);
        let shown = FfmpegFrameExtractor
            .extract(&FfprobeVideoLoader.load(&hdr).unwrap(), 0)
            .unwrap();
        let (ref_mean, ref_std) = mean_std(&luma(&reference));
        let (naive_mean, naive_std) = mean_std(&luma(&naive));
        let (mean, std) = mean_std(&luma(&shown));
        eprintln!(
            "{transfer}: reference {ref_mean:.1}±{ref_std:.1}, naive {naive_mean:.1}±{naive_std:.1}, \
             tone-mapped {mean:.1}±{std:.1}; |diff| naive {:.1}, tone-mapped {:.1}",
            mean_abs_diff(&naive, &reference),
            mean_abs_diff(&shown, &reference)
        );
        // Brightness near the SDR rendition, contrast not flattened.
        assert!(
            (mean - ref_mean).abs() < 15.0,
            "{transfer}: mean {mean} vs {ref_mean}"
        );
        assert!(
            std > 0.8 * ref_std,
            "{transfer}: contrast {std} vs {ref_std}"
        );
        assert!(
            std > naive_std,
            "{transfer}: tone-mapped is flatter than naive"
        );
        assert!(
            mean_abs_diff(&shown, &reference) < 0.6 * mean_abs_diff(&naive, &reference),
            "{transfer}: tone mapping does not bring the frame closer to SDR"
        );
    }
}

#[test]
fn hdr_frame_stream_matches_extractor() {
    let dir = tempfile::tempdir().unwrap();
    let source = hdr10_hevc(dir.path(), "hdr10.mp4", "hvc1");
    let video = FfprobeVideoLoader.load(&source).unwrap();
    let mut stream = FfmpegFrameStream::new(video.clone());
    for ms in [0, 40, 80, 1000, 960, 920, 1960, 2000, 500, 0] {
        let streamed = stream.frame_at(ms).unwrap();
        let extracted = FfmpegFrameExtractor.extract(&video, ms).unwrap();
        assert!(streamed == extracted, "frame mismatch at {ms} ms");
    }
    assert_eq!(stream.fallbacks(), 0);
}

#[test]
fn sdr_sources_are_not_tone_mapped() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture(
        &dir.path().join("sdr.mp4"),
        1,
        "320x240",
        &["-pix_fmt", "yuv420p", "-c:v", "libx264"],
    );
    let shown = FfmpegFrameExtractor
        .extract(&FfprobeVideoLoader.load(&source).unwrap(), 0)
        .unwrap();
    assert!(shown == support::first_playable_frame(&source));
}
