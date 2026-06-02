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

const FRAME: usize = 960;

pub struct OpusDecoder {
    stream: OpusStream,
    mode: Mode,
    state: DecoderState,
    idx: usize,
    pre_skip: u64,
    gain: f32,
    emitted: u64,
}

impl OpusDecoder {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path)?;
        let stream = OpusStream::parse(&data)?;
        let gain = gain_from_q8(stream.head.output_gain);
        let pre_skip = stream.head.pre_skip as u64;
        let channels = stream.head.channels.max(1) as usize;
        Ok(OpusDecoder {
            stream,
            mode: Mode::new(),
            state: DecoderState::new(channels),
            idx: 0,
            pre_skip,
            gain,
            emitted: 0,
        })
    }
}

impl Decoder for OpusDecoder {
    fn next(&mut self) -> Option<Vec<f32>> {
        if self.idx >= self.stream.packets.len() {
            return None;
        }
        let pkt = self.stream.packets[self.idx].clone();
        self.idx += 1;
        let cfg = match toc::Config::parse(&pkt) {
            Ok(c) => c,
            Err(_) => return Some(Vec::new()),
        };
        let frame = decode_frame(&mut self.state, &self.mode, cfg.frame, cfg.stereo);
        self.emitted += FRAME as u64;
        let mut drop = 0usize;
        if self.pre_skip > 0 {
            let d = (self.pre_skip as usize).min(FRAME);
            self.pre_skip -= d as u64;
            drop = d * 2;
        }
        Some(frame[drop..].iter().map(|&s| s * self.gain).collect())
    }

    fn sample_rate(&self) -> u32 {
        48000
    }

    fn channels(&self) -> usize {
        self.stream.head.channels.max(1) as usize
    }

    fn duration_frames(&self) -> u64 {
        self.stream.total_samples
    }

    fn pos_frames(&self) -> u64 {
        self.emitted.saturating_sub(self.stream.head.pre_skip as u64)
    }

    fn seek(&mut self, frame: u64) {
        let pkt = (frame / FRAME as u64) as usize;
        self.idx = pkt.min(self.stream.packets.len());
        self.state.reset();
        self.pre_skip = 0;
        self.emitted = self.idx as u64 * FRAME as u64;
    }
}
