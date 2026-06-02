mod celt;
mod mdct;
mod ogg;
mod range;
mod toc;

use crate::decoder::Decoder;
use crate::error::Result;
use crate::pcm::gain_from_q8;
use celt::{decode_frame, DecoderState, Mode};
use ogg::OpusStream;
use toc::{split_frames, FrameMode, Toc};

const PREROLL: usize = 3;

#[derive(Clone, Copy)]
struct FrameRef {
    pkt: usize,
    off: usize,
    len: usize,
    toc: Toc,
    sample_pos: u64,
}

pub struct OpusDecoder {
    stream: OpusStream,
    mode: Mode,
    state: DecoderState,
    frames: Vec<FrameRef>,
    idx: usize,
    channels: usize,
    pre_skip: u64,
    pre_skip_total: u64,
    gain: f32,
}

fn fit_channels(pcm: Vec<f32>, coded: usize, out: usize) -> Vec<f32> {
    if coded == out {
        return pcm;
    }
    if coded == 1 && out == 2 {
        let mut v = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            v.push(s);
            v.push(s);
        }
        return v;
    }
    pcm
}

impl OpusDecoder {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::from_bytes(&crate::decoder::read_file_capped(path)?)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let stream = OpusStream::parse(data)?;
        let gain = gain_from_q8(stream.head.output_gain);
        let pre_skip = stream.head.pre_skip as u64;
        let channels = (stream.head.channels.max(1) as usize).min(2);

        let mut frames = Vec::new();
        let mut pos = 0u64;
        for (pi, pkt) in stream.packets.iter().enumerate() {
            if let Ok((toc, ranges)) = split_frames(pkt) {
                for (off, len) in ranges {
                    frames.push(FrameRef { pkt: pi, off, len, toc, sample_pos: pos });
                    pos += toc.samples as u64;
                }
            }
        }

        Ok(OpusDecoder {
            stream,
            mode: Mode::new(),
            state: DecoderState::new(channels),
            frames,
            idx: 0,
            channels,
            pre_skip,
            pre_skip_total: pre_skip,
            gain,
        })
    }

    fn decode_at(&mut self, i: usize) -> Vec<f32> {
        let fr = self.frames[i];
        let out_ch = self.channels;
        match fr.toc.mode {
            FrameMode::Celt => {
                let coded = if fr.toc.stereo && out_ch == 2 { 2 } else { 1 };
                let bytes = &self.stream.packets[fr.pkt][fr.off..fr.off + fr.len];
                let pcm = decode_frame(
                    &mut self.state,
                    &self.mode,
                    bytes,
                    fr.toc.lm as i32,
                    fr.toc.end as usize,
                    coded == 2,
                );
                fit_channels(pcm, coded, out_ch)
            }
            _ => vec![0.0f32; fr.toc.samples as usize * out_ch],
        }
    }
}

impl Decoder for OpusDecoder {
    fn next(&mut self) -> Option<Vec<f32>> {
        if self.idx >= self.frames.len() {
            return None;
        }
        let i = self.idx;
        self.idx += 1;
        let samples = self.frames[i].toc.samples as usize;
        let mut frame = self.decode_at(i);
        if self.gain != 1.0 {
            for s in frame.iter_mut() {
                *s *= self.gain;
            }
        }
        if self.pre_skip > 0 {
            let d = (self.pre_skip as usize).min(samples);
            self.pre_skip -= d as u64;
            frame.drain(0..d * self.channels);
        }
        Some(frame)
    }

    fn sample_rate(&self) -> u32 {
        48000
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn duration_frames(&self) -> u64 {
        self.stream.total_samples
    }

    fn pos_frames(&self) -> u64 {
        let raw = self
            .frames
            .get(self.idx)
            .map(|f| f.sample_pos)
            .unwrap_or(self.stream.total_samples + self.pre_skip_total);
        raw.saturating_sub(self.pre_skip_total)
    }

    fn seek(&mut self, frame: u64) {
        let raw = frame + self.pre_skip_total;
        let target = self
            .frames
            .partition_point(|f| f.sample_pos <= raw)
            .saturating_sub(1);
        let start = target.saturating_sub(PREROLL);
        self.state.reset();
        self.pre_skip = 0;
        self.idx = start;
        while self.idx < target {
            let i = self.idx;
            self.idx += 1;
            let _ = self.decode_at(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::Decoder;

    #[test]
    fn seek_preroll_lands_at_target_with_finite_audio() {
        let data = match std::fs::read("tests/fixtures/tiny.opus") {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut dec = OpusDecoder::from_bytes(&data).unwrap();
        if dec.stream.packets.len() < 51 {
            return;
        }
        dec.seek(48000);
        let pos = dec.pos_frames();
        assert!((45000..=48000).contains(&pos), "pos {pos} off target");
        let frame = dec.next().expect("frame after seek");
        assert!(!frame.is_empty());
        assert!(frame.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn decodes_celt_matrix_within_rms() {
        let dir = std::path::Path::new("tests/fixtures/opus-matrix");
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut checked = 0;
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("opus") {
                continue;
            }
            let refp = p.with_extension("f32le");
            if !refp.exists() {
                continue;
            }
            let want: Vec<f32> = std::fs::read(&refp)
                .unwrap()
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let mut dec = OpusDecoder::from_bytes(&std::fs::read(&p).unwrap()).unwrap();
            let mut got: Vec<f32> = Vec::new();
            while let Some(f) = dec.next() {
                got.extend_from_slice(&f);
            }
            let n = got.len().min(want.len());
            assert!(n > 48000, "{:?} decoded too little: {n}", p);
            let mut num = 0.0f64;
            let mut den = 0.0f64;
            for i in 0..n {
                let d = (got[i] - want[i]) as f64;
                num += d * d;
                den += (want[i] as f64).powi(2);
            }
            let rms = (num / den.max(1e-9)).sqrt();
            assert!(rms < 0.05, "{:?} relative RMS {rms} too high", p);
            checked += 1;
        }
        assert!(checked > 0, "no opus-matrix fixtures");
    }

    #[test]
    fn multiframe_packet_decodes_like_separate_frames() {
        let data = match std::fs::read("tests/fixtures/tiny.opus") {
            Ok(d) => d,
            Err(_) => return,
        };
        let stream = OpusStream::parse(&data).unwrap();
        if stream.packets.len() < 2 {
            return;
        }
        let p0 = &stream.packets[0];
        let p1 = &stream.packets[1];
        let toc = Toc::parse(p0[0]);
        let f0 = &p0[1..];
        let f1 = &p1[1..];

        let mut len = Vec::new();
        if f0.len() < 252 {
            len.push(f0.len() as u8);
        } else {
            len.push(252 + (f0.len() % 4) as u8);
            len.push(((f0.len() - 252) / 4) as u8);
        }
        let mut pkt = vec![(p0[0] & !3) | 3, 0x80 | 2];
        pkt.extend_from_slice(&len);
        pkt.extend_from_slice(f0);
        pkt.extend_from_slice(f1);

        let (t2, ranges) = split_frames(&pkt).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(&pkt[ranges[0].0..ranges[0].0 + ranges[0].1], f0);
        assert_eq!(&pkt[ranges[1].0..ranges[1].0 + ranges[1].1], f1);

        let mut a = DecoderState::new(2);
        let mut b = DecoderState::new(2);
        let lm = toc.lm as i32;
        let end = toc.end as usize;
        let packed: Vec<f32> = ranges
            .iter()
            .flat_map(|&(o, l)| decode_frame(&mut a, &Mode::new(), &pkt[o..o + l], lm, end, t2.stereo))
            .collect();
        let mode = Mode::new();
        let mut sep = decode_frame(&mut b, &mode, f0, lm, end, toc.stereo);
        sep.extend(decode_frame(&mut b, &mode, f1, lm, end, toc.stereo));
        assert_eq!(packed, sep);
    }

    fn drain(dec: &mut OpusDecoder, n: usize) {
        for _ in 0..n {
            match dec.next() {
                Some(f) => assert!(f.iter().all(|v| v.is_finite())),
                None => break,
            }
        }
    }

    fn ogg_page(htype: u8, granule: i64, seq: u32, packets: &[&[u8]]) -> Vec<u8> {
        let mut segs = Vec::new();
        let mut body = Vec::new();
        for p in packets {
            let mut l = p.len();
            while l >= 255 {
                segs.push(255u8);
                l -= 255;
            }
            segs.push(l as u8);
            body.extend_from_slice(p);
        }
        let mut v = b"OggS".to_vec();
        v.push(0);
        v.push(htype);
        v.extend_from_slice(&granule.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.push(segs.len() as u8);
        v.extend_from_slice(&segs);
        v.extend_from_slice(&body);
        v
    }

    fn opus_head(channels: u8) -> Vec<u8> {
        let mut h = b"OpusHead".to_vec();
        h.push(1);
        h.push(channels);
        h.extend_from_slice(&312u16.to_le_bytes());
        h.extend_from_slice(&48000u32.to_le_bytes());
        h.extend_from_slice(&0i16.to_le_bytes());
        h.push(0);
        h
    }

    #[test]
    fn fuzz_synthetic_stream_frames() {
        let tags = b"OpusTags\0\0\0\0";
        for channels in [1u8, 2] {
            let head = opus_head(channels);
            let head = &head;
            crate::fuzz::for_seeds(4000, |seed| {
                let alen = 1 + (seed as usize % 250);
                let audio = crate::fuzz::bytes(seed, alen);
                let pkts: [&[u8]; 3] = [head, tags, &audio];
                let stream = ogg_page(0x02, 960, 0, &pkts);
                if let Ok(mut dec) = OpusDecoder::from_bytes(&stream) {
                    drain(&mut dec, 4);
                }
            });
        }
    }

    fn opus_fixtures() -> Vec<Vec<u8>> {
        let mut v = crate::fuzz::read_dir_ext("tests/fixtures", ".opus");
        v.extend(crate::fuzz::read_dir_ext("tests/fixtures/voyager", ".opus"));
        v.truncate(2);
        v
    }

    #[test]
    fn fuzz_corrupt_and_truncate_fixtures() {
        for data in opus_fixtures() {
            crate::fuzz::corrupt_spread(&data, |c| {
                if let Ok(mut dec) = OpusDecoder::from_bytes(&c) {
                    drain(&mut dec, 6);
                }
            });
            crate::fuzz::truncate_points(&data, 64, |t| {
                if let Ok(mut dec) = OpusDecoder::from_bytes(t) {
                    drain(&mut dec, 8);
                }
            });
        }
    }

    #[test]
    fn fuzz_seek_never_panics() {
        for data in opus_fixtures() {
            if let Ok(mut dec) = OpusDecoder::from_bytes(&data) {
                for &t in &[0u64, 1, 1000, u64::MAX, u64::MAX / 2, 48000, 1 << 40, 1 << 50] {
                    dec.seek(t);
                    drain(&mut dec, 8);
                }
                for seed in 0..2000u64 {
                    let t = u64::from_le_bytes(crate::fuzz::bytes(seed, 8).try_into().unwrap());
                    dec.seek(t);
                    drain(&mut dec, 4);
                }
            }
        }
    }
}
