mod bits;
mod header;
mod huffman;
mod imdct;
mod requant;
mod sideinfo;
mod stereo;
mod synthesis;
mod tables;

use crate::decoder::Decoder;
use crate::error::{Error, Result};
use bits::Bits;
use header::{
    find_frame, frame_bytes, frame_samples, get_my_sample_rate, is_crc, is_mono, padding,
    sample_rate_hz, skip_id3v2, test_mpeg1, HDR_SIZE,
};
use sideinfo::{read_side_info, GrInfo};

const MAX_BITRESERVOIR_BYTES: usize = 511;

pub struct Mp3Decoder {
    data: Vec<u8>,
    pos: usize,
    header: [u8; 4],
    free_format_bytes: usize,
    reserv: usize,
    reserv_buf: [u8; MAX_BITRESERVOIR_BYTES],
    overlap: [[f32; 576]; 2],
    vfifo: [[f32; 1024]; 2],
    voff: [usize; 2],
    ist_pos: [[u8; 39]; 2],
    rate: u32,
    channels: usize,
    spf: usize,
    duration: u64,
    emitted: u64,
    start_pos: usize,
    cbr_frame_bytes: usize,
}

impl Mp3Decoder {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::from_bytes(crate::decoder::read_file_capped(path)?)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let id3 = skip_id3v2(&data).min(data.len());
        let mut ff = 0usize;
        let (off, fbytes) = find_frame(&data[id3..], &mut ff);
        if fbytes == 0 {
            return Err(Error::Unsupported("no mp3 frame found"));
        }
        let fstart = id3 + off;
        let h = &data[fstart..fstart + 4];
        if header::get_layer(h) != 1 {
            return Err(Error::Unsupported("only MPEG Layer III"));
        }
        let rate = sample_rate_hz(h);
        let channels = if is_mono(h) { 1 } else { 2 };
        let spf = frame_samples(h) as u64;

        let mut start_pos = fstart;
        let mut duration;
        if let Some(nframes) = parse_xing(&data, fstart, h) {
            duration = nframes * spf;
            start_pos = fstart + fbytes;
        } else {
            let audio_bytes = data.len() - fstart;
            let nframes = (audio_bytes / fbytes) as u64;
            duration = nframes * spf;
        }
        if duration == 0 {
            duration = spf;
        }

        Ok(Mp3Decoder {
            data,
            pos: start_pos,
            header: [0; 4],
            free_format_bytes: 0,
            reserv: 0,
            reserv_buf: [0; MAX_BITRESERVOIR_BYTES],
            overlap: [[0.0; 576]; 2],
            vfifo: [[0.0; 1024]; 2],
            voff: [0; 2],
            ist_pos: [[0; 39]; 2],
            rate,
            channels,
            spf: spf as usize,
            duration,
            emitted: 0,
            start_pos,
            cbr_frame_bytes: fbytes,
        })
    }

    fn reset_state(&mut self) {
        self.header = [0; 4];
        self.free_format_bytes = 0;
        self.reserv = 0;
        self.overlap = [[0.0; 576]; 2];
        self.vfifo = [[0.0; 1024]; 2];
        self.voff = [0; 2];
    }

    fn decode_frame(&mut self) -> Option<Vec<f32>> {
        let mut frame_size = 0usize;
        let mut i = 0usize;
        let avail = self.data.len() - self.pos;
        {
            let mp3 = &self.data[self.pos..];
            if avail > 4 && self.header[0] == 0xff && header::compare(&self.header, mp3) {
                frame_size = frame_bytes(mp3, self.free_format_bytes) + padding(mp3);
                if frame_size != avail
                    && (frame_size + HDR_SIZE > avail || !header::compare(mp3, &mp3[frame_size..]))
                {
                    frame_size = 0;
                }
            }
        }
        if frame_size == 0 {
            self.reset_state();
            let mut ff = 0usize;
            let (off, fs) = find_frame(&self.data[self.pos..], &mut ff);
            self.free_format_bytes = ff;
            i = off;
            frame_size = fs;
            if frame_size == 0 || i + frame_size > avail {
                self.pos += i;
                return None;
            }
        }
        if frame_size < HDR_SIZE + 1 {
            self.pos += i + frame_size;
            return None;
        }

        let fstart = self.pos + i;
        let h = [
            self.data[fstart],
            self.data[fstart + 1],
            self.data[fstart + 2],
            self.data[fstart + 3],
        ];
        self.header = h;
        let channels = if is_mono(&h) { 1 } else { 2 };
        let ngr = if test_mpeg1(&h) { 2 } else { 1 };
        let spf = frame_samples(&h);

        let mut payload = self.data[fstart + 4..fstart + frame_size].to_vec();
        payload.extend_from_slice(&[0u8; 8]);
        let payload_len = frame_size - 4;

        self.pos = fstart + frame_size;

        let mut bs = Bits::new(&payload, payload_len);
        if is_crc(&h) {
            bs.get_bits(16);
        }
        let mut gr_info = [GrInfo::default(); 4];
        let main_data_begin = read_side_info(&mut bs, &mut gr_info, &h);
        let mut out = vec![0.0f32; spf * channels];
        if main_data_begin < 0 || bs.pos > bs.limit {
            self.header = [0; 4];
            return Some(out);
        }

        let sideinfo_bytes = bs.pos / 8;
        let main_bytes = payload_len - sideinfo_bytes;
        let mdb = main_data_begin as usize;
        let bytes_have = self.reserv.min(mdb);
        let src = self.reserv.saturating_sub(mdb);
        let total_main = bytes_have + main_bytes;
        let mut maindata = vec![0u8; total_main + 64];
        maindata[..bytes_have].copy_from_slice(&self.reserv_buf[src..src + bytes_have]);
        maindata[bytes_have..total_main]
            .copy_from_slice(&payload[sideinfo_bytes..sideinfo_bytes + main_bytes]);
        let success = self.reserv >= mdb;

        let mut bs2 = Bits::new(&maindata, total_main);

        if success {
            self.ist_pos = [[0; 39]; 2];
            let mut grbuf = [0.0f32; 1152];
            let mut is = [0i32; 576];
            for igr in 0..ngr {
                let g0 = igr * channels;
                for ch in 0..channels {
                    let mut scf = [0.0f32; 40];
                    let gi = gr_info[g0 + ch];
                    let part_2_start = bs2.pos;
                    requant::decode_scalefactors(&h, &mut self.ist_pos[ch], &mut bs2, &gi, &mut scf, ch);
                    huffman::read_huffman(&mut bs2, &gi, part_2_start, &mut is);
                    requant::requantize(&is, &scf, gi.sfbtab, &mut grbuf[ch * 576..ch * 576 + 576]);
                }

                {
                    let (_, ist1) = self.ist_pos.split_at_mut(1);
                    stereo::apply_stereo(&mut grbuf, &mut ist1[0], &gr_info[g0..g0 + channels], &h);
                }

                let base = igr * 576 * channels;
                for ch in 0..channels {
                    let gi = gr_info[g0 + ch];
                    let n_long_bands = (if gi.mixed_block_flag != 0 { 2 } else { 0 })
                        << (get_my_sample_rate(&h) == 2) as usize;
                    let cb = ch * 576;
                    if gi.n_short_sfb != 0 {
                        stereo::reorder(
                            &mut grbuf[cb..cb + 576],
                            n_long_bands * 18,
                            &gi.sfbtab[gi.n_long_sfb as usize..],
                        );
                    }
                    let aa_bands = if gi.n_short_sfb != 0 {
                        n_long_bands as i32 - 1
                    } else {
                        31
                    };
                    if aa_bands > 0 {
                        stereo::antialias(&mut grbuf[cb..cb + 576], 0, aa_bands as usize);
                    }
                    imdct::hybrid_synthesis(
                        &mut grbuf[cb..cb + 576],
                        &mut self.overlap[ch],
                        gi.block_type,
                        n_long_bands,
                    );
                    imdct::frequency_inversion(&mut grbuf[cb..cb + 576]);
                    synthesis::subband_synthesis(
                        &grbuf[cb..cb + 576],
                        &mut self.vfifo[ch],
                        &mut self.voff[ch],
                        &mut out[base..base + 576 * channels],
                        ch,
                        channels,
                    );
                }
            }
        }

        let pos = (bs2.pos + 7) / 8;
        let mut remains = total_main - pos.min(total_main);
        let mut srcpos = pos;
        if remains > MAX_BITRESERVOIR_BYTES {
            srcpos += remains - MAX_BITRESERVOIR_BYTES;
            remains = MAX_BITRESERVOIR_BYTES;
        }
        if remains > 0 {
            self.reserv_buf[..remains].copy_from_slice(&maindata[srcpos..srcpos + remains]);
        }
        self.reserv = remains;

        self.emitted += spf as u64;
        Some(out)
    }
}

fn parse_xing(data: &[u8], fstart: usize, h: &[u8]) -> Option<u64> {
    let si = if test_mpeg1(h) {
        if is_mono(h) {
            17
        } else {
            32
        }
    } else if is_mono(h) {
        9
    } else {
        17
    };
    let off = fstart + 4 + si;
    if off + 12 > data.len() {
        return None;
    }
    let tag = &data[off..off + 4];
    if tag != b"Xing" && tag != b"Info" {
        return None;
    }
    let flags = u32::from_be_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]);
    if flags & 1 == 0 {
        return None;
    }
    let nframes = u32::from_be_bytes([data[off + 8], data[off + 9], data[off + 10], data[off + 11]]);
    Some(nframes as u64)
}

impl Decoder for Mp3Decoder {
    fn next(&mut self) -> Option<Vec<f32>> {
        if self.pos + HDR_SIZE > self.data.len() {
            return None;
        }
        self.decode_frame()
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn duration_frames(&self) -> u64 {
        self.duration
    }
    fn pos_frames(&self) -> u64 {
        self.emitted
    }
    fn seek(&mut self, frame: u64) {
        let nframe = frame / self.spf as u64;
        self.pos = (self.start_pos + nframe as usize * self.cbr_frame_bytes).min(self.data.len());
        self.reset_state();
        self.emitted = nframe * self.spf as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use header::{bitrate_kbps, valid};

    #[test]
    fn parses_first_header() {
        let data = match std::fs::read("tests/fixtures/tiny.mp3") {
            Ok(d) => d,
            Err(_) => return,
        };
        let id3 = skip_id3v2(&data);
        let mut ff = 0usize;
        let (off, fb) = find_frame(&data[id3..], &mut ff);
        let h = &data[id3 + off..id3 + off + 4];
        assert!(valid(h));
        assert_eq!(sample_rate_hz(h), 48000);
        assert_eq!(if is_mono(h) { 1 } else { 2 }, 2);
        assert!((200..=1500).contains(&fb), "frame_bytes {fb}");
        assert!(bitrate_kbps(h) > 0);
        assert_eq!(frame_samples(h), 1152);
    }

    #[test]
    fn duration_within_5_percent() {
        let dec = match Mp3Decoder::open(std::path::Path::new("tests/fixtures/tiny.mp3")) {
            Ok(d) => d,
            Err(_) => return,
        };
        let secs = dec.duration as f64 / dec.rate as f64;
        assert!((secs - 2.0).abs() / 2.0 < 0.05, "duration {secs}s");
    }

    #[test]
    fn decodes_tiny_within_rms_tolerance() {
        let dec = match Mp3Decoder::open(std::path::Path::new("tests/fixtures/tiny.mp3")) {
            Ok(d) => d,
            Err(_) => return,
        };
        let want_bytes = std::fs::read("tests/fixtures/tiny_mp3.f32le").unwrap();
        let want: Vec<f32> = want_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();

        let mut dec = dec;
        let mut got: Vec<f32> = Vec::new();
        while let Some(frame) = dec.next() {
            got.extend_from_slice(&frame);
        }
        assert!(got.len() > 48000, "decoded too little: {}", got.len());

        let ch = dec.channels as i64;
        let mut best = f64::INFINITY;
        for lag in -1200i64..=1200 {
            let shift = lag * ch;
            let mut num = 0.0f64;
            let mut den = 0.0f64;
            let mut count = 0usize;
            let n = want.len().min(got.len());
            for j in 0..n {
                let gi = j as i64 + shift;
                if gi < 0 || gi as usize >= got.len() {
                    continue;
                }
                let d = (got[gi as usize] - want[j]) as f64;
                num += d * d;
                den += (want[j] as f64).powi(2);
                count += 1;
            }
            if count > 40000 {
                let rms = (num / den.max(1e-9)).sqrt();
                if rms < best {
                    best = rms;
                }
            }
        }
        assert!(best < 0.05, "min relative RMS {best} too high");
    }

    #[test]
    fn fuzz_framing_never_panics() {
        crate::fuzz::each_case(8000, 512, |data| {
            let id3 = skip_id3v2(data).min(data.len());
            let mut ff = 0usize;
            let _ = find_frame(&data[id3..], &mut ff);
        });
    }

    #[test]
    fn fuzz_full_decode_never_panics() {
        crate::fuzz::each_case(6000, 1024, |data| {
            if let Ok(mut dec) = Mp3Decoder::from_bytes(data.to_vec()) {
                for _ in 0..16 {
                    match dec.next() {
                        Some(frame) => assert!(frame.iter().all(|v| v.is_finite())),
                        None => break,
                    }
                }
            }
        });
    }

    #[test]
    fn fuzz_full_decode_with_frame_sync_prefix() {
        crate::fuzz::each_case(6000, 1024, |data| {
            let mut framed = Vec::with_capacity(data.len() + 2);
            framed.push(0xff);
            framed.push(0xfb);
            framed.extend_from_slice(data);
            if let Ok(mut dec) = Mp3Decoder::from_bytes(framed) {
                for _ in 0..16 {
                    match dec.next() {
                        Some(frame) => assert!(frame.iter().all(|v| v.is_finite())),
                        None => break,
                    }
                }
            }
        });
    }
}
