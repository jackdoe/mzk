use crate::error::{Error, Result};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FrameMode {
    Silk,
    Hybrid,
    Celt,
}

#[derive(Clone, Copy, Debug)]
pub struct Toc {
    pub mode: FrameMode,
    pub stereo: bool,
    pub lm: u8,
    pub end: u8,
    pub samples: u32,
}

const MAX_FRAME: usize = 1275;

impl Toc {
    pub fn parse(toc: u8) -> Toc {
        let config = toc >> 3;
        let stereo = (toc >> 2) & 1 == 1;
        if config < 12 {
            let ms = [10u32, 20, 40, 60][(config & 3) as usize];
            Toc { mode: FrameMode::Silk, stereo, lm: 0, end: 0, samples: 48 * ms }
        } else if config < 16 {
            let ms = if config & 1 == 0 { 10 } else { 20 };
            Toc { mode: FrameMode::Hybrid, stereo, lm: 0, end: 0, samples: 48 * ms }
        } else {
            let lm = (config & 3) as u8;
            let end = [13u8, 17, 19, 21][((config - 16) / 4) as usize];
            Toc { mode: FrameMode::Celt, stereo, lm, end, samples: 120u32 << lm }
        }
    }
}

fn read_len(b: &[u8]) -> Result<(usize, usize)> {
    match b.first() {
        None => Err(Error::BadOpus("frame length truncated")),
        Some(&b0) if b0 < 252 => Ok((b0 as usize, 1)),
        Some(&b0) => match b.get(1) {
            Some(&b1) => Ok((b0 as usize + 4 * b1 as usize, 2)),
            None => Err(Error::BadOpus("frame length truncated")),
        },
    }
}

pub fn split_frames(pkt: &[u8]) -> Result<(Toc, Vec<(usize, usize)>)> {
    if pkt.is_empty() {
        return Err(Error::BadOpus("empty packet"));
    }
    let toc = Toc::parse(pkt[0]);
    let code = pkt[0] & 3;
    let body = &pkt[1..];
    let mut frames = Vec::new();

    match code {
        0 => frames.push((1, body.len())),
        1 => {
            if body.len() % 2 != 0 {
                return Err(Error::BadOpus("code 1 odd length"));
            }
            let h = body.len() / 2;
            frames.push((1, h));
            frames.push((1 + h, h));
        }
        2 => {
            let (n, used) = read_len(body)?;
            if used + n > body.len() {
                return Err(Error::BadOpus("code 2 overrun"));
            }
            frames.push((1 + used, n));
            frames.push((1 + used + n, body.len() - used - n));
        }
        _ => {
            let fc = *body.first().ok_or(Error::BadOpus("code 3 missing count"))?;
            let vbr = fc & 0x80 != 0;
            let pad = fc & 0x40 != 0;
            let m = (fc & 0x3f) as usize;
            if m == 0 || m as u32 * toc.samples > 48 * 120 {
                return Err(Error::BadOpus("code 3 frame count"));
            }
            let mut p = 1usize;
            let mut padding = 0usize;
            if pad {
                loop {
                    let b = *body.get(p).ok_or(Error::BadOpus("code 3 padding"))?;
                    p += 1;
                    if b == 255 {
                        padding += 254;
                    } else {
                        padding += b as usize;
                        break;
                    }
                }
            }
            let data_end = body
                .len()
                .checked_sub(padding)
                .ok_or(Error::BadOpus("code 3 padding > body"))?;
            if vbr {
                let mut total = 0usize;
                let mut lens = Vec::with_capacity(m);
                for _ in 0..m - 1 {
                    let (n, used) = read_len(body.get(p..data_end).unwrap_or(&[]))?;
                    p += used;
                    total += n;
                    lens.push(n);
                }
                let last = data_end
                    .checked_sub(p)
                    .and_then(|r| r.checked_sub(total))
                    .ok_or(Error::BadOpus("code 3 vbr overrun"))?;
                lens.push(last);
                let mut off = p;
                for n in lens {
                    frames.push((1 + off, n));
                    off += n;
                }
            } else {
                let region = data_end
                    .checked_sub(p)
                    .ok_or(Error::BadOpus("code 3 cbr overrun"))?;
                if region % m != 0 {
                    return Err(Error::BadOpus("code 3 cbr not divisible"));
                }
                let n = region / m;
                let mut off = p;
                for _ in 0..m {
                    frames.push((1 + off, n));
                    off += n;
                }
            }
        }
    }

    for &(start, n) in &frames {
        if n > MAX_FRAME || start.checked_add(n).map_or(true, |e| e > pkt.len()) {
            return Err(Error::BadOpus("frame out of bounds"));
        }
    }
    Ok((toc, frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config31_is_celt_fb_20ms_stereo() {
        let t = Toc::parse(0b11111_1_00);
        assert_eq!(t.mode, FrameMode::Celt);
        assert!(t.stereo);
        assert_eq!(t.lm, 3);
        assert_eq!(t.end, 21);
        assert_eq!(t.samples, 960);
    }

    #[test]
    fn celt_bandwidths_and_sizes() {
        assert_eq!(Toc::parse(16 << 3).end, 13);
        assert_eq!(Toc::parse(20 << 3).end, 17);
        assert_eq!(Toc::parse(24 << 3).end, 19);
        assert_eq!(Toc::parse(28 << 3).end, 21);
        assert_eq!(Toc::parse(16 << 3).samples, 120);
        assert_eq!(Toc::parse(17 << 3).samples, 240);
        assert_eq!(Toc::parse(18 << 3).samples, 480);
        assert_eq!(Toc::parse(19 << 3).samples, 960);
    }

    #[test]
    fn silk_and_hybrid_modes_and_durations() {
        assert_eq!(Toc::parse(0).mode, FrameMode::Silk);
        assert_eq!(Toc::parse(0).samples, 480);
        assert_eq!(Toc::parse(3 << 3).samples, 2880);
        assert_eq!(Toc::parse(8 << 3).mode, FrameMode::Silk);
        assert_eq!(Toc::parse(12 << 3).mode, FrameMode::Hybrid);
        assert_eq!(Toc::parse(12 << 3).samples, 480);
        assert_eq!(Toc::parse(13 << 3).samples, 960);
    }

    #[test]
    fn code0_single_frame() {
        let pkt = [0b11111_0_00, 1, 2, 3, 4];
        let (_, f) = split_frames(&pkt).unwrap();
        assert_eq!(f, vec![(1, 4)]);
    }

    #[test]
    fn code1_two_equal_frames() {
        let pkt = [0b11111_0_01, 1, 2, 3, 4];
        let (_, f) = split_frames(&pkt).unwrap();
        assert_eq!(f, vec![(1, 2), (3, 2)]);
        assert!(split_frames(&[0b11111_0_01, 1, 2, 3]).is_err());
    }

    #[test]
    fn code2_explicit_length() {
        let pkt = [0b11111_0_10, 2, 10, 20, 30, 40];
        let (_, f) = split_frames(&pkt).unwrap();
        assert_eq!(f, vec![(2, 2), (4, 2)]);
    }

    #[test]
    fn code3_cbr() {
        let pkt = [0b11111_0_11, 3, 1, 2, 3, 4, 5, 6];
        let (_, f) = split_frames(&pkt).unwrap();
        assert_eq!(f, vec![(2, 2), (4, 2), (6, 2)]);
    }

    #[test]
    fn code3_vbr_with_padding() {
        let pkt = [0b11111_0_11, 0x80 | 0x40 | 3, 1, 2, 3, 10, 20, 30, 40, 50, 0, 0];
        let (_, f) = split_frames(&pkt).unwrap();
        let lens: Vec<usize> = f.iter().map(|&(_, n)| n).collect();
        assert_eq!(lens, vec![2, 3, 1]);
    }

    #[test]
    fn fuzz_never_panics() {
        crate::fuzz::each_case(8000, 64, |data| {
            let _ = split_frames(data);
            if !data.is_empty() {
                let _ = Toc::parse(data[0]);
            }
        });
    }
}
