use crate::decoder::Decoder;
use crate::error::{Error, Result};

const CHUNK_FRAMES: usize = 4096;

#[derive(Clone, Copy)]
enum Sample {
    U8,
    I16,
    I24,
    I32,
    F32,
    F64,
}

pub struct WavDecoder {
    data: Vec<u8>,
    sample: Sample,
    rate: u32,
    channels: usize,
    block_align: usize,
    data_start: usize,
    data_end: usize,
    cursor: usize,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl WavDecoder {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return Err(Error::Decode("not a RIFF/WAVE file"));
        }

        let mut fmt: Option<(u16, u16, u32, u16)> = None;
        let mut data_range: Option<(usize, usize)> = None;
        let mut pos = 12usize;
        while pos + 8 <= data.len() {
            let id = &data[pos..pos + 4];
            let size = le32(&data, pos + 4) as usize;
            let body = pos + 8;
            if body + size > data.len() {
                break;
            }
            if id == b"fmt " && size >= 16 {
                let mut format = le16(&data, body);
                let channels = le16(&data, body + 2);
                let rate = le32(&data, body + 4);
                let bits = le16(&data, body + 14);
                if format == 0xFFFE && size >= 40 {
                    format = le16(&data, body + 24);
                }
                fmt = Some((format, channels, rate, bits));
            } else if id == b"data" {
                data_range = Some((body, body + size));
            }
            pos = body + size + (size & 1);
        }

        let (format, channels, rate, bits) = fmt.ok_or(Error::Decode("wav: no fmt chunk"))?;
        let (data_start, data_end) = data_range.ok_or(Error::Decode("wav: no data chunk"))?;
        if channels == 0 {
            return Err(Error::Decode("wav: zero channels"));
        }

        let sample = match (format, bits) {
            (1, 8) => Sample::U8,
            (1, 16) => Sample::I16,
            (1, 24) => Sample::I24,
            (1, 32) => Sample::I32,
            (3, 32) => Sample::F32,
            (3, 64) => Sample::F64,
            _ => return Err(Error::Decode("wav: unsupported sample format")),
        };

        let block_align = channels as usize * (bits as usize / 8);
        if block_align == 0 {
            return Err(Error::Decode("wav: zero block align"));
        }

        Ok(WavDecoder {
            data,
            sample,
            rate,
            channels: channels as usize,
            block_align,
            data_start,
            data_end,
            cursor: data_start,
        })
    }
}

fn convert(sample: Sample, b: &[u8], o: usize) -> f32 {
    match sample {
        Sample::U8 => (b[o] as f32 - 128.0) / 128.0,
        Sample::I16 => i16::from_le_bytes([b[o], b[o + 1]]) as f32 / 32768.0,
        Sample::I24 => {
            let v = (b[o] as i32) | ((b[o + 1] as i32) << 8) | ((b[o + 2] as i32) << 16);
            let v = (v << 8) >> 8;
            v as f32 / 8388608.0
        }
        Sample::I32 => le32(b, o) as i32 as f32 / 2147483648.0,
        Sample::F32 => f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]),
        Sample::F64 => f64::from_le_bytes([
            b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7],
        ]) as f32,
    }
}

impl Decoder for WavDecoder {
    fn next(&mut self) -> Option<Vec<f32>> {
        if self.cursor + self.block_align > self.data_end {
            return None;
        }
        let frames = ((self.data_end - self.cursor) / self.block_align).min(CHUNK_FRAMES);
        let width = self.block_align / self.channels;
        let mut out = Vec::with_capacity(frames * self.channels);
        for f in 0..frames {
            let frame = self.cursor + f * self.block_align;
            for c in 0..self.channels {
                out.push(convert(self.sample, &self.data, frame + c * width));
            }
        }
        self.cursor += frames * self.block_align;
        Some(out)
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn duration_frames(&self) -> u64 {
        ((self.data_end - self.data_start) / self.block_align) as u64
    }

    fn pos_frames(&self) -> u64 {
        ((self.cursor - self.data_start) / self.block_align) as u64
    }

    fn seek(&mut self, frame: u64) {
        let off = self.data_start + frame as usize * self.block_align;
        self.cursor = off.min(self.data_end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_matches_reference_bit_exact() {
        let dir = std::path::Path::new("tests/fixtures/voyager");
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut checked = 0;
        for e in entries.flatten() {
            let wav = e.path();
            if wav.extension().and_then(|s| s.to_str()) != Some("wav") {
                continue;
            }
            let want_bytes = match std::fs::read(wav.with_extension("f32le")) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let want: Vec<f32> = want_bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let mut dec = WavDecoder::open(&wav).unwrap();
            let mut got = Vec::new();
            while let Some(f) = dec.next() {
                got.extend_from_slice(&f);
            }
            assert_eq!(got.len(), want.len(), "{:?} length", wav);
            let maxd = got
                .iter()
                .zip(&want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(maxd < 1e-7, "{:?} max diff {maxd}", wav);
            checked += 1;
        }
        assert!(checked > 0, "no wav fixtures found");
    }
}

