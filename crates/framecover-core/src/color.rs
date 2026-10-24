//! Colour handling for HDR and high-bit-depth video (all done by ffmpeg
//! filters in the child processes).
//!
//! * **Display** ([`display_filter`]): frames shown in the UI, and therefore
//!   the cover JPEG, are 8-bit sRGB-ish RGB. PQ/HLG frames converted naively
//!   come out flat and grey, so HDR sources are tone-mapped to SDR BT.709:
//!   linearise with 100 cd/m² as 1.0, convert BT.2020 → BT.709 primaries in
//!   linear light, compress highlights with the Möbius curve (identity below
//!   0.3, i.e. 30 cd/m², smooth roll-off up to the peak from the stream's
//!   mastering/content-light metadata), then BT.709 transfer to full-range
//!   RGB. Möbius keeps shadows and mid-tones where the grade put them (BT.2408
//!   reference white at 203 cd/m² lands near SDR white); Hable, the usual
//!   alternative, darkens the whole picture by about 20 % at the same peak.
//!   `desat=0` keeps highlight colours instead of greying them. SDR sources
//!   keep the plain swscale conversion (pixels unchanged).
//! * **Insertion** ([`preview_filter`]): the chosen frame (8-bit RGB) is
//!   brought into the source's colour space before it is encoded as the
//!   preview frame. For PQ/HLG targets it is treated as BT.709 SDR and placed
//!   with SDR reference white at 203 cd/m² (ITU-R BT.2408: 58 % PQ, 75 % HLG),
//!   so it is neither blinding nor washed out next to the video. For SDR
//!   targets only the matrix, range and chroma siting change: the RGB values
//!   already are the source's (non-linear) values.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::probe::{self, Stream};

/// HDR → SDR BT.709 RGB24 tone-mapping chain (see the module docs). The
/// input colour properties come from the decoded frames.
pub const TONEMAP_TO_SDR_RGB: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
     tonemap=tonemap=mobius:desat=0,zscale=t=bt709:m=bt709:r=pc,format=gbrp,format=rgb24";

/// SDR reference white for SDR content placed in PQ/HLG video (BT.2408).
pub const SDR_WHITE_NITS: u32 = 203;

/// `-vf` filter that turns decoded frames of `stream` into display RGB24, or
/// `None` when the default conversion (SDR) is right.
pub fn display_filter(stream: &Stream) -> Option<&'static str> {
    stream.is_hdr().then_some(TONEMAP_TO_SDR_RGB)
}

type CacheKey = (PathBuf, u64, Option<SystemTime>);

/// [`display_filter`] for the first playable video stream of `path`,
/// remembered per file (path, size and modification time) so repeated frame
/// extractions do not probe again. Unreadable files: `None`.
pub fn display_filter_for(path: &Path) -> Option<&'static str> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Option<&'static str>>>> = OnceLock::new();
    let meta = std::fs::metadata(path).ok();
    let key: CacheKey = (
        path.to_path_buf(),
        meta.as_ref().map_or(0, std::fs::Metadata::len),
        meta.and_then(|m| m.modified().ok()),
    );
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return hit;
    }
    let filter = probe::probe(path)
        .ok()
        .and_then(|info| info.first_playable_video().and_then(display_filter));
    if let Ok(mut c) = cache.lock() {
        if c.len() > 256 {
            c.clear();
        }
        c.insert(key, filter);
    }
    filter
}

/// The colour description a preview frame must carry to match its source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetColor {
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    pub space: Option<String>,
    pub range: Option<String>,
    pub chroma_location: Option<String>,
}

impl TargetColor {
    pub fn of(stream: &Stream) -> Self {
        let own = |v: Option<&str>| v.map(str::to_owned);
        Self {
            primaries: own(stream.color_primaries()),
            transfer: own(stream.color_transfer()),
            space: own(stream.color_space()),
            range: own(stream.color_range()),
            chroma_location: own(stream.chroma_location()),
        }
    }

    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer.as_deref(), Some("smpte2084" | "arib-std-b67"))
    }

    /// ffmpeg encoder options tagging the output (`-color_primaries`…).
    pub fn encoder_args(&self) -> Vec<String> {
        self.encoder_args_for("")
    }

    /// [`encoder_args`](Self::encoder_args) with a stream specifier suffix
    /// (`":v:0"`).
    pub fn encoder_args_for(&self, specifier: &str) -> Vec<String> {
        let mut args = Vec::new();
        for (flag, value) in [
            ("-color_primaries", &self.primaries),
            ("-color_trc", &self.transfer),
            ("-colorspace", &self.space),
            ("-color_range", &self.range),
            ("-chroma_sample_location", &self.chroma_location),
        ] {
            if let Some(value) = value {
                args.push(format!("{flag}{specifier}"));
                args.push(value.clone());
            }
        }
        args
    }

    /// Whether `stream` reports the same primaries, transfer, matrix and
    /// range (the properties players act on).
    pub fn matches(&self, stream: &Stream) -> bool {
        let same = |want: &Option<String>, got: Option<&str>| want.as_deref() == got;
        same(&self.primaries, stream.color_primaries())
            && same(&self.transfer, stream.color_transfer())
            && same(&self.space, stream.color_space())
            && same(&self.range, stream.color_range())
    }
}

/// `-vf` chain scaling the 8-bit RGB frame to `width`x`height` in the
/// target's colour space and pixel format (see the module docs). With
/// `color == None` (8-bit SDR sources) it is the original plain lanczos
/// scale, left to the encoder's default conversion.
pub fn preview_filter(
    width: u32,
    height: u32,
    pix_fmt: &str,
    color: Option<&TargetColor>,
) -> String {
    let Some(color) = color else {
        return format!("scale={width}:{height}:flags=lanczos");
    };
    if !color.is_hdr() && color.space.is_none() {
        // Unknown matrix: nothing to match, keep swscale's default.
        return format!("scale={width}:{height}:flags=lanczos,format={pix_fmt}");
    }
    let mut z = format!("zscale=w={width}:h={height}:f=lanczos");
    if color.is_hdr() {
        z.push_str(":tin=bt709:pin=bt709:rin=pc");
        for (key, value) in [("t", &color.transfer), ("p", &color.primaries)] {
            if let Some(v) = value {
                z.push_str(&format!(":{key}={v}"));
            }
        }
        z.push_str(&format!(":npl={SDR_WHITE_NITS}"));
    }
    for (key, value) in [
        ("m", &color.space),
        ("r", &color.range),
        ("c", &color.chroma_location),
    ] {
        if let Some(v) = value {
            z.push_str(&format!(":{key}={v}"));
        }
    }
    format!("{z},format={pix_fmt}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_filters() {
        assert_eq!(
            preview_filter(320, 240, "yuv420p", None),
            "scale=320:240:flags=lanczos"
        );
        let pq = TargetColor {
            primaries: Some("bt2020".into()),
            transfer: Some("smpte2084".into()),
            space: Some("bt2020nc".into()),
            range: Some("tv".into()),
            chroma_location: Some("left".into()),
        };
        assert_eq!(
            preview_filter(64, 48, "yuv420p10le", Some(&pq)),
            "zscale=w=64:h=48:f=lanczos:tin=bt709:pin=bt709:rin=pc:t=smpte2084:p=bt2020:npl=203:\
             m=bt2020nc:r=tv:c=left,format=yuv420p10le"
        );
        let sdr10 = TargetColor {
            space: Some("bt709".into()),
            range: Some("tv".into()),
            ..TargetColor::default()
        };
        assert_eq!(
            preview_filter(64, 48, "yuv420p10le", Some(&sdr10)),
            "zscale=w=64:h=48:f=lanczos:m=bt709:r=tv,format=yuv420p10le"
        );
        assert_eq!(
            pq.encoder_args().join(" "),
            "-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc -color_range tv \
             -chroma_sample_location left"
        );
    }
}
