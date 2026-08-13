//! Bounded ISO Base Media File Format parsing for fragmented clear content.
//!
//! The parser implements the subset required by MSE-style `ftyp`/`moov`
//! initialization segments and `moof`/`mdat` media segments. It validates box
//! sizes, track timescales, sample counts, timestamps and payload boundaries.

use crate::media_core::{EncodedSample, MediaCodec};

const MAX_BOXES: usize = 4_096;
const MAX_TRACKS: usize = 32;
const MAX_SAMPLES_PER_SEGMENT: usize = 100_000;
const MAX_SEGMENT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Video,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub track_id: u32,
    pub timescale: u32,
    pub kind: TrackKind,
    pub codec: MediaCodec,
}

#[derive(Debug, Clone, Copy)]
struct Mp4Box<'a> {
    kind: [u8; 4],
    payload: &'a [u8],
}

#[derive(Debug, Clone, Copy, Default)]
struct TrackFragmentDefaults {
    track_id: u32,
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Debug, Clone, Copy)]
struct RunSample {
    duration: u32,
    size: u32,
    flags: u32,
    composition_offset: i64,
}

#[derive(Debug, Clone)]
struct TrackRun {
    track_id: u32,
    base_decode_time: u64,
    samples: Vec<RunSample>,
}

pub fn parse_init_segment(bytes: &[u8]) -> Result<Vec<TrackInfo>, String> {
    validate_segment_size(bytes)?;
    let top = parse_boxes(bytes)?;
    if !top.iter().any(|item| item.kind == *b"ftyp") {
        return Err("ISO-BMFF initialization segment has no ftyp box".to_string());
    }
    let moov = top
        .iter()
        .find(|item| item.kind == *b"moov")
        .ok_or_else(|| "ISO-BMFF initialization segment has no moov box".to_string())?;
    let mut tracks = Vec::new();
    for item in parse_boxes(moov.payload)? {
        if item.kind != *b"trak" {
            continue;
        }
        if tracks.len() >= MAX_TRACKS {
            return Err("ISO-BMFF track budget exceeded".to_string());
        }
        tracks.push(parse_track(item.payload)?);
    }
    if tracks.is_empty() {
        return Err("ISO-BMFF initialization segment has no tracks".to_string());
    }
    tracks.sort_by_key(|track| track.track_id);
    if tracks
        .windows(2)
        .any(|pair| pair[0].track_id == pair[1].track_id)
    {
        return Err("ISO-BMFF initialization segment has duplicate track ids".to_string());
    }
    Ok(tracks)
}

pub fn parse_media_segment(
    bytes: &[u8],
    tracks: &[TrackInfo],
) -> Result<Vec<EncodedSample>, String> {
    validate_segment_size(bytes)?;
    if tracks.is_empty() || tracks.len() > MAX_TRACKS {
        return Err("ISO-BMFF media segment has no valid track configuration".to_string());
    }
    let top = parse_boxes(bytes)?;
    let moof = top
        .iter()
        .find(|item| item.kind == *b"moof")
        .ok_or_else(|| "ISO-BMFF media segment has no moof box".to_string())?;
    let mdat = top
        .iter()
        .find(|item| item.kind == *b"mdat")
        .ok_or_else(|| "ISO-BMFF media segment has no mdat box".to_string())?;
    let runs = parse_movie_fragment(moof.payload)?;
    let sample_count = runs.iter().map(|run| run.samples.len()).sum::<usize>();
    if sample_count == 0 || sample_count > MAX_SAMPLES_PER_SEGMENT {
        return Err("ISO-BMFF sample count budget exceeded".to_string());
    }
    let mut payload_offset = 0usize;
    let mut output = Vec::with_capacity(sample_count);
    for run in runs {
        let track = tracks
            .iter()
            .find(|track| track.track_id == run.track_id)
            .ok_or_else(|| "ISO-BMFF media segment references an unknown track".to_string())?;
        let mut decode_time = run.base_decode_time;
        for sample in run.samples {
            if sample.duration == 0 || sample.size == 0 {
                return Err("ISO-BMFF sample has no duration or payload".to_string());
            }
            let sample_size = sample.size as usize;
            let end = payload_offset
                .checked_add(sample_size)
                .ok_or_else(|| "ISO-BMFF sample payload offset overflow".to_string())?;
            let data = mdat
                .payload
                .get(payload_offset..end)
                .ok_or_else(|| "ISO-BMFF sample exceeds the mdat payload".to_string())?
                .to_vec();
            let dts = scale_time(decode_time as i128, track.timescale)?;
            let presentation = (decode_time as i128)
                .checked_add(sample.composition_offset as i128)
                .ok_or_else(|| "ISO-BMFF composition timestamp overflow".to_string())?;
            let pts = scale_time(presentation, track.timescale)?;
            let duration = scale_duration(sample.duration, track.timescale)?;
            output.push(EncodedSample {
                track_id: run.track_id,
                decode_timestamp_us: dts,
                presentation_timestamp_us: pts,
                duration_us: duration,
                keyframe: sample.flags & 0x0001_0000 == 0,
                data,
            });
            payload_offset = end;
            decode_time = decode_time
                .checked_add(u64::from(sample.duration))
                .ok_or_else(|| "ISO-BMFF decode timestamp overflow".to_string())?;
        }
    }
    if payload_offset != mdat.payload.len() {
        return Err("ISO-BMFF mdat contains unreferenced payload bytes".to_string());
    }
    Ok(output)
}

fn parse_track(bytes: &[u8]) -> Result<TrackInfo, String> {
    let children = parse_boxes(bytes)?;
    let tkhd = children
        .iter()
        .find(|item| item.kind == *b"tkhd")
        .ok_or_else(|| "ISO-BMFF track has no tkhd box".to_string())?;
    let track_id = parse_tkhd_track_id(tkhd.payload)?;
    if track_id == 0 {
        return Err("ISO-BMFF track id cannot be zero".to_string());
    }
    let mdia = children
        .iter()
        .find(|item| item.kind == *b"mdia")
        .ok_or_else(|| "ISO-BMFF track has no mdia box".to_string())?;
    let media_children = parse_boxes(mdia.payload)?;
    let mdhd = media_children
        .iter()
        .find(|item| item.kind == *b"mdhd")
        .ok_or_else(|| "ISO-BMFF track has no mdhd box".to_string())?;
    let timescale = parse_mdhd_timescale(mdhd.payload)?;
    let handler = media_children
        .iter()
        .find(|item| item.kind == *b"hdlr")
        .ok_or_else(|| "ISO-BMFF track has no hdlr box".to_string())?;
    let kind = parse_handler_kind(handler.payload)?;
    let minf = media_children
        .iter()
        .find(|item| item.kind == *b"minf")
        .ok_or_else(|| "ISO-BMFF track has no minf box".to_string())?;
    let codec = parse_sample_entry_codec(minf.payload)?;
    Ok(TrackInfo {
        track_id,
        timescale,
        kind,
        codec,
    })
}

fn parse_sample_entry_codec(minf: &[u8]) -> Result<MediaCodec, String> {
    let stbl = parse_boxes(minf)?
        .into_iter()
        .find(|item| item.kind == *b"stbl")
        .ok_or_else(|| "ISO-BMFF track has no stbl box".to_string())?;
    let stsd = parse_boxes(stbl.payload)?
        .into_iter()
        .find(|item| item.kind == *b"stsd")
        .ok_or_else(|| "ISO-BMFF track has no stsd box".to_string())?;
    if stsd.payload.len() < 8 || read_be_u32(stsd.payload, 4)? == 0 {
        return Err("ISO-BMFF stsd has no sample entry".to_string());
    }
    let entry = parse_boxes(&stsd.payload[8..])?
        .into_iter()
        .next()
        .ok_or_else(|| "ISO-BMFF stsd sample entry is truncated".to_string())?;
    Ok(match &entry.kind {
        b"avc1" | b"avc3" => MediaCodec::Avc,
        b"hvc1" | b"hev1" => MediaCodec::Hevc,
        b"vp08" => MediaCodec::Vp8,
        b"vp09" => MediaCodec::Vp9,
        b"av01" => MediaCodec::Av1,
        b"mp4a" => MediaCodec::Aac,
        b"Opus" => MediaCodec::Opus,
        other => MediaCodec::Unknown(String::from_utf8_lossy(other).into_owned()),
    })
}

fn parse_movie_fragment(bytes: &[u8]) -> Result<Vec<TrackRun>, String> {
    let mut runs = Vec::new();
    for item in parse_boxes(bytes)? {
        if item.kind != *b"traf" {
            continue;
        }
        let children = parse_boxes(item.payload)?;
        let tfhd = children
            .iter()
            .find(|item| item.kind == *b"tfhd")
            .ok_or_else(|| "ISO-BMFF traf has no tfhd box".to_string())?;
        let defaults = parse_tfhd(tfhd.payload)?;
        let tfdt = children
            .iter()
            .find(|item| item.kind == *b"tfdt")
            .ok_or_else(|| "ISO-BMFF traf has no tfdt box".to_string())?;
        let base_decode_time = parse_tfdt(tfdt.payload)?;
        let mut next_time = base_decode_time;
        for trun in children.iter().filter(|item| item.kind == *b"trun") {
            let samples = parse_trun(trun.payload, defaults)?;
            let run_duration = samples
                .iter()
                .try_fold(0u64, |total, sample| {
                    total.checked_add(u64::from(sample.duration))
                })
                .ok_or_else(|| "ISO-BMFF run duration overflow".to_string())?;
            runs.push(TrackRun {
                track_id: defaults.track_id,
                base_decode_time: next_time,
                samples,
            });
            next_time = next_time
                .checked_add(run_duration)
                .ok_or_else(|| "ISO-BMFF run timestamp overflow".to_string())?;
        }
    }
    Ok(runs)
}

fn parse_tfhd(bytes: &[u8]) -> Result<TrackFragmentDefaults, String> {
    if bytes.len() < 8 {
        return Err("Truncated ISO-BMFF tfhd box".to_string());
    }
    let flags = read_u24(bytes, 1)?;
    let track_id = read_be_u32(bytes, 4)?;
    let mut offset = 8usize;
    if flags & 0x000001 != 0 {
        offset = checked_skip(bytes, offset, 8)?;
    }
    if flags & 0x000002 != 0 {
        offset = checked_skip(bytes, offset, 4)?;
    }
    let duration = if flags & 0x000008 != 0 {
        let value = read_be_u32(bytes, offset)?;
        offset += 4;
        value
    } else {
        0
    };
    let size = if flags & 0x000010 != 0 {
        let value = read_be_u32(bytes, offset)?;
        offset += 4;
        value
    } else {
        0
    };
    let sample_flags = if flags & 0x000020 != 0 {
        read_be_u32(bytes, offset)?
    } else {
        0
    };
    if track_id == 0 {
        return Err("ISO-BMFF tfhd track id cannot be zero".to_string());
    }
    Ok(TrackFragmentDefaults {
        track_id,
        duration,
        size,
        flags: sample_flags,
    })
}

fn parse_tfdt(bytes: &[u8]) -> Result<u64, String> {
    let version = *bytes
        .first()
        .ok_or_else(|| "Truncated ISO-BMFF tfdt box".to_string())?;
    if version == 1 {
        read_be_u64(bytes, 4)
    } else {
        Ok(u64::from(read_be_u32(bytes, 4)?))
    }
}

fn parse_trun(bytes: &[u8], defaults: TrackFragmentDefaults) -> Result<Vec<RunSample>, String> {
    if bytes.len() < 8 {
        return Err("Truncated ISO-BMFF trun box".to_string());
    }
    let version = bytes[0];
    let flags = read_u24(bytes, 1)?;
    let count = read_be_u32(bytes, 4)? as usize;
    if count == 0 || count > MAX_SAMPLES_PER_SEGMENT {
        return Err("ISO-BMFF trun sample count budget exceeded".to_string());
    }
    let mut offset = 8usize;
    if flags & 0x000001 != 0 {
        offset = checked_skip(bytes, offset, 4)?;
    }
    let first_sample_flags = if flags & 0x000004 != 0 {
        let value = read_be_u32(bytes, offset)?;
        offset += 4;
        Some(value)
    } else {
        None
    };
    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        let duration = if flags & 0x000100 != 0 {
            let value = read_be_u32(bytes, offset)?;
            offset += 4;
            value
        } else {
            defaults.duration
        };
        let size = if flags & 0x000200 != 0 {
            let value = read_be_u32(bytes, offset)?;
            offset += 4;
            value
        } else {
            defaults.size
        };
        let sample_flags = if flags & 0x000400 != 0 {
            let value = read_be_u32(bytes, offset)?;
            offset += 4;
            value
        } else if index == 0 {
            first_sample_flags.unwrap_or(defaults.flags)
        } else {
            defaults.flags
        };
        let composition_offset = if flags & 0x000800 != 0 {
            let raw = read_be_u32(bytes, offset)?;
            offset += 4;
            if version == 1 {
                i64::from(raw as i32)
            } else {
                i64::from(raw)
            }
        } else {
            0
        };
        samples.push(RunSample {
            duration,
            size,
            flags: sample_flags,
            composition_offset,
        });
    }
    if offset > bytes.len() {
        return Err("Truncated ISO-BMFF trun sample table".to_string());
    }
    Ok(samples)
}

fn parse_tkhd_track_id(bytes: &[u8]) -> Result<u32, String> {
    let version = *bytes
        .first()
        .ok_or_else(|| "Truncated ISO-BMFF tkhd box".to_string())?;
    read_be_u32(bytes, if version == 1 { 20 } else { 12 })
}

fn parse_mdhd_timescale(bytes: &[u8]) -> Result<u32, String> {
    let version = *bytes
        .first()
        .ok_or_else(|| "Truncated ISO-BMFF mdhd box".to_string())?;
    let timescale = read_be_u32(bytes, if version == 1 { 20 } else { 12 })?;
    if timescale == 0 || timescale > 10_000_000 {
        return Err("ISO-BMFF track timescale is invalid".to_string());
    }
    Ok(timescale)
}

fn parse_handler_kind(bytes: &[u8]) -> Result<TrackKind, String> {
    let handler = bytes
        .get(8..12)
        .ok_or_else(|| "Truncated ISO-BMFF hdlr box".to_string())?;
    Ok(match handler {
        b"soun" => TrackKind::Audio,
        b"vide" => TrackKind::Video,
        _ => TrackKind::Unknown,
    })
}

fn parse_boxes(bytes: &[u8]) -> Result<Vec<Mp4Box<'_>>, String> {
    let mut boxes = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        if boxes.len() >= MAX_BOXES {
            return Err("ISO-BMFF box count budget exceeded".to_string());
        }
        let basic = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| "Truncated ISO-BMFF box header".to_string())?;
        let size32 = u32::from_be_bytes([basic[0], basic[1], basic[2], basic[3]]);
        let kind = [basic[4], basic[5], basic[6], basic[7]];
        let (header, size) = if size32 == 1 {
            (
                16usize,
                usize::try_from(read_be_u64(bytes, offset + 8)?)
                    .map_err(|_| "ISO-BMFF extended box size exceeds address space".to_string())?,
            )
        } else if size32 == 0 {
            (8usize, bytes.len() - offset)
        } else {
            (8usize, size32 as usize)
        };
        if size < header {
            return Err("ISO-BMFF box size is smaller than its header".to_string());
        }
        let end = offset
            .checked_add(size)
            .ok_or_else(|| "ISO-BMFF box size overflow".to_string())?;
        let payload = bytes
            .get(offset + header..end)
            .ok_or_else(|| "ISO-BMFF box exceeds segment boundaries".to_string())?;
        boxes.push(Mp4Box { kind, payload });
        offset = end;
    }
    Ok(boxes)
}

fn checked_skip(bytes: &[u8], offset: usize, length: usize) -> Result<usize, String> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "ISO-BMFF field offset overflow".to_string())?;
    if end > bytes.len() {
        return Err("Truncated ISO-BMFF optional field".to_string());
    }
    Ok(end)
}

fn scale_time(value: i128, timescale: u32) -> Result<i64, String> {
    let scaled = value
        .checked_mul(1_000_000)
        .ok_or_else(|| "ISO-BMFF timestamp scale overflow".to_string())?
        / i128::from(timescale);
    i64::try_from(scaled).map_err(|_| "ISO-BMFF timestamp exceeds supported range".to_string())
}

fn scale_duration(value: u32, timescale: u32) -> Result<u64, String> {
    let scaled = u64::from(value)
        .checked_mul(1_000_000)
        .ok_or_else(|| "ISO-BMFF duration scale overflow".to_string())?
        / u64::from(timescale);
    if scaled == 0 {
        return Err("ISO-BMFF sample duration rounds to zero".to_string());
    }
    Ok(scaled)
}

fn validate_segment_size(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > MAX_SEGMENT_BYTES {
        return Err("ISO-BMFF segment size budget exceeded".to_string());
    }
    Ok(())
}

fn read_u24(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset.saturating_add(3))
        .ok_or_else(|| "Truncated ISO-BMFF u24".to_string())?;
    Ok(u32::from_be_bytes([0, value[0], value[1], value[2]]))
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| "Truncated ISO-BMFF u32".to_string())?;
    Ok(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_be_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let value = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| "Truncated ISO-BMFF u64".to_string())?;
    Ok(u64::from_be_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(payload);
        output
    }

    fn full_box(version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut output = vec![
            version,
            (flags >> 16) as u8,
            (flags >> 8) as u8,
            flags as u8,
        ];
        output.extend_from_slice(body);
        output
    }

    pub fn init(track_id: u32, timescale: u32, handler: &[u8; 4], codec: &[u8; 4]) -> Vec<u8> {
        let mut tkhd_body = vec![0; 8];
        tkhd_body.extend_from_slice(&track_id.to_be_bytes());
        tkhd_body.extend_from_slice(&[0; 4]);
        let tkhd = boxed(b"tkhd", &full_box(0, 0, &tkhd_body));

        let mut mdhd_body = vec![0; 8];
        mdhd_body.extend_from_slice(&timescale.to_be_bytes());
        mdhd_body.extend_from_slice(&0u32.to_be_bytes());
        let mdhd = boxed(b"mdhd", &full_box(0, 0, &mdhd_body));

        let mut hdlr_body = vec![0; 4];
        hdlr_body.extend_from_slice(handler);
        let hdlr = boxed(b"hdlr", &full_box(0, 0, &hdlr_body));

        let sample_entry = boxed(codec, &[0; 8]);
        let mut stsd_body = 1u32.to_be_bytes().to_vec();
        stsd_body.extend_from_slice(&sample_entry);
        let stsd = boxed(b"stsd", &full_box(0, 0, &stsd_body));
        let stbl = boxed(b"stbl", &stsd);
        let minf = boxed(b"minf", &stbl);
        let mut mdia_payload = Vec::new();
        mdia_payload.extend_from_slice(&mdhd);
        mdia_payload.extend_from_slice(&hdlr);
        mdia_payload.extend_from_slice(&minf);
        let mdia = boxed(b"mdia", &mdia_payload);
        let mut trak_payload = tkhd;
        trak_payload.extend_from_slice(&mdia);
        let trak = boxed(b"trak", &trak_payload);
        let moov = boxed(b"moov", &trak);
        let mut ftyp_payload = b"isom".to_vec();
        ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
        ftyp_payload.extend_from_slice(b"isom");
        let mut output = boxed(b"ftyp", &ftyp_payload);
        output.extend_from_slice(&moov);
        output
    }

    pub fn media(track_id: u32, base_time: u64, duration: u32, payloads: &[&[u8]]) -> Vec<u8> {
        let mut tfhd_body = track_id.to_be_bytes().to_vec();
        tfhd_body.extend_from_slice(&duration.to_be_bytes());
        let tfhd = boxed(b"tfhd", &full_box(0, 0x000008, &tfhd_body));
        let tfdt = boxed(b"tfdt", &full_box(1, 0, &base_time.to_be_bytes()));
        let mut trun_body = (payloads.len() as u32).to_be_bytes().to_vec();
        for payload in payloads {
            trun_body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        }
        let trun = boxed(b"trun", &full_box(0, 0x000200, &trun_body));
        let mut traf_payload = tfhd;
        traf_payload.extend_from_slice(&tfdt);
        traf_payload.extend_from_slice(&trun);
        let traf = boxed(b"traf", &traf_payload);
        let moof = boxed(b"moof", &traf);
        let mdat_payload = payloads
            .iter()
            .flat_map(|payload| payload.iter().copied())
            .collect::<Vec<_>>();
        let mut output = moof;
        output.extend_from_slice(&boxed(b"mdat", &mdat_payload));
        output
    }

    #[test]
    fn fragmented_mp4_init_and_media_are_parsed_with_timestamps() {
        let init = init(7, 1_000, b"vide", b"avc1");
        let tracks = parse_init_segment(&init).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, MediaCodec::Avc);
        let media = media(7, 2_000, 40, &[b"one", b"two"]);
        let samples = parse_media_segment(&media, &tracks).unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].decode_timestamp_us, 2_000_000);
        assert_eq!(samples[1].decode_timestamp_us, 2_040_000);
        assert_eq!(samples[1].data, b"two");
    }
}
