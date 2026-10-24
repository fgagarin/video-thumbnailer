//! Per-sample codec configuration for the HEVC concat fast path.
//!
//! The concat demuxer stream-copies the source behind the one-frame preview,
//! but the output track gets a single sample entry — the *preview's* `hvcC`
//! (VPS/SPS/PPS from x265). HEVC source packets carry no parameter sets of
//! their own in `hvc1` files, so a decoder would decode the whole source with
//! the preview's SPS/PPS and produce garbage. (H.264 does not have this
//! problem: the concat demuxer converts it to Annex B, which repeats each
//! file's SPS/PPS in band.)
//!
//! ISO/IEC 14496-12 allows a track to switch sample descriptions per chunk
//! (`stsc.sample_description_index`). After the concat run, the video track
//! is rewritten so that
//!
//! * sample description 1 is the source's own sample entry, copied byte for
//!   byte (`hvc1`/`hev1` box with its `hvcC`, `colr`, `pasp`, `mdcv`, `clli`…
//!   children), used by every source sample;
//! * sample description 2 is the preview's entry, used only by the first
//!   sample, which is split off into its own chunk when needed.
//!
//! So the source keeps its exact parameter sets and container colour/HDR
//! metadata, the packets stay untouched, and `hvc1` stays conformant (all
//! parameter sets in the sample entries, none in band). FFmpeg's mov demuxer
//! signals the switch to the decoder as new extradata; QuickTime/AVFoundation
//! create a new format description per sample description.
//!
//! Only files whose `moov` box comes after the media data are handled (what
//! ffmpeg writes without `+faststart`): the `moov` box is rebuilt in place and
//! no chunk offset into `mdat` moves.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{Error, Result};

fn err(msg: impl Into<String>) -> Error {
    Error::Other(format!("MP4 sample entry rewrite: {}", msg.into()))
}

/// A box inside a payload: `[start, end)` of the whole box and its header length.
#[derive(Debug, Clone, Copy)]
struct BoxRef {
    kind: [u8; 4],
    start: usize,
    header: usize,
    end: usize,
}

impl BoxRef {
    fn payload<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        &data[self.start + self.header..self.end]
    }
}

fn be32(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes(b.try_into().expect("4 bytes")))
        .ok_or_else(|| err("truncated box"))
}

fn be64(data: &[u8], at: usize) -> Result<u64> {
    data.get(at..at + 8)
        .map(|b| u64::from_be_bytes(b.try_into().expect("8 bytes")))
        .ok_or_else(|| err("truncated box"))
}

/// The boxes directly inside `data`.
fn children(data: &[u8]) -> Result<Vec<BoxRef>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let size32 = be32(data, at)? as u64;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().expect("4 bytes");
        let (size, header) = match size32 {
            0 => ((data.len() - at) as u64, 8),
            1 => (be64(data, at + 8)?, 16),
            n => (n, 8),
        };
        let end = at as u64 + size;
        if size < header as u64 || end > data.len() as u64 {
            return Err(err(format!(
                "box '{}' overruns its parent",
                String::from_utf8_lossy(&kind)
            )));
        }
        out.push(BoxRef {
            kind,
            start: at,
            header,
            end: end as usize,
        });
        at = end as usize;
    }
    Ok(out)
}

fn find(data: &[u8], kind: &[u8; 4]) -> Result<Option<BoxRef>> {
    Ok(children(data)?.into_iter().find(|b| &b.kind == kind))
}

fn require(data: &[u8], kind: &[u8; 4]) -> Result<BoxRef> {
    find(data, kind)?.ok_or_else(|| err(format!("no '{}' box", String::from_utf8_lossy(kind))))
}

/// Serialise a box with a fresh 32-bit (or 64-bit) header.
fn make_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    let size = payload.len() as u64 + 8;
    if size > u32::MAX as u64 {
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(&(size + 8).to_be_bytes());
    } else {
        out.extend_from_slice(&(size as u32).to_be_bytes());
        out.extend_from_slice(kind);
    }
    out.extend_from_slice(payload);
    out
}

/// Rebuild container payload `data` with the child `kind` replaced by `f(its payload)`.
fn replace_child(
    data: &[u8],
    kind: &[u8; 4],
    f: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let target = require(data, kind)?;
    let mut out = Vec::with_capacity(data.len() + 256);
    out.extend_from_slice(&data[..target.start]);
    out.extend_from_slice(&make_box(kind, &f(target.payload(data))?));
    out.extend_from_slice(&data[target.end..]);
    Ok(out)
}

/// `track_ID` from a `tkhd` payload.
fn track_id(tkhd: &[u8]) -> Result<u32> {
    let version = *tkhd.first().ok_or_else(|| err("empty tkhd"))?;
    be32(tkhd, if version == 1 { 20 } else { 12 })
}

/// Index (among `moov`'s children) of the `trak` with `id`.
fn trak_with_id(moov: &[u8], id: u32) -> Result<BoxRef> {
    for child in children(moov)? {
        if &child.kind != b"trak" {
            continue;
        }
        let trak = child.payload(moov);
        if track_id(require(trak, b"tkhd")?.payload(trak))? == id {
            return Ok(child);
        }
    }
    Err(err(format!("no track with ID {id}")))
}

/// Payload of `trak/mdia/minf/stbl`.
fn stbl_of(trak: &[u8]) -> Result<&[u8]> {
    let mdia = require(trak, b"mdia")?.payload(trak);
    let minf = require(mdia, b"minf")?.payload(mdia);
    Ok(require(minf, b"stbl")?.payload(minf))
}

/// Sample entries of an `stsd` payload.
fn stsd_entries(stsd: &[u8]) -> Result<Vec<&[u8]>> {
    let count = be32(stsd, 4)? as usize;
    let entries = children(stsd.get(8..).ok_or_else(|| err("truncated stsd"))?)?;
    if entries.len() != count {
        return Err(err("stsd entry count mismatch"));
    }
    Ok(entries
        .iter()
        .map(|b| &stsd[8 + b.start..8 + b.end])
        .collect())
}

/// Locate the `moov` box of a file: `(offset, size)`. Fails unless every box
/// after it is free space or metadata, i.e. no media data follows.
fn locate_moov(file: &mut std::fs::File) -> Result<(u64, u64)> {
    let len = file.metadata()?.len();
    let mut at = 0u64;
    let mut moov = None;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 16];
        let n = file.read(&mut header)?;
        if n < 8 {
            break;
        }
        let size32 = u32::from_be_bytes(header[..4].try_into().expect("4 bytes")) as u64;
        let kind: [u8; 4] = header[4..8].try_into().expect("4 bytes");
        let size = match size32 {
            0 => len - at,
            1 if n >= 16 => u64::from_be_bytes(header[8..16].try_into().expect("8 bytes")),
            1 => return Err(err("truncated box header")),
            n => n,
        };
        if size < 8 || at + size > len {
            return Err(err("top-level box overruns the file"));
        }
        match &kind {
            b"moov" => moov = Some((at, size)),
            b"mdat" | b"moof" if moov.is_some() => {
                return Err(err("media data follows the moov box"));
            }
            _ => {}
        }
        at += size;
    }
    moov.ok_or_else(|| err("no moov box"))
}

fn read_moov(path: &Path) -> Result<(std::fs::File, u64, Vec<u8>)> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let (offset, size) = locate_moov(&mut file)?;
    let mut moov = vec![0u8; size as usize];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut moov)?;
    Ok((file, offset, moov))
}

/// The first sample entry (e.g. the whole `hvc1` box) of track `track_id`.
pub(crate) fn first_sample_entry(path: &Path, track_id: u32) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    // Unlike the rewrite, reading works wherever the moov box sits.
    let mut at = 0u64;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 16];
        let n = file.read(&mut header)?;
        let size32 = u32::from_be_bytes(header[..4].try_into().expect("4 bytes")) as u64;
        let size = match size32 {
            0 => len - at,
            1 if n >= 16 => u64::from_be_bytes(header[8..16].try_into().expect("8 bytes")),
            1 => return Err(err("truncated box header")),
            n => n,
        };
        if size < 8 || at + size > len {
            return Err(err("top-level box overruns the file"));
        }
        if &header[4..8] == b"moov" {
            let mut moov = vec![0u8; size as usize];
            file.seek(SeekFrom::Start(at))?;
            file.read_exact(&mut moov)?;
            let header = children(&moov)?[0].header;
            let moov = &moov[header..];
            let trak = trak_with_id(moov, track_id)?;
            let stbl = stbl_of(trak.payload(moov))?;
            let stsd = require(stbl, b"stsd")?.payload(stbl);
            return stsd_entries(stsd)?
                .first()
                .map(|e| e.to_vec())
                .ok_or_else(|| err("empty stsd"));
        }
        at += size;
    }
    Err(err("no moov box"))
}

/// Per-chunk `(samples, sample description index)`, expanded from `stsc`.
fn expand_stsc(stsc: &[u8], chunks: usize) -> Result<Vec<(u32, u32)>> {
    let count = be32(stsc, 4)? as usize;
    let mut runs = Vec::with_capacity(count);
    for i in 0..count {
        let at = 8 + 12 * i;
        runs.push((be32(stsc, at)?, be32(stsc, at + 4)?, be32(stsc, at + 8)?));
    }
    let mut out = Vec::with_capacity(chunks);
    for (i, &(first, samples, sdi)) in runs.iter().enumerate() {
        let next = runs.get(i + 1).map_or(chunks as u64 + 1, |r| r.0 as u64);
        if first == 0 || (first as u64) > next {
            return Err(err("invalid stsc"));
        }
        for _ in first as u64..next {
            out.push((samples, sdi));
        }
    }
    if out.len() != chunks {
        return Err(err("stsc does not cover every chunk"));
    }
    Ok(out)
}

fn build_stsc(chunks: &[(u32, u32)]) -> Vec<u8> {
    let mut runs: Vec<(u32, u32, u32)> = Vec::new();
    for (i, &(samples, sdi)) in chunks.iter().enumerate() {
        if runs.last().is_none_or(|r| (r.1, r.2) != (samples, sdi)) {
            runs.push((i as u32 + 1, samples, sdi));
        }
    }
    let mut out = vec![0u8; 4];
    out.extend_from_slice(&(runs.len() as u32).to_be_bytes());
    for (first, samples, sdi) in runs {
        out.extend_from_slice(&first.to_be_bytes());
        out.extend_from_slice(&samples.to_be_bytes());
        out.extend_from_slice(&sdi.to_be_bytes());
    }
    out
}

/// Chunk offsets from `stco` or `co64`, and whether they were 64-bit.
fn chunk_offsets(stbl: &[u8]) -> Result<(Vec<u64>, bool)> {
    if let Some(stco) = find(stbl, b"stco")? {
        let p = stco.payload(stbl);
        let n = be32(p, 4)? as usize;
        let offsets = (0..n)
            .map(|i| be32(p, 8 + 4 * i).map(u64::from))
            .collect::<Result<_>>()?;
        return Ok((offsets, false));
    }
    let co64 = require(stbl, b"co64")?;
    let p = co64.payload(stbl);
    let n = be32(p, 4)? as usize;
    let offsets = (0..n).map(|i| be64(p, 8 + 8 * i)).collect::<Result<_>>()?;
    Ok((offsets, true))
}

fn first_sample_size(stbl: &[u8]) -> Result<u64> {
    let stsz = require(stbl, b"stsz")?;
    let p = stsz.payload(stbl);
    match be32(p, 4)? {
        0 => be32(p, 12).map(u64::from),
        fixed => Ok(fixed as u64),
    }
}

/// The new `stbl` payload: `entry` becomes sample description 1 for every
/// sample but the first, which keeps the track's current (single) entry.
fn rewrite_stbl(stbl: &[u8], entry: &[u8]) -> Result<Vec<u8>> {
    let stsd = require(stbl, b"stsd")?.payload(stbl);
    let existing = stsd_entries(stsd)?;
    if existing.len() != 1 {
        return Err(err(format!(
            "expected one sample entry, found {}",
            existing.len()
        )));
    }
    let (mut offsets, wide) = chunk_offsets(stbl)?;
    let stsc = require(stbl, b"stsc")?.payload(stbl);
    let mut chunks = expand_stsc(stsc, offsets.len())?;
    let Some(&(first_samples, _)) = chunks.first() else {
        return Err(err("track has no chunks"));
    };
    // Every source sample: new entry 1; the preview: old entry, now 2.
    for chunk in &mut chunks {
        chunk.1 = 1;
    }
    if first_samples > 1 {
        let split = offsets[0] + first_sample_size(stbl)?;
        offsets.insert(1, split);
        chunks.insert(1, (first_samples - 1, 1));
    }
    chunks[0] = (1, 2);

    let mut new_stsd = stsd[..4].to_vec();
    new_stsd.extend_from_slice(&2u32.to_be_bytes());
    new_stsd.extend_from_slice(entry);
    new_stsd.extend_from_slice(existing[0]);

    let wide = wide || offsets.iter().any(|&o| o > u32::MAX as u64);
    let mut new_offsets = vec![0u8; 4];
    new_offsets.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for o in &offsets {
        if wide {
            new_offsets.extend_from_slice(&o.to_be_bytes());
        } else {
            new_offsets.extend_from_slice(&(*o as u32).to_be_bytes());
        }
    }

    let mut out = Vec::with_capacity(stbl.len() + entry.len() + 64);
    for child in children(stbl)? {
        match &child.kind {
            b"stsd" => out.extend_from_slice(&make_box(b"stsd", &new_stsd)),
            b"stsc" => out.extend_from_slice(&make_box(b"stsc", &build_stsc(&chunks))),
            b"stco" | b"co64" => out.extend_from_slice(&make_box(
                if wide { b"co64" } else { b"stco" },
                &new_offsets,
            )),
            _ => out.extend_from_slice(&stbl[child.start..child.end]),
        }
    }
    Ok(out)
}

/// Make `entry` (a complete sample entry box, e.g. the source's `hvc1`) the
/// sample description of every sample of track `track_id` but the first,
/// which keeps the track's current entry (see the module docs).
pub(crate) fn use_entry_after_first_sample(path: &Path, track_id: u32, entry: &[u8]) -> Result<()> {
    let (mut file, offset, moov_box) = read_moov(path)?;
    let header = children(&moov_box)?[0].header;
    let moov = &moov_box[header..];
    let trak = trak_with_id(moov, track_id)?;

    let new_trak = replace_child(trak.payload(moov), b"mdia", |mdia| {
        replace_child(mdia, b"minf", |minf| {
            replace_child(minf, b"stbl", |stbl| rewrite_stbl(stbl, entry))
        })
    })?;
    let mut new_moov = Vec::with_capacity(moov.len() + entry.len() + 64);
    new_moov.extend_from_slice(&moov[..trak.start]);
    new_moov.extend_from_slice(&make_box(b"trak", &new_trak));
    new_moov.extend_from_slice(&moov[trak.end..]);
    let new_moov = make_box(b"moov", &new_moov);

    // Keep whatever follows moov (free space, metadata) after the new one.
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(offset + moov_box.len() as u64))?;
    file.read_to_end(&mut tail)?;
    file.set_len(offset)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&new_moov)?;
    file.write_all(&tail)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; 4];
        p.extend_from_slice(body);
        make_box(kind, &p)
    }

    fn u32s(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    fn stbl(samples_per_chunk: u32, chunks: u32) -> Vec<u8> {
        let entry = make_box(b"hvc1", b"preview");
        let mut stsd = u32s(&[1]);
        stsd.extend_from_slice(&entry);
        let mut out = full_box(b"stsd", &stsd);
        out.extend(full_box(b"stsc", &u32s(&[1, 1, samples_per_chunk, 1])));
        let total = samples_per_chunk * chunks;
        let mut stsz = u32s(&[0, total]);
        stsz.extend(u32s(&vec![100; total as usize]));
        out.extend(full_box(b"stsz", &stsz));
        let mut stco = u32s(&[chunks]);
        stco.extend(u32s(
            &(0..chunks)
                .map(|c| 1000 + c * 100 * samples_per_chunk)
                .collect::<Vec<_>>(),
        ));
        out.extend(full_box(b"stco", &stco));
        out
    }

    #[test]
    fn splits_the_first_chunk() {
        let new = rewrite_stbl(&stbl(3, 2), &make_box(b"hvc1", b"source")).unwrap();
        let stsd = require(&new, b"stsd").unwrap().payload(&new).to_vec();
        let entries = stsd_entries(&stsd).unwrap();
        assert_eq!(entries[0], make_box(b"hvc1", b"source"));
        assert_eq!(entries[1], make_box(b"hvc1", b"preview"));
        let (offsets, wide) = chunk_offsets(&new).unwrap();
        assert!(!wide);
        assert_eq!(offsets, vec![1000, 1100, 1300]);
        let stsc = require(&new, b"stsc").unwrap().payload(&new).to_vec();
        assert_eq!(expand_stsc(&stsc, 3).unwrap(), vec![(1, 2), (2, 1), (3, 1)]);
    }

    #[test]
    fn single_sample_first_chunk_is_relabelled_only() {
        let new = rewrite_stbl(&stbl(1, 3), &make_box(b"hvc1", b"source")).unwrap();
        let (offsets, _) = chunk_offsets(&new).unwrap();
        assert_eq!(offsets, vec![1000, 1100, 1200]);
        let stsc = require(&new, b"stsc").unwrap().payload(&new).to_vec();
        assert_eq!(u32s(&[0, 2, 1, 1, 2, 2, 1, 1]), stsc);
    }

    #[test]
    fn rejects_multiple_entries() {
        let mut s = stbl(1, 2);
        // Bump the entry count to 2 with a second entry.
        let stsd_box = require(&s, b"stsd").unwrap();
        let mut stsd = u32s(&[0, 2]);
        stsd.extend(make_box(b"hvc1", b"a"));
        stsd.extend(make_box(b"hvc1", b"b"));
        s.splice(stsd_box.start..stsd_box.end, make_box(b"stsd", &stsd));
        assert!(rewrite_stbl(&s, b"x").is_err());
    }
}
