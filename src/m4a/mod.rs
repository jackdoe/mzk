mod aac;
mod aac_tables;
mod alac;
mod mp4;

use crate::decoder::Decoder;
use crate::error::Result;
use aac::Aac;
use alac::Alac;
use mp4::{Codec, Mp4};

enum Audio {
    Alac(Alac),
    Aac(Aac),
}

pub struct M4aDecoder {
    data: Vec<u8>,
    mp4: Mp4,
    audio: Audio,
    frame_length: u64,
    idx: usize,
    emitted: u64,
}

impl M4aDecoder {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::from_bytes(crate::decoder::read_file_capped(path)?)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let mp4 = mp4::demux(&data)?;
        let (audio, frame_length) = match &mp4.codec {
            Codec::Alac(cfg) => (Audio::Alac(Alac::new(cfg)?), cfg.frame_length as u64),
            Codec::Aac(cfg) => (Audio::Aac(Aac::new(cfg)?), cfg.frame_length as u64),
        };
        Ok(M4aDecoder {
            data,
            mp4,
            audio,
            frame_length,
            idx: 0,
            emitted: 0,
        })
    }
}

impl Decoder for M4aDecoder {
    fn next(&mut self) -> Option<Vec<f32>> {
        if self.idx >= self.mp4.samples.len() {
            return None;
        }
        let (off, len) = self.mp4.samples[self.idx];
        self.idx += 1;
        if off.checked_add(len).map_or(true, |e| e > self.data.len()) {
            return None;
        }
        let pkt = &self.data[off..off + len];
        let pcm = match &mut self.audio {
            Audio::Alac(a) => a.decode_packet(pkt),
            Audio::Aac(a) => a.decode_packet(pkt),
        };
        self.emitted += (pcm.len() / self.mp4.channels.max(1)) as u64;
        Some(pcm)
    }

    fn sample_rate(&self) -> u32 {
        self.mp4.sample_rate
    }

    fn channels(&self) -> usize {
        self.mp4.channels
    }

    fn duration_frames(&self) -> u64 {
        self.mp4.samples.len() as u64 * self.frame_length
    }

    fn pos_frames(&self) -> u64 {
        self.emitted
    }

    fn seek(&mut self, frame: u64) {
        let fl = self.frame_length.max(1);
        self.idx = ((frame / fl) as usize).min(self.mp4.samples.len());
        self.emitted = self.idx as u64 * fl;
        match &mut self.audio {
            Audio::Alac(a) => a.reset(),
            Audio::Aac(a) => a.reset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voyager(ext: &str) -> Vec<std::path::PathBuf> {
        let dir = std::path::Path::new("tests/fixtures/voyager");
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.to_string_lossy().ends_with(ext) {
                    v.push(p);
                }
            }
        }
        v
    }

    #[test]
    fn alac_matches_reference_bit_exact() {
        let mut checked = 0;
        for path in voyager(".alac.m4a") {
            let stem = path.to_string_lossy().replace(".alac.m4a", ".f32le");
            let want_bytes = match std::fs::read(&stem) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let want: Vec<f32> = want_bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let mut dec = M4aDecoder::open(&path).unwrap();
            let mut got = Vec::new();
            while got.len() < want.len() {
                match dec.next() {
                    Some(f) => got.extend_from_slice(&f),
                    None => break,
                }
            }
            got.truncate(want.len());
            assert_eq!(got.len(), want.len(), "{:?} length", path);
            let maxd = got
                .iter()
                .zip(&want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(maxd < 1e-6, "{:?} max diff {maxd}", path);
            checked += 1;
        }
        assert!(checked > 0, "no alac fixtures");
    }

    #[test]
    fn demux_alac_structure() {
        for path in voyager(".alac.m4a") {
            let data = std::fs::read(&path).unwrap();
            let m = mp4::demux(&data).unwrap();
            assert_eq!(m.sample_rate, 44100, "{:?}", path);
            assert_eq!(m.channels, 2, "{:?}", path);
            assert!(matches!(m.codec, Codec::Alac(_)));
            assert!(m.samples.len() > 100, "{:?}", path);
        }
    }

    #[test]
    fn box_size_overflow_does_not_panic() {
        let data = match std::fs::read("tests/fixtures/mp4_box_overflow.bin") {
            Ok(d) => d,
            Err(_) => return,
        };
        let _ = mp4::demux(&data);
        if let Ok(mut dec) = M4aDecoder::from_bytes(data) {
            for _ in 0..16 {
                if dec.next().is_none() {
                    break;
                }
            }
        }
    }

    #[test]
    fn fuzz_demux_and_decode_never_panic() {
        let prefixes: [&[u8]; 3] = [
            &[],
            b"\x00\x00\x00\x14ftypM4A \x00\x00\x00\x00M4A mp42",
            b"\x00\x00\x00\x08moov",
        ];
        for prefix in prefixes {
            crate::fuzz::each_case(4000, 512, |data| {
                let mut buf = Vec::with_capacity(prefix.len() + data.len());
                buf.extend_from_slice(prefix);
                buf.extend_from_slice(data);
                let _ = mp4::demux(&buf);
                if let Ok(mut dec) = M4aDecoder::from_bytes(buf) {
                    for _ in 0..32 {
                        match dec.next() {
                            Some(f) => assert!(f.iter().all(|v| v.is_finite())),
                            None => break,
                        }
                    }
                }
            });
        }
    }

    #[test]
    fn demux_aac_detected() {
        for path in voyager(".aac.m4a") {
            let data = std::fs::read(&path).unwrap();
            let m = mp4::demux(&data).unwrap();
            assert!(matches!(m.codec, Codec::Aac(_)), "{:?}", path);
            assert_eq!(m.sample_rate, 44100, "{:?}", path);
        }
    }

    #[test]
    fn aac_matches_reference_within_rms() {
        let mut checked = 0;
        for path in voyager(".aac.m4a") {
            let refp = path.to_string_lossy().replace(".aac.m4a", ".aac.f32le");
            let want_bytes = match std::fs::read(&refp) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let want: Vec<f32> = want_bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let mut dec = M4aDecoder::open(&path).unwrap();
            let cap = 44100 * 2;
            let mut got = Vec::new();
            while got.len() < cap {
                match dec.next() {
                    Some(f) => got.extend_from_slice(&f),
                    None => break,
                }
            }
            assert!(got.len() > 44100, "{:?} decoded too little", path);

            let mut best = f64::INFINITY;
            let mut best_lag = 0i64;
            for lag in 900i64..=1150 {
                let shift = lag * 2;
                let mut num = 0.0f64;
                let mut den = 0.0f64;
                let mut cnt = 0usize;
                let n = got.len().min(want.len());
                for j in 0..n {
                    let gi = j as i64 + shift;
                    if gi < 0 || gi as usize >= got.len() {
                        continue;
                    }
                    let d = (got[gi as usize] - want[j]) as f64;
                    num += d * d;
                    den += (want[j] as f64).powi(2);
                    cnt += 1;
                }
                if cnt > 40000 {
                    let rms = (num / den.max(1e-9)).sqrt();
                    if rms < best {
                        best = rms;
                        best_lag = lag;
                    }
                }
            }
            let _ = best_lag;
            assert!(best < 0.05, "{:?} min relative RMS {best}", path);
            checked += 1;
        }
        assert!(checked > 0, "no aac fixtures");
    }

    fn drain(dec: &mut M4aDecoder, n: usize) {
        for _ in 0..n {
            match dec.next() {
                Some(f) => assert!(f.iter().all(|v| v.is_finite())),
                None => break,
            }
        }
    }

    #[test]
    fn fuzz_corrupt_and_truncate_fixtures() {
        let mut files = crate::fuzz::read_dir_ext("tests/fixtures/voyager", ".alac.m4a");
        files.truncate(1);
        files.extend(crate::fuzz::read_dir_ext("tests/fixtures/voyager", ".aac.m4a").into_iter().take(1));
        for data in files {
            crate::fuzz::corrupt_spread(&data, |c| {
                let _ = mp4::demux(&c);
                if let Ok(mut dec) = M4aDecoder::from_bytes(c) {
                    drain(&mut dec, 6);
                }
            });
            crate::fuzz::truncate_points(&data, 64, |t| {
                let _ = mp4::demux(t);
                if let Ok(mut dec) = M4aDecoder::from_bytes(t.to_vec()) {
                    drain(&mut dec, 12);
                }
            });
        }
    }

    #[test]
    fn fuzz_seek_never_panics() {
        let mut files = crate::fuzz::read_dir_ext("tests/fixtures/voyager", ".alac.m4a");
        files.truncate(1);
        files.extend(crate::fuzz::read_dir_ext("tests/fixtures/voyager", ".aac.m4a").into_iter().take(1));
        for data in files {
            if let Ok(mut dec) = M4aDecoder::from_bytes(data) {
                for &t in &[0u64, 1, 1000, u64::MAX, u64::MAX / 2, 44100, 1 << 40, 1 << 50] {
                    dec.seek(t);
                    drain(&mut dec, 8);
                }
                for seed in 0..3000u64 {
                    let t = u64::from_le_bytes(crate::fuzz::bytes(seed, 8).try_into().unwrap());
                    dec.seek(t);
                    drain(&mut dec, 4);
                }
            }
        }
    }
}
