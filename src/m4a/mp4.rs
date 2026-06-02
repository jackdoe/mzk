use crate::error::{Error, Result};

pub struct AlacConfig {
    pub frame_length: u32,
    pub bit_depth: u8,
    pub pb: u8,
    pub mb: u8,
    pub kb: u8,
    pub num_channels: u8,
    pub sample_rate: u32,
}

pub struct AacConfig {
    pub audio_object_type: u32,
    pub sample_rate: u32,
    pub channels: usize,
    pub frame_length: usize,
}

pub enum Codec {
    Alac(AlacConfig),
    Aac(AacConfig),
}

const AAC_SAMPLE_RATES: [u32; 16] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350, 0, 0,
    0,
];

fn read_desc_len(d: &[u8], mut pos: usize) -> (usize, usize) {
    let mut len = 0usize;
    for _ in 0..4 {
        let b = *d.get(pos).unwrap_or(&0);
        pos += 1;
        len = (len << 7) | (b & 0x7f) as usize;
        if b & 0x80 == 0 {
            break;
        }
    }
    (len, pos)
}

fn find_asc(d: &[u8], start: usize, end: usize, depth: u32) -> Option<(usize, usize)> {
    if depth > 32 {
        return None;
    }
    let end = end.min(d.len());
    let mut pos = start;
    while pos + 2 <= end {
        let tag = d[pos];
        let (len, body) = read_desc_len(d, pos + 1);
        let next = body + len;
        if next > end || next <= pos {
            return None;
        }
        match tag {
            0x05 => return Some((body, next)),
            0x03 => {
                if body + 3 > end {
                    return None;
                }
                let flags = d[body + 2];
                let mut p = body + 3;
                if flags & 0x80 != 0 {
                    p += 2;
                }
                if flags & 0x40 != 0 {
                    if p >= end {
                        return None;
                    }
                    p += 1 + d[p] as usize;
                }
                if flags & 0x20 != 0 {
                    p += 2;
                }
                if p <= end {
                    if let Some(r) = find_asc(d, p, next, depth + 1) {
                        return Some(r);
                    }
                }
            }
            0x04 => {
                if let Some(r) = find_asc(d, body + 13, next, depth + 1) {
                    return Some(r);
                }
            }
            _ => {}
        }
        pos = next;
    }
    None
}

fn parse_asc(d: &[u8], start: usize, end: usize, sd_channels: usize) -> AacConfig {
    let mut pos = start * 8;
    let mut rd = |n: u32| -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = pos >> 3;
            let bit = if byte < end {
                (d[byte] >> (7 - (pos & 7))) & 1
            } else {
                0
            };
            v = (v << 1) | bit as u32;
            pos += 1;
        }
        v
    };
    let mut aot = rd(5);
    if aot == 31 {
        aot = 32 + rd(6);
    }
    let sr_index = rd(4);
    let sample_rate = if sr_index == 15 {
        rd(24)
    } else {
        AAC_SAMPLE_RATES[sr_index as usize]
    };
    let chan_config = rd(4) as usize;
    let frame_length_flag = rd(1);
    AacConfig {
        audio_object_type: aot,
        sample_rate,
        channels: if chan_config > 0 { chan_config } else { sd_channels },
        frame_length: if frame_length_flag == 1 { 960 } else { 1024 },
    }
}

pub struct Mp4 {
    pub codec: Codec,
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<(usize, usize)>,
}

fn be(d: &[u8], o: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        v = (v << 8) | *d.get(o + i).unwrap_or(&0) as u64;
    }
    v
}

fn boxes(data: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut p = start;
    while p + 8 <= end {
        let size = be(data, p, 4) as usize;
        let typ = [data[p + 4], data[p + 5], data[p + 6], data[p + 7]];
        let (body, next) = if size == 1 {
            if p + 16 > end {
                break;
            }
            (p + 16, p + be(data, p + 8, 8) as usize)
        } else if size == 0 {
            (p + 8, end)
        } else {
            (p + 8, p + size)
        };
        if next <= p || next > end {
            break;
        }
        out.push((typ, body, next));
        p = next;
    }
    out
}

fn find_stbl(data: &[u8], start: usize, end: usize, depth: u32) -> Option<(usize, usize)> {
    if depth > 32 {
        return None;
    }
    for (typ, body, next) in boxes(data, start, end) {
        match &typ {
            b"stbl" => return Some((body, next)),
            b"moov" | b"trak" | b"mdia" | b"minf" => {
                if let Some(r) = find_stbl(data, body, next, depth + 1) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn child(data: &[u8], start: usize, end: usize, want: &[u8; 4]) -> Option<(usize, usize)> {
    boxes(data, start, end)
        .into_iter()
        .find(|(t, _, _)| t == want)
        .map(|(_, b, n)| (b, n))
}

fn parse_stsd(data: &[u8], start: usize, end: usize) -> Result<(Codec, usize, u32)> {
    let entry = start + 8;
    if entry + 36 > end {
        return Err(Error::Decode("mp4: short stsd"));
    }
    let size = be(data, entry, 4) as usize;
    let format = &data[entry + 4..entry + 8];
    let channels = be(data, entry + 24, 2) as usize;
    let sample_rate = be(data, entry + 32, 2) as u32;
    let entry_end = (entry + size).min(end);

    if format == b"alac" {
        let (cb, ce) = child(data, entry + 36, entry_end, b"alac")
            .ok_or(Error::Decode("mp4: no alac cookie box"))?;
        let _ = ce;
        let c = cb + 4;
        if c + 24 > end {
            return Err(Error::Decode("mp4: short alac cookie"));
        }
        let cfg = AlacConfig {
            frame_length: be(data, c, 4) as u32,
            bit_depth: data[c + 5],
            pb: data[c + 6],
            mb: data[c + 7],
            kb: data[c + 8],
            num_channels: data[c + 9],
            sample_rate: be(data, c + 20, 4) as u32,
        };
        Ok((Codec::Alac(cfg), channels, sample_rate))
    } else if format == b"mp4a" {
        let (eb, ee) = child(data, entry + 36, entry_end, b"esds")
            .ok_or(Error::Decode("mp4: no esds"))?;
        let (asc_s, asc_e) = find_asc(data, eb + 4, ee, 0).ok_or(Error::Decode("mp4: no asc"))?;
        let cfg = parse_asc(data, asc_s, asc_e, channels);
        Ok((Codec::Aac(cfg), channels, sample_rate))
    } else {
        Err(Error::Decode("mp4: unsupported sample format"))
    }
}

const MAX_SAMPLES: usize = 1 << 28;

fn parse_stsz(data: &[u8], start: usize, end: usize) -> Vec<usize> {
    let sample_size = be(data, start + 4, 4) as usize;
    let count = be(data, start + 8, 4) as usize;
    if sample_size != 0 {
        return vec![sample_size; count.min(MAX_SAMPLES).min(data.len())];
    }
    let count = count.min(end.saturating_sub(start + 12) / 4);
    (0..count).map(|i| be(data, start + 12 + 4 * i, 4) as usize).collect()
}

fn parse_offsets(data: &[u8], start: usize, end: usize, wide: bool) -> Vec<usize> {
    let count = be(data, start + 4, 4) as usize;
    let w = if wide { 8 } else { 4 };
    let count = count.min(end.saturating_sub(start + 8) / w);
    (0..count).map(|i| be(data, start + 8 + w * i, w) as usize).collect()
}

fn parse_stsc(data: &[u8], start: usize, end: usize) -> Vec<(usize, usize)> {
    let count = be(data, start + 4, 4) as usize;
    let count = count.min(end.saturating_sub(start + 8) / 12);
    (0..count)
        .map(|i| {
            let o = start + 8 + 12 * i;
            (be(data, o, 4) as usize, be(data, o + 4, 4) as usize)
        })
        .collect()
}

pub fn demux(data: &[u8]) -> Result<Mp4> {
    let (sb, se) = find_stbl(data, 0, data.len(), 0).ok_or(Error::Decode("mp4: no stbl"))?;
    let (stsd_b, stsd_e) = child(data, sb, se, b"stsd").ok_or(Error::Decode("mp4: no stsd"))?;
    let (codec, _channels, sd_rate) = parse_stsd(data, stsd_b, stsd_e)?;

    let sizes = child(data, sb, se, b"stsz")
        .map(|(b, n)| parse_stsz(data, b, n))
        .ok_or(Error::Decode("mp4: no stsz"))?;
    let chunk_offsets = child(data, sb, se, b"stco")
        .map(|(b, n)| parse_offsets(data, b, n, false))
        .or_else(|| child(data, sb, se, b"co64").map(|(b, n)| parse_offsets(data, b, n, true)))
        .ok_or(Error::Decode("mp4: no stco/co64"))?;
    let stsc = child(data, sb, se, b"stsc")
        .map(|(b, n)| parse_stsc(data, b, n))
        .ok_or(Error::Decode("mp4: no stsc"))?;

    let num_chunks = chunk_offsets.len();
    let mut spc = vec![0usize; num_chunks];
    for e in 0..stsc.len() {
        let first = stsc[e].0.saturating_sub(1);
        let last = if e + 1 < stsc.len() {
            stsc[e + 1].0.saturating_sub(1)
        } else {
            num_chunks
        };
        for c in first..last.min(num_chunks) {
            spc[c] = stsc[e].1;
        }
    }

    let mut samples = Vec::with_capacity(sizes.len());
    let mut si = 0usize;
    for c in 0..num_chunks {
        let mut off = chunk_offsets[c];
        for _ in 0..spc[c] {
            if si >= sizes.len() {
                break;
            }
            samples.push((off, sizes[si]));
            off = match off.checked_add(sizes[si]) {
                Some(v) => v,
                None => break,
            };
            si += 1;
        }
    }

    let (sample_rate, channels) = match &codec {
        Codec::Alac(cfg) => (
            if cfg.sample_rate != 0 { cfg.sample_rate } else { sd_rate },
            cfg.num_channels.max(1) as usize,
        ),
        Codec::Aac(cfg) => (
            if cfg.sample_rate != 0 { cfg.sample_rate } else { sd_rate },
            cfg.channels.max(1),
        ),
    };

    Ok(Mp4 {
        codec,
        sample_rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stsz_var_count_bomb_is_capped() {
        let mut d = vec![0u8; 16];
        d[8] = 0xff;
        d[9] = 0xff;
        d[10] = 0xff;
        d[11] = 0xff;
        let v = parse_stsz(&d, 0, d.len());
        assert!(v.len() <= d.len());
    }

    #[test]
    fn stsz_const_count_bomb_is_capped() {
        let mut d = vec![0u8; 16];
        d[7] = 4;
        d[8] = 0xff;
        d[9] = 0xff;
        d[10] = 0xff;
        d[11] = 0xff;
        let v = parse_stsz(&d, 0, d.len());
        assert!(v.len() <= d.len());
    }

    #[test]
    fn stco_count_bomb_is_capped() {
        let mut d = vec![0u8; 16];
        d[4] = 0xff;
        d[5] = 0xff;
        d[6] = 0xff;
        d[7] = 0xff;
        let v = parse_offsets(&d, 0, d.len(), false);
        assert!(v.len() <= d.len());
    }

    #[test]
    fn stsc_count_bomb_is_capped() {
        let mut d = vec![0u8; 16];
        d[4] = 0xff;
        d[5] = 0xff;
        d[6] = 0xff;
        d[7] = 0xff;
        let v = parse_stsc(&d, 0, d.len());
        assert!(v.len() <= d.len());
    }
}
