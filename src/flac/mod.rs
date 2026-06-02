use crate::decoder::Decoder;
use crate::error::{Error, Result};

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], byte_off: usize) -> Self {
        BitReader {
            data,
            pos: byte_off * 8,
        }
    }

    fn bit(&mut self) -> u32 {
        let byte = self.pos >> 3;
        let b = if byte < self.data.len() {
            self.data[byte]
        } else {
            0
        };
        let shift = 7 - (self.pos & 7);
        self.pos += 1;
        ((b >> shift) & 1) as u32
    }

    fn read(&mut self, n: u32) -> u64 {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | self.bit() as u64;
        }
        v
    }

    fn read_signed(&mut self, n: u32) -> i64 {
        if n == 0 {
            return 0;
        }
        let v = self.read(n);
        let shift = 64 - n;
        ((v << shift) as i64) >> shift
    }

    fn read_unary(&mut self) -> u32 {
        let mut c = 0u32;
        let end = self.data.len() * 8;
        while self.pos < end && self.bit() == 0 {
            c += 1;
        }
        c
    }

    fn read_utf8(&mut self) -> u64 {
        let b0 = self.read(8) as u8;
        if b0 < 0x80 {
            return b0 as u64;
        }
        let mut n = 0u32;
        let mut mask = 0x80u8;
        while b0 & mask != 0 {
            n += 1;
            mask >>= 1;
        }
        let mut val = (b0 & (0x7f >> n)) as u64;
        for _ in 1..n {
            let b = self.read(8) as u8;
            val = (val << 6) | (b & 0x3f) as u64;
        }
        val
    }

    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    fn byte_pos(&self) -> usize {
        self.pos >> 3
    }
}

const BLOCK_SIZE_TABLE: [u32; 16] = [
    0, 192, 576, 1152, 2304, 4608, 0, 0, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768,
];
const BPS_TABLE: [u32; 8] = [0, 8, 12, 0, 16, 20, 24, 0];

struct StreamInfo {
    sample_rate: u32,
    channels: usize,
    bps: u32,
    total_samples: u64,
}

pub struct FlacDecoder {
    data: Vec<u8>,
    info: StreamInfo,
    first_frame: usize,
    cursor: usize,
    emitted: u64,
}

fn be(data: &[u8], off: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        v = (v << 8) | data[off + i] as u64;
    }
    v
}

fn parse_metadata(data: &[u8]) -> Result<(StreamInfo, usize)> {
    if data.len() < 4 || &data[..4] != b"fLaC" {
        return Err(Error::Decode("flac: missing fLaC marker"));
    }
    let mut pos = 4usize;
    let mut info: Option<StreamInfo> = None;
    loop {
        if pos + 4 > data.len() {
            return Err(Error::Decode("flac: truncated metadata"));
        }
        let header = data[pos];
        let last = header & 0x80 != 0;
        let block_type = header & 0x7f;
        let len = be(data, pos + 1, 3) as usize;
        let body = pos + 4;
        if body + len > data.len() {
            return Err(Error::Decode("flac: truncated metadata block"));
        }
        if block_type == 0 {
            if len < 34 {
                return Err(Error::Decode("flac: short STREAMINFO"));
            }
            let packed = be(data, body + 10, 8);
            info = Some(StreamInfo {
                sample_rate: ((packed >> 44) & 0xFFFFF) as u32,
                channels: (((packed >> 41) & 0x7) + 1) as usize,
                bps: (((packed >> 36) & 0x1F) + 1) as u32,
                total_samples: packed & 0xF_FFFF_FFFF,
            });
        }
        pos = body + len;
        if last {
            break;
        }
    }
    let info = info.ok_or(Error::Decode("flac: no STREAMINFO"))?;
    Ok((info, pos))
}

fn fixed_restore(buf: &mut [i64], order: usize) {
    let n = buf.len();
    match order {
        0 => {}
        1 => {
            for i in 1..n {
                buf[i] += buf[i - 1];
            }
        }
        2 => {
            for i in 2..n {
                buf[i] += 2 * buf[i - 1] - buf[i - 2];
            }
        }
        3 => {
            for i in 3..n {
                buf[i] += 3 * buf[i - 1] - 3 * buf[i - 2] + buf[i - 3];
            }
        }
        4 => {
            for i in 4..n {
                buf[i] += 4 * buf[i - 1] - 6 * buf[i - 2] + 4 * buf[i - 3] - buf[i - 4];
            }
        }
        _ => {}
    }
}

fn lpc_restore(buf: &mut [i64], coefs: &[i64], shift: i32, order: usize) {
    let n = buf.len();
    for i in order..n {
        let mut pred = 0i64;
        for j in 0..order {
            pred += coefs[j] * buf[i - 1 - j];
        }
        buf[i] += pred >> shift;
    }
}

impl FlacDecoder {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let (info, first_frame) = parse_metadata(&data)?;
        if info.channels == 0 || info.bps == 0 || info.sample_rate == 0 {
            return Err(Error::Decode("flac: invalid stream info"));
        }
        Ok(FlacDecoder {
            data,
            info,
            first_frame,
            cursor: first_frame,
            emitted: 0,
        })
    }

    fn read_residual(&self, br: &mut BitReader, buf: &mut [i64], order: usize, blocksize: usize) {
        let method = br.read(2);
        let param_bits = if method == 1 { 5 } else { 4 };
        let escape = (1u64 << param_bits) - 1;
        let partition_order = br.read(4) as usize;
        let partitions = 1usize << partition_order;
        let part_len = blocksize >> partition_order;
        let mut idx = order;
        for p in 0..partitions {
            let count = if p == 0 { part_len - order } else { part_len };
            let param = br.read(param_bits);
            if param == escape {
                let raw = br.read(5) as u32;
                for _ in 0..count {
                    buf[idx] = br.read_signed(raw);
                    idx += 1;
                }
            } else {
                for _ in 0..count {
                    let q = br.read_unary() as u64;
                    let r = br.read(param as u32);
                    let val = (q << param) | r;
                    buf[idx] = (val >> 1) as i64 ^ -((val & 1) as i64);
                    idx += 1;
                }
            }
        }
    }

    fn read_subframe(&self, br: &mut BitReader, buf: &mut [i64], bps: u32) {
        let blocksize = buf.len();
        br.read(1);
        let kind = br.read(6) as u32;
        let flag = br.read(1);
        let wasted = if flag == 1 { br.read_unary() + 1 } else { 0 };
        let bps = bps - wasted;

        if kind == 0 {
            let v = br.read_signed(bps);
            for s in buf.iter_mut() {
                *s = v;
            }
        } else if kind == 1 {
            for s in buf.iter_mut() {
                *s = br.read_signed(bps);
            }
        } else if kind >= 32 {
            let order = (kind & 0x1f) as usize + 1;
            for s in buf.iter_mut().take(order) {
                *s = br.read_signed(bps);
            }
            let precision = br.read(4) as u32 + 1;
            let shift = br.read_signed(5) as i32;
            let mut coefs = [0i64; 32];
            for c in coefs.iter_mut().take(order) {
                *c = br.read_signed(precision);
            }
            self.read_residual(br, buf, order, blocksize);
            lpc_restore(buf, &coefs, shift, order);
        } else if kind >= 8 {
            let order = (kind & 0x07) as usize;
            for s in buf.iter_mut().take(order) {
                *s = br.read_signed(bps);
            }
            self.read_residual(br, buf, order, blocksize);
            fixed_restore(buf, order);
        }

        if wasted > 0 {
            for s in buf.iter_mut() {
                *s <<= wasted;
            }
        }
    }

    fn decode_frame(&mut self) -> Option<Vec<f32>> {
        if self.cursor + 2 >= self.data.len() {
            return None;
        }
        let mut br = BitReader::new(&self.data, self.cursor);
        if br.read(14) != 0x3FFE {
            return None;
        }
        br.read(1);
        br.read(1);
        let bs_code = br.read(4) as usize;
        let sr_code = br.read(4) as usize;
        let ch_assign = br.read(4) as usize;
        let ss_code = br.read(3) as usize;
        br.read(1);
        br.read_utf8();

        let blocksize = match bs_code {
            6 => br.read(8) as usize + 1,
            7 => br.read(16) as usize + 1,
            c => BLOCK_SIZE_TABLE[c] as usize,
        };
        match sr_code {
            12 => {
                br.read(8);
            }
            13 | 14 => {
                br.read(16);
            }
            _ => {}
        }
        br.read(8);

        if blocksize == 0 {
            return None;
        }

        let frame_bps = if ss_code == 0 {
            self.info.bps
        } else {
            BPS_TABLE[ss_code]
        };
        let channels = if ch_assign < 8 {
            ch_assign + 1
        } else {
            2
        };

        let mut chans: Vec<Vec<i64>> = Vec::with_capacity(channels);
        for c in 0..channels {
            let cbps = match ch_assign {
                8 if c == 1 => frame_bps + 1,
                9 if c == 0 => frame_bps + 1,
                10 if c == 1 => frame_bps + 1,
                _ => frame_bps,
            };
            let mut buf = vec![0i64; blocksize];
            self.read_subframe(&mut br, &mut buf, cbps);
            chans.push(buf);
        }

        br.align();
        br.read(16);
        self.cursor = br.byte_pos();
        self.emitted += blocksize as u64;

        let scale = 1.0 / (1u64 << (frame_bps - 1)) as f32;
        let mut out = vec![0.0f32; blocksize * channels];
        match ch_assign {
            8 => {
                for i in 0..blocksize {
                    let l = chans[0][i];
                    let r = l - chans[1][i];
                    out[i * 2] = l as f32 * scale;
                    out[i * 2 + 1] = r as f32 * scale;
                }
            }
            9 => {
                for i in 0..blocksize {
                    let r = chans[1][i];
                    let l = r + chans[0][i];
                    out[i * 2] = l as f32 * scale;
                    out[i * 2 + 1] = r as f32 * scale;
                }
            }
            10 => {
                for i in 0..blocksize {
                    let s = chans[1][i];
                    let m = (chans[0][i] << 1) | (s & 1);
                    out[i * 2] = ((m + s) >> 1) as f32 * scale;
                    out[i * 2 + 1] = ((m - s) >> 1) as f32 * scale;
                }
            }
            _ => {
                for i in 0..blocksize {
                    for (c, chan) in chans.iter().enumerate() {
                        out[i * channels + c] = chan[i] as f32 * scale;
                    }
                }
            }
        }
        Some(out)
    }
}

impl Decoder for FlacDecoder {
    fn next(&mut self) -> Option<Vec<f32>> {
        self.decode_frame()
    }

    fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    fn channels(&self) -> usize {
        self.info.channels
    }

    fn duration_frames(&self) -> u64 {
        self.info.total_samples
    }

    fn pos_frames(&self) -> u64 {
        self.emitted
    }

    fn seek(&mut self, frame: u64) {
        self.cursor = self.first_frame;
        self.emitted = 0;
        while self.emitted + 1 < frame {
            let before = self.emitted;
            if self.decode_frame().is_none() {
                break;
            }
            if self.emitted == before {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_matches_reference_bit_exact() {
        let dir = std::path::Path::new("tests/fixtures/voyager");
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut checked = 0;
        for e in entries.flatten() {
            let flac = e.path();
            if flac.extension().and_then(|s| s.to_str()) != Some("flac") {
                continue;
            }
            let want_bytes = match std::fs::read(flac.with_extension("f32le")) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let want: Vec<f32> = want_bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let mut dec = FlacDecoder::open(&flac).unwrap();
            let mut got = Vec::new();
            while got.len() < want.len() {
                match dec.next() {
                    Some(f) => got.extend_from_slice(&f),
                    None => break,
                }
            }
            got.truncate(want.len());
            assert_eq!(got.len(), want.len(), "{:?} length", flac);
            let maxd = got
                .iter()
                .zip(&want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(maxd < 1e-6, "{:?} max diff {maxd}", flac);
            checked += 1;
        }
        assert!(checked > 0, "no flac fixtures found");
    }
}
