//! Container/stream inspection via `ffprobe -of json`.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::ffmpeg::{self, Tool};

/// An exact rational number such as a time base (`1/90000`) or frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i64,
    pub den: i64,
}

impl Rational {
    /// Parse `"num/den"` (or a bare integer). Returns `None` for `0/0` or junk.
    pub fn parse(text: &str) -> Option<Self> {
        let (num, den) = match text.split_once('/') {
            Some((n, d)) => (n.trim().parse().ok()?, d.trim().parse().ok()?),
            None => (text.trim().parse().ok()?, 1),
        };
        (den != 0).then_some(Self { num, den })
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

impl std::fmt::Display for Rational {
    /// Formats like Python's `fractions.Fraction`: `30` or `30000/1001`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = gcd(self.num.unsigned_abs(), self.den.unsigned_abs()).max(1) as i64;
        let (mut n, mut d) = (self.num / g, self.den / g);
        if d < 0 {
            n = -n;
            d = -d;
        }
        if d == 1 {
            write!(f, "{n}")
        } else {
            write!(f, "{n}/{d}")
        }
    }
}

/// Maximum relative difference between `avg_frame_rate` and `r_frame_rate`
/// treated as timestamp drift of a constant-rate stream (5%: two squashed
/// frames in a clip of about 40 frames or more).
pub const RATE_DRIFT_TOLERANCE: f64 = 0.05;

/// `average` differs from `base`, but by no more than the drift tolerance.
fn rate_drifted(average: Rational, base: Rational) -> bool {
    let (a, b) = (average.to_f64(), base.to_f64());
    a > 0.0
        && b > 0.0
        && average.num as i128 * base.den as i128 != base.num as i128 * average.den as i128
        && ((a - b) / b).abs() <= RATE_DRIFT_TOLERANCE
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Stream disposition flags we care about.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Disposition {
    #[serde(default)]
    pub attached_pic: u8,
}

/// One stream as reported by ffprobe.
#[derive(Debug, Clone, Deserialize)]
pub struct Stream {
    pub index: usize,
    #[serde(default)]
    pub codec_type: String,
    #[serde(default)]
    pub codec_name: Option<String>,
    #[serde(default)]
    pub codec_long_name: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    time_base: Option<String>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
    #[serde(default)]
    r_frame_rate: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    duration_ts: Option<i64>,
    #[serde(default)]
    nb_frames: Option<String>,
    #[serde(default)]
    sample_aspect_ratio: Option<String>,
    #[serde(default)]
    pub disposition: Disposition,
    #[serde(default)]
    pub tags: HashMap<String, String>,
    /// Container stream ID (`0x1`: the MP4/MOV `track_ID`).
    #[serde(default)]
    pub id: Option<String>,
    /// Sample entry / FourCC (`avc1`, `hvc1`, `hev1`, `dvh1`…).
    #[serde(default)]
    pub codec_tag_string: Option<String>,
    /// `tv` / `pc` (or `unknown`).
    #[serde(default)]
    pub color_range: Option<String>,
    /// Matrix coefficients (`bt709`, `bt2020nc`…).
    #[serde(default)]
    pub color_space: Option<String>,
    /// Transfer characteristics (`bt709`, `smpte2084`, `arib-std-b67`…).
    #[serde(default)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    pub color_primaries: Option<String>,
    /// `left`, `topleft`, `center`…
    #[serde(default)]
    pub chroma_location: Option<String>,
    /// `progressive`, `tt`, `bb`, `tb`, `bt` or `unknown`.
    #[serde(default)]
    pub field_order: Option<String>,
    /// Stream-level side data (mastering display, content light level,
    /// Dolby Vision configuration…), as ffprobe prints it.
    #[serde(default)]
    pub side_data_list: Vec<serde_json::Value>,
}

/// A colour property ffprobe reports, `None` when unknown/unspecified.
fn known(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .filter(|v| !v.is_empty() && !matches!(*v, "unknown" | "unspecified" | "reserved"))
}

impl Stream {
    /// The container's numeric stream ID (MP4/MOV `track_ID`).
    pub fn track_id(&self) -> Option<u32> {
        let id = self.id.as_deref()?;
        match id.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => id.parse().ok(),
        }
    }

    /// Known colour range (`tv`/`pc`).
    pub fn color_range(&self) -> Option<&str> {
        known(&self.color_range)
    }

    /// Known matrix coefficients.
    pub fn color_space(&self) -> Option<&str> {
        known(&self.color_space)
    }

    /// Known transfer characteristics.
    pub fn color_transfer(&self) -> Option<&str> {
        known(&self.color_transfer)
    }

    /// Known colour primaries.
    pub fn color_primaries(&self) -> Option<&str> {
        known(&self.color_primaries)
    }

    /// Known chroma sample location.
    pub fn chroma_location(&self) -> Option<&str> {
        known(&self.chroma_location)
    }

    /// High dynamic range video: PQ (SMPTE ST 2084, HDR10) or HLG
    /// (ARIB STD-B67) transfer characteristics.
    pub fn is_hdr(&self) -> bool {
        matches!(self.color_transfer(), Some("smpte2084" | "arib-std-b67"))
    }

    /// Bits per sample from the pixel format (`yuv420p10le` → 10, 8 for
    /// formats without a depth suffix).
    pub fn bit_depth(&self) -> Option<u32> {
        let pix_fmt = known(&self.pix_fmt)?;
        let base = pix_fmt
            .strip_suffix("le")
            .or_else(|| pix_fmt.strip_suffix("be"))
            .unwrap_or(pix_fmt);
        // `yuv420p10` / `gbrp12`: digits after the planar `p` are the depth;
        // `yuv420p` (no suffix) and packed formats are 8-bit.
        let head = base.trim_end_matches(|c: char| c.is_ascii_digit());
        let digits = &base[head.len()..];
        Some(match digits.parse::<u32>() {
            Ok(depth) if head.ends_with('p') => depth,
            _ => 8,
        })
    }

    /// Interlaced (field-coded) video according to ffprobe's field order.
    pub fn is_interlaced(&self) -> bool {
        matches!(known(&self.field_order), Some("tt" | "bb" | "tb" | "bt"))
    }

    /// Dolby Vision: a DOVI configuration record in the stream side data or
    /// a Dolby Vision sample entry (`dvh1`, `dvhe`, `dav1`, `dva1`, `dvav`).
    /// Returns the profile when known (`Some(None)` for an unknown profile).
    pub fn dolby_vision(&self) -> Option<Option<u64>> {
        let record = self.side_data_list.iter().find(|d| {
            d.get("side_data_type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t.contains("DOVI") || t.contains("Dolby Vision"))
        });
        if let Some(record) = record {
            return Some(record.get("dv_profile").and_then(serde_json::Value::as_u64));
        }
        let tag = self.codec_tag_string.as_deref().unwrap_or("");
        matches!(tag, "dvh1" | "dvhe" | "dav1" | "dva1" | "dvav").then_some(None)
    }

    /// Stream-level side data entry of `kind` (e.g. `Mastering display metadata`).
    pub fn side_data(&self, kind: &str) -> Option<&serde_json::Value> {
        self.side_data_list
            .iter()
            .find(|d| d.get("side_data_type").and_then(|t| t.as_str()) == Some(kind))
    }
    pub fn is_video(&self) -> bool {
        self.codec_type == "video"
    }

    pub fn is_attached_pic(&self) -> bool {
        self.disposition.attached_pic != 0
    }

    /// A video stream that is real playable video, not cover art.
    pub fn is_playable_video(&self) -> bool {
        self.is_video() && !self.is_attached_pic()
    }

    pub fn time_base(&self) -> Option<Rational> {
        self.time_base.as_deref().and_then(Rational::parse)
    }

    /// Average frame rate (`avg_frame_rate`), `None` if unknown/zero.
    pub fn average_rate(&self) -> Option<Rational> {
        self.avg_frame_rate
            .as_deref()
            .and_then(Rational::parse)
            .filter(|r| r.num > 0)
    }

    /// Base/real frame rate (`r_frame_rate`), `None` if unknown/zero.
    pub fn base_rate(&self) -> Option<Rational> {
        self.r_frame_rate
            .as_deref()
            .and_then(Rational::parse)
            .filter(|r| r.num > 0)
    }

    /// Frame rate to use for frame steps and for encoding a matching preview.
    ///
    /// Normally `avg_frame_rate`. When `r_frame_rate` is also known and the
    /// two differ by at most [`RATE_DRIFT_TOLERANCE`] (relative), prefer
    /// `r_frame_rate`: a small deviation means a constant-rate stream whose
    /// average was skewed by a few odd timestamps — typically MP4s saved by
    /// earlier FrameCover versions, whose leading DTS were squashed by the
    /// muxer (`6275/249` or `1575/62` instead of `25/1`). Larger differences
    /// (true VFR, field rates, `90000/1` base rates) keep the average.
    pub fn effective_rate(&self) -> Option<Rational> {
        match (self.average_rate(), self.base_rate()) {
            (Some(a), Some(b)) if rate_drifted(a, b) => Some(b),
            (Some(a), _) => Some(a),
            (None, b) => b,
        }
    }

    /// Whether [`effective_rate`](Self::effective_rate) replaced a drifted
    /// `avg_frame_rate` with `r_frame_rate`.
    pub fn has_rate_drift(&self) -> bool {
        matches!((self.average_rate(), self.base_rate()), (Some(a), Some(b)) if rate_drifted(a, b))
    }

    /// Sample (pixel) aspect ratio, `None` if unknown (`0:1`, `N/A`).
    pub fn sample_aspect_ratio(&self) -> Option<Rational> {
        self.sample_aspect_ratio
            .as_deref()
            .and_then(|s| Rational::parse(&s.replace(':', "/")))
            .filter(|r| r.num > 0 && r.den > 0)
    }

    /// Number of frames (`nb_frames`), if the container reports it.
    pub fn nb_frames(&self) -> Option<u64> {
        self.nb_frames.as_deref()?.trim().parse().ok()
    }

    /// Stream duration in `time_base` units (`duration_ts`), if reported.
    pub fn duration_ts(&self) -> Option<i64> {
        self.duration_ts
    }

    /// Stream duration in seconds, if reported.
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration.as_deref()?.parse().ok()
    }

    /// Case-insensitive tag lookup (ffprobe preserves container tag case).
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// Container-level information.
#[derive(Debug, Clone, Deserialize)]
pub struct Format {
    #[serde(default)]
    pub format_name: String,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

impl Format {
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration.as_deref()?.parse().ok()
    }
}

/// Full ffprobe result for a file.
#[derive(Debug, Clone, Deserialize)]
pub struct ProbeInfo {
    #[serde(default)]
    pub streams: Vec<Stream>,
    pub format: Format,
}

impl ProbeInfo {
    pub fn playable_video(&self) -> impl Iterator<Item = &Stream> {
        self.streams.iter().filter(|s| s.is_playable_video())
    }

    pub fn first_playable_video(&self) -> Option<&Stream> {
        self.playable_video().next()
    }

    pub fn has_attached_pic(&self) -> bool {
        self.streams
            .iter()
            .any(|s| s.is_video() && s.is_attached_pic())
    }
}

/// Run ffprobe on `path`. Fails with [`Error::Other`] if the file cannot be
/// parsed as media at all.
pub fn probe(path: &Path) -> Result<ProbeInfo> {
    let mut cmd = ffmpeg::command(Tool::Ffprobe)?;
    cmd.args([
        "-v",
        "error",
        "-of",
        "json",
        "-show_format",
        "-show_streams",
        "-i",
    ])
    .arg(path);
    let out = ffmpeg::run(&mut cmd).map_err(|e| match e {
        Error::Ffmpeg { stderr, .. } => {
            Error::Other(format!("Could not read '{}': {stderr}", path.display()))
        }
        other => other,
    })?;
    parse(&out.stdout)
}

/// Parse ffprobe JSON output.
pub fn parse(json: &[u8]) -> Result<ProbeInfo> {
    serde_json::from_slice(json).map_err(|e| Error::Other(format!("invalid ffprobe output: {e}")))
}

/// HDR10 static metadata (SMPTE ST 2086 mastering display colour volume and
/// CTA-861.3 content light level).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HdrStaticMetadata {
    /// `[[x, y]; 4]` for red, green, blue and the white point (CIE 1931).
    pub primaries: Option<[[f64; 2]; 4]>,
    /// `(min, max)` mastering luminance in cd/m².
    pub luminance: Option<(f64, f64)>,
    /// `(MaxCLL, MaxFALL)` in cd/m².
    pub light_level: Option<(u64, u64)>,
}

impl HdrStaticMetadata {
    pub fn is_empty(&self) -> bool {
        self.primaries.is_none() && self.luminance.is_none() && self.light_level.is_none()
    }

    /// Read from ffprobe side data entries (stream or frame level).
    pub fn from_side_data<'a>(entries: impl IntoIterator<Item = &'a serde_json::Value>) -> Self {
        let mut meta = Self::default();
        for entry in entries {
            match entry.get("side_data_type").and_then(|t| t.as_str()) {
                Some("Mastering display metadata") => {
                    let value = |key: &str| {
                        entry
                            .get(key)
                            .and_then(|v| v.as_str())
                            .and_then(Rational::parse)
                            .map(Rational::to_f64)
                    };
                    let point = |x: &str, y: &str| Some([value(x)?, value(y)?]);
                    meta.primaries = (|| {
                        Some([
                            point("red_x", "red_y")?,
                            point("green_x", "green_y")?,
                            point("blue_x", "blue_y")?,
                            point("white_point_x", "white_point_y")?,
                        ])
                    })();
                    meta.luminance = value("min_luminance").zip(value("max_luminance"));
                }
                Some("Content light level metadata") => {
                    let value = |key: &str| entry.get(key).and_then(serde_json::Value::as_u64);
                    meta.light_level = value("max_content").zip(value("max_average"));
                }
                _ => {}
            }
        }
        meta
    }

    /// x265/x264 `master-display` value:
    /// `G(x,y)B(x,y)R(x,y)WP(x,y)L(max,min)` in 0.00002 and 0.0001 cd/m² units.
    pub fn master_display(&self) -> Option<String> {
        let [r, g, b, wp] = self.primaries?;
        let (min, max) = self.luminance?;
        let c = |v: f64| (v * 50_000.0).round() as u64;
        let l = |v: f64| (v * 10_000.0).round() as u64;
        Some(format!(
            "G({},{})B({},{})R({},{})WP({},{})L({},{})",
            c(g[0]),
            c(g[1]),
            c(b[0]),
            c(b[1]),
            c(r[0]),
            c(r[1]),
            c(wp[0]),
            c(wp[1]),
            l(max),
            l(min)
        ))
    }

    /// x265 `max-cll` / x264 `cll` value: `MaxCLL,MaxFALL`.
    pub fn max_cll(&self) -> Option<String> {
        self.light_level.map(|(cll, fall)| format!("{cll},{fall}"))
    }
}

/// HDR10 static metadata of video stream `index`: the stream-level side data
/// (MP4 `mdcv`/`clli` boxes) when present, else the first decoded frame's
/// (HEVC/H.264 SEI, AV1 metadata OBUs).
pub fn hdr_static_metadata(path: &Path, stream: &Stream) -> Result<HdrStaticMetadata> {
    let from_stream = HdrStaticMetadata::from_side_data(&stream.side_data_list);
    if from_stream.primaries.is_some() && from_stream.light_level.is_some() {
        return Ok(from_stream);
    }
    #[derive(Deserialize)]
    struct Frame {
        #[serde(default)]
        side_data_list: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Frames {
        #[serde(default)]
        frames: Vec<Frame>,
    }
    let mut cmd = ffmpeg::command(Tool::Ffprobe)?;
    cmd.args(["-v", "error", "-select_streams"])
        .arg(stream.index.to_string())
        .args([
            "-read_intervals",
            "%+#1",
            "-show_frames",
            "-of",
            "json",
            "-i",
        ])
        .arg(path);
    let out = ffmpeg::run(&mut cmd)?;
    let parsed: Frames = serde_json::from_slice(&out.stdout)
        .map_err(|e| Error::Other(format!("invalid ffprobe frame output: {e}")))?;
    let from_frame = HdrStaticMetadata::from_side_data(
        parsed.frames.iter().take(1).flat_map(|f| &f.side_data_list),
    );
    Ok(HdrStaticMetadata {
        primaries: from_stream.primaries.or(from_frame.primaries),
        luminance: from_stream.luminance.or(from_frame.luminance),
        light_level: from_stream.light_level.or(from_frame.light_level),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_parse_and_display() {
        assert_eq!(
            Rational::parse("1/90000"),
            Some(Rational { num: 1, den: 90000 })
        );
        assert_eq!(Rational::parse("0/0"), None);
        assert_eq!(Rational::parse("60/2").unwrap().to_string(), "30");
        assert_eq!(
            Rational::parse("30000/1001").unwrap().to_string(),
            "30000/1001"
        );
        assert_eq!(Rational::parse("25").unwrap().to_string(), "25");
    }

    #[test]
    fn parses_ffprobe_json() {
        let json = br#"{
          "streams": [
            {"index": 0, "codec_type": "video", "codec_name": "h264", "profile": "High",
             "pix_fmt": "yuv420p", "width": 320, "height": 240, "time_base": "1/12800",
             "avg_frame_rate": "25/1", "r_frame_rate": "25/1", "duration": "2.000000",
             "disposition": {"default": 1, "attached_pic": 0}},
            {"index": 1, "codec_type": "audio", "codec_name": "aac", "time_base": "1/44100",
             "avg_frame_rate": "0/0", "disposition": {"attached_pic": 0}},
            {"index": 2, "codec_type": "video", "codec_name": "mjpeg", "width": 64, "height": 64,
             "disposition": {"attached_pic": 1}, "tags": {"FILENAME": "cover.jpg"}}
          ],
          "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "2.023000"}
        }"#;
        let info = parse(json).unwrap();
        assert_eq!(info.playable_video().count(), 1);
        assert!(info.has_attached_pic());
        let v = info.first_playable_video().unwrap();
        assert_eq!(v.time_base(), Some(Rational { num: 1, den: 12800 }));
        assert_eq!(v.average_rate().unwrap().to_string(), "25");
        assert_eq!(info.streams[1].average_rate(), None);
        assert_eq!(info.streams[2].tag("filename"), Some("cover.jpg"));
        assert_eq!(info.format.duration_secs(), Some(2.023));
    }

    fn rates(avg: &str, r: &str) -> Stream {
        let json = format!(
            r#"{{"streams": [{{"index": 0, "codec_type": "video", "avg_frame_rate": "{avg}",
                "r_frame_rate": "{r}", "nb_frames": "126"}}], "format": {{"format_name": "mp4"}}}}"#
        );
        parse(json.as_bytes()).unwrap().streams.remove(0)
    }

    fn effective(avg: &str, r: &str) -> String {
        rates(avg, r).effective_rate().unwrap().to_string()
    }

    #[test]
    fn effective_rate_corrects_small_drift_only() {
        let s = rates("25/1", "25/1");
        assert_eq!(effective("25/1", "25/1"), "25");
        assert!(!s.has_rate_drift());
        assert_eq!(s.nb_frames(), Some(126));
        // Squashed-DTS outputs of earlier versions (both drift values seen).
        for drifted in ["6275/249", "1575/62"] {
            assert_eq!(effective(drifted, "25/1"), "25");
            assert!(rates(drifted, "25/1").has_rate_drift());
        }
        // Same value, different spelling: not drift.
        assert!(!rates("50/2", "25/1").has_rate_drift());
        // Large deviations are real (field rate, VFR with a timescale base rate).
        assert_eq!(effective("25/1", "50/1"), "25");
        assert_eq!(effective("2997/100", "90000/1"), "2997/100");
        // Missing average: base rate.
        assert_eq!(effective("0/0", "24/1"), "24");
        assert!(!rates("0/0", "24/1").has_rate_drift());
    }
}
