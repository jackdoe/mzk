use super::tables::{BITRES, QTHETA_OFFSET, QTHETA_OFFSET_TWOPHASE};
use super::Mode;
use crate::opus::range::RangeDecoder;

const EMEANS: [f32; 25] = [
    6.437500, 6.250000, 5.750000, 5.312500, 5.062500, 4.812500, 4.500000, 4.375000, 4.875000,
    4.687500, 4.562500, 4.437500, 4.875000, 4.625000, 4.312500, 4.500000, 4.375000, 4.625000,
    4.750000, 4.437500, 3.750000, 3.750000, 3.750000, 3.750000, 3.750000,
];

const SPREAD_AGGRESSIVE: i32 = 3;

const EXP2_TABLE8: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];

const ORDERY_TABLE: [usize; 30] = [
    1, 0, 3, 0, 2, 1, 7, 0, 4, 3, 6, 1, 5, 2, 15, 0, 8, 7, 12, 3, 11, 4, 14, 1, 9, 6, 13, 2, 10, 5,
];

const BIT_INTERLEAVE_TABLE: [u8; 16] = [0, 1, 1, 1, 2, 3, 3, 3, 2, 3, 3, 3, 2, 3, 3, 3];

const BIT_DEINTERLEAVE_TABLE: [u8; 16] = [
    0x00, 0x03, 0x0C, 0x0F, 0x30, 0x33, 0x3C, 0x3F, 0xC0, 0xC3, 0xCC, 0xCF, 0xF0, 0xF3, 0xFC, 0xFF,
];

const MAX_BAND_N: usize = 176;

pub fn denormalise_bands(
    mode: &Mode,
    x: &[f32],
    freq: &mut [f32],
    band_log_e: &[f32],
    start: usize,
    end: usize,
    m: usize,
    silence: bool,
) {
    let eb = &mode.e_bands;
    let n = m * mode.short_mdct;
    let mut bound = m * eb[end] as usize;
    let lead = m * eb[start] as usize;
    if silence {
        bound = 0;
    }
    for v in freq[..lead].iter_mut() {
        *v = 0.0;
    }
    if !silence {
        for i in start..end {
            let j = m * eb[i] as usize;
            let band_end = m * eb[i + 1] as usize;
            let lg = band_log_e[i] + EMEANS[i];
            let g = (lg.min(32.0)).exp2();
            for k in j..band_end {
                freq[k] = x[k] * g;
            }
        }
    }
    for v in freq[bound..n].iter_mut() {
        *v = 0.0;
    }
}

fn celt_lcg_rand(seed: u32) -> u32 {
    1664525u32.wrapping_mul(seed).wrapping_add(1013904223)
}

fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + a * b) >> 15
}

fn bitexact_cos(x: i32) -> i32 {
    let tmp = (4096 + x * x) >> 13;
    let mut x2 = tmp;
    x2 = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    1 + x2
}

fn ec_ilog(x: u32) -> i32 {
    32 - x.leading_zeros() as i32
}

fn bitexact_log2tan(isin: i32, icos: i32) -> i32 {
    let lc = ec_ilog(icos as u32);
    let ls = ec_ilog(isin as u32);
    let icos = icos << (15 - lc);
    let isin = isin << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(isin, frac_mul16(isin, -2597) + 7932)
        - frac_mul16(icos, frac_mul16(icos, -2597) + 7932)
}

fn isqrt32(val: u32) -> u32 {
    let mut g = 0u32;
    let mut bshift = ((ec_ilog(val) - 1) >> 1) as i32;
    let mut b = 1u32 << bshift;
    let mut val = val;
    loop {
        if val >= (g + b) << bshift {
            val -= (g + b) << bshift;
            g += b << 1;
        }
        b >>= 1;
        bshift -= 1;
        if bshift < 0 {
            break;
        }
    }
    g >> 1
}

fn compute_qn(n: i32, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    let mut n2 = 2 * n - 1;
    if stereo && n == 2 {
        n2 -= 1;
    }
    let mut qb = (b + n2 * offset) / n2;
    qb = qb.min(b - pulse_cap - (4 << BITRES));
    qb = qb.min(8 << BITRES);
    if qb < (1 << BITRES >> 1) {
        1
    } else {
        let mut qn = EXP2_TABLE8[(qb & 0x7) as usize] >> (14 - (qb >> BITRES));
        qn = (qn + 1) >> 1 << 1;
        qn
    }
}

fn dual_inner_prod(t: &[f32], x: &[f32], y: &[f32], n: usize) -> (f32, f32) {
    let mut xy = 0.0;
    let mut yy = 0.0;
    for j in 0..n {
        xy += t[j] * x[j];
        yy += t[j] * y[j];
    }
    (xy, yy)
}

fn stereo_merge(x: &mut [f32], y: &mut [f32], mid: f32, n: usize) {
    let (xp0, side) = dual_inner_prod(y, x, y, n);
    let xp = mid * xp0;
    let el = mid * mid + side - 2.0 * xp;
    let er = mid * mid + side + 2.0 * xp;
    if er < 6e-4 || el < 6e-4 {
        y[..n].copy_from_slice(&x[..n]);
        return;
    }
    let lgain = 1.0 / el.sqrt();
    let rgain = 1.0 / er.sqrt();
    for j in 0..n {
        let l = mid * x[j];
        let r = y[j];
        x[j] = lgain * (l - r);
        y[j] = rgain * (l + r);
    }
}

fn deinterleave_hadamard(x: &mut [f32], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    let mut tmp = [0.0f32; MAX_BAND_N];
    let tmp = &mut tmp[..n];
    if hadamard {
        let ordery = &ORDERY_TABLE[stride - 2..];
        for i in 0..stride {
            for j in 0..n0 {
                tmp[ordery[i] * n0 + j] = x[j * stride + i];
            }
        }
    } else {
        for i in 0..stride {
            for j in 0..n0 {
                tmp[i * n0 + j] = x[j * stride + i];
            }
        }
    }
    x[..n].copy_from_slice(&tmp);
}

fn interleave_hadamard(x: &mut [f32], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    let mut tmp = [0.0f32; MAX_BAND_N];
    let tmp = &mut tmp[..n];
    if hadamard {
        let ordery = &ORDERY_TABLE[stride - 2..];
        for i in 0..stride {
            for j in 0..n0 {
                tmp[j * stride + i] = x[ordery[i] * n0 + j];
            }
        }
    } else {
        for i in 0..stride {
            for j in 0..n0 {
                tmp[j * stride + i] = x[i * n0 + j];
            }
        }
    }
    x[..n].copy_from_slice(&tmp);
}

fn haar1(x: &mut [f32], n0: usize, stride: usize) {
    let n0 = n0 >> 1;
    for i in 0..stride {
        for j in 0..n0 {
            let tmp1 = 0.70710678 * x[stride * 2 * j + i];
            let tmp2 = 0.70710678 * x[stride * (2 * j + 1) + i];
            x[stride * 2 * j + i] = tmp1 + tmp2;
            x[stride * (2 * j + 1) + i] = tmp1 - tmp2;
        }
    }
}

struct Split {
    inv: bool,
    mid: f32,
    side: f32,
    delta: i32,
    theta: i32,
    qalloc: i32,
    b: i32,
    fill: i32,
}

struct Ctx<'m, 'f> {
    m: &'m Mode,
    rd: &'m mut RangeDecoder<'f>,
    seed: u32,
    spread: i32,
    intensity: usize,
    band: usize,
    tf_change: i32,
    remaining_bits: i32,
}

impl Ctx<'_, '_> {
    fn decode_theta(
        &mut self,
        n: usize,
        mut b: i32,
        bparam: i32,
        b0: i32,
        lm: i32,
        stereo: bool,
        mut fill: i32,
    ) -> Split {
        let pulse_cap = self.m.log_n[self.band] + lm * (1 << BITRES);
        let offset = (pulse_cap >> 1)
            - if stereo && n == 2 {
                QTHETA_OFFSET_TWOPHASE
            } else {
                QTHETA_OFFSET
            };
        let mut qn = compute_qn(n as i32, b, offset, pulse_cap, stereo);
        if stereo && self.band >= self.intensity {
            qn = 1;
        }
        let tell = self.rd.tell_frac() as i32;
        let mut theta = 0i32;
        let mut inv = false;

        if qn != 1 {
            let qh = qn >> 1;
            if stereo && n > 2 {
                let p0 = 3i32;
                let x0 = qn / 2;
                let ft = p0 * (x0 + 1) + x0;
                let fs = self.rd.decode(ft as u32) as i32;
                let xv = if fs < (x0 + 1) * p0 {
                    fs / p0
                } else {
                    x0 + 1 + (fs - (x0 + 1) * p0)
                };
                let fl = if xv <= x0 {
                    p0 * xv
                } else {
                    (xv - 1 - x0) + (x0 + 1) * p0
                };
                let fh = if xv <= x0 {
                    p0 * (xv + 1)
                } else {
                    (xv - x0) + (x0 + 1) * p0
                };
                self.rd.update(fl as u32, fh as u32, ft as u32);
                theta = xv;
            } else if b0 > 1 || stereo {
                theta = self.rd.dec_uint((qn + 1) as u32) as i32;
            } else {
                let ft = (qh + 1) * (qh + 1);
                let fm = self.rd.decode(ft as u32) as i32;
                let fl;
                let fs;
                if fm < (qh * (qh + 1) >> 1) {
                    theta = ((isqrt32(8 * fm as u32 + 1) as i32) - 1) >> 1;
                    fs = theta + 1;
                    fl = theta * (theta + 1) >> 1;
                } else {
                    theta = (2 * (qn + 1) - isqrt32(8 * (ft - fm - 1) as u32 + 1) as i32) >> 1;
                    fs = qn + 1 - theta;
                    fl = ft - ((qn + 1 - theta) * (qn + 2 - theta) >> 1);
                }
                self.rd.update(fl as u32, (fl + fs) as u32, ft as u32);
            }
            theta = (theta * 16384) / qn;
        } else if stereo {
            inv = b > 2 << BITRES && self.remaining_bits > 2 << BITRES && self.rd.dec_bit_logp(2) != 0;
            theta = 0;
        }

        let qalloc = self.rd.tell_frac() as i32 - tell;
        b -= qalloc;

        let imid;
        let iside;
        let delta;
        if theta == 0 {
            imid = 32767;
            iside = 0;
            fill &= (1 << bparam) - 1;
            delta = -16384;
        } else if theta == 16384 {
            imid = 0;
            iside = 32767;
            fill &= ((1 << bparam) - 1) << bparam;
            delta = 16384;
        } else {
            imid = bitexact_cos(theta);
            iside = bitexact_cos(16384 - theta);
            delta = frac_mul16(((n as i32) - 1) << 7, bitexact_log2tan(iside, imid));
        }

        Split {
            inv,
            mid: (1.0 / 32768.0) * imid as f32,
            side: (1.0 / 32768.0) * iside as f32,
            delta,
            theta,
            qalloc,
            b,
            fill,
        }
    }

    fn decode_unit_band(
        &mut self,
        x: &mut [f32],
        y: Option<&mut [f32]>,
        lowband_out: Option<&mut [f32]>,
    ) -> u32 {
        let stereo = y.is_some();
        let mut chans: [Option<&mut [f32]>; 2] = [Some(x), y];
        let count = if stereo { 2 } else { 1 };
        let mut x0_first = 0.0f32;
        for (c, ch) in chans.iter_mut().enumerate().take(count) {
            let mut sign = 0u32;
            if self.remaining_bits >= 1 << BITRES {
                sign = self.rd.dec_bits(1);
                self.remaining_bits -= 1 << BITRES;
            }
            let v = if sign != 0 { -1.0 } else { 1.0 };
            if let Some(ch) = ch.as_mut() {
                ch[0] = v;
                if c == 0 {
                    x0_first = v;
                }
            }
        }
        if let Some(lo) = lowband_out {
            lo[0] = x0_first * 0.0625;
        }
        1
    }

    fn split_band(
        &mut self,
        x: &mut [f32],
        mut n: usize,
        mut b: i32,
        mut bparam: i32,
        lowband: Option<&[f32]>,
        mut lm: i32,
        gain: f32,
        mut fill: i32,
    ) -> u32 {
        let b0 = bparam;
        let band = self.band;
        let cache_base = self.m.cache.index[((lm + 1) as usize) * self.m.nb_ebands + band] as usize;
        let cache0 = self.m.cache.bits[cache_base] as usize;
        let cache_top = self.m.cache.bits[cache_base + cache0] as i32;

        if lm != -1 && b > cache_top + 12 && n > 2 {
            n >>= 1;
            let (xl, xr) = x.split_at_mut(n);
            lm -= 1;
            if bparam == 1 {
                fill = (fill & 1) | (fill << 1);
            }
            bparam = (bparam + 1) >> 1;

            let s = self.decode_theta(n, b, bparam, b0, lm, false, fill);
            let mut delta = s.delta;
            let theta = s.theta;
            b = s.b;
            fill = s.fill;

            if b0 > 1 && (theta & 0x3fff) != 0 {
                if theta > 8192 {
                    delta -= delta >> (4 - lm);
                } else {
                    delta = 0.min(delta + ((n as i32) << BITRES >> (5 - lm)));
                }
            }
            let mut mid_bits = 0.max(b.min((b - delta) / 2));
            let mut side_bits = b - mid_bits;
            self.remaining_bits -= s.qalloc;

            let next_lowband2: Option<&[f32]> = lowband.map(|lb| &lb[n..]);
            let mut collapse_mask;
            let rebalance = self.remaining_bits;
            if mid_bits >= side_bits {
                collapse_mask = self.split_band(xl, n, mid_bits, bparam, lowband, lm, gain * s.mid, fill);
                let bal = mid_bits - (rebalance - self.remaining_bits);
                if bal > 3 << BITRES && theta != 0 {
                    side_bits += bal - (3 << BITRES);
                }
                collapse_mask |= self.split_band(xr, n, side_bits, bparam, next_lowband2, lm, gain * s.side, fill >> bparam)
                    << (b0 >> 1);
            } else {
                collapse_mask = self.split_band(xr, n, side_bits, bparam, next_lowband2, lm, gain * s.side, fill >> bparam)
                    << (b0 >> 1);
                let bal = side_bits - (rebalance - self.remaining_bits);
                if bal > 3 << BITRES && theta != 16384 {
                    mid_bits += bal - (3 << BITRES);
                }
                collapse_mask |= self.split_band(xl, n, mid_bits, bparam, lowband, lm, gain * s.mid, fill);
            }
            collapse_mask
        } else {
            self.quantise_band(x, n, b, bparam, lowband, lm, gain, fill)
        }
    }

    fn quantise_band(
        &mut self,
        x: &mut [f32],
        n: usize,
        b: i32,
        bparam: i32,
        lowband: Option<&[f32]>,
        lm: i32,
        gain: f32,
        fill: i32,
    ) -> u32 {
        let band = self.band;
        let mut q = super::rate::bits2pulses(&self.m.cache, self.m.nb_ebands, band, lm, b);
        let mut curr_bits = super::rate::pulses2bits(&self.m.cache, self.m.nb_ebands, band, lm, q);
        self.remaining_bits -= curr_bits;
        while self.remaining_bits < 0 && q > 0 {
            self.remaining_bits += curr_bits;
            q -= 1;
            curr_bits = super::rate::pulses2bits(&self.m.cache, self.m.nb_ebands, band, lm, q);
            self.remaining_bits -= curr_bits;
        }

        if q != 0 {
            let k = super::cwrs::get_pulses(q);
            return super::vq::alg_unquant(x, n, k, self.spread, bparam as usize, self.rd, gain);
        }

        let cm_mask = ((1u32 << bparam) - 1) as i32;
        let fill = fill & cm_mask;
        if fill == 0 {
            for v in x[..n].iter_mut() {
                *v = 0.0;
            }
            return 0;
        }

        let collapse_mask;
        match lowband {
            None => {
                for v in x[..n].iter_mut() {
                    self.seed = celt_lcg_rand(self.seed);
                    *v = (self.seed as i32 >> 20) as f32;
                }
                collapse_mask = cm_mask as u32;
            }
            Some(lb) => {
                for j in 0..n {
                    self.seed = celt_lcg_rand(self.seed);
                    let tmp = if self.seed & 0x8000 != 0 {
                        1.0 / 256.0
                    } else {
                        -1.0 / 256.0
                    };
                    x[j] = lb[j] + tmp;
                }
                collapse_mask = fill as u32;
            }
        }
        super::vq::renormalise_vector(x, n, gain);
        collapse_mask
    }

    fn decode_band(
        &mut self,
        x: &mut [f32],
        n: usize,
        b: i32,
        bparam: i32,
        mut lowband: Option<&mut [f32]>,
        lm: i32,
        lowband_out: Option<&mut [f32]>,
        gain: f32,
        mut fill: i32,
    ) -> u32 {
        let n0 = n;
        let b0 = bparam;
        let mut bparam = bparam;
        let long_blocks = b0 == 1;
        let mut tf_change = self.tf_change;
        let mut n_b = n / bparam as usize;

        if n == 1 {
            return self.decode_unit_band(x, None, lowband_out);
        }

        let mut recombine = 0i32;
        if tf_change > 0 {
            recombine = tf_change;
        }

        for k in 0..recombine {
            if let Some(lo) = lowband.as_deref_mut() {
                haar1(lo, n >> k, 1 << k);
            }
            fill = BIT_INTERLEAVE_TABLE[(fill & 0xF) as usize] as i32
                | (BIT_INTERLEAVE_TABLE[(fill >> 4) as usize] as i32) << 2;
        }
        bparam >>= recombine;
        n_b <<= recombine;

        let mut time_divide = 0i32;
        while (n_b & 1) == 0 && tf_change < 0 {
            if let Some(lo) = lowband.as_deref_mut() {
                haar1(lo, n_b, bparam as usize);
            }
            fill |= fill << bparam;
            bparam <<= 1;
            n_b >>= 1;
            time_divide += 1;
            tf_change += 1;
        }
        let split_blocks = bparam;
        let n_b0 = n_b;

        if split_blocks > 1 {
            if let Some(lo) = lowband.as_deref_mut() {
                deinterleave_hadamard(lo, n_b >> recombine, (split_blocks << recombine) as usize, long_blocks);
            }
        }

        let mut collapse_mask = self.split_band(x, n, b, split_blocks, lowband.as_deref(), lm, gain, fill);

        if split_blocks > 1 {
            interleave_hadamard(x, n_b >> recombine, (split_blocks << recombine) as usize, long_blocks);
        }

        let mut n_b = n_b0;
        let mut blocks = split_blocks;
        for _ in 0..time_divide {
            blocks >>= 1;
            n_b <<= 1;
            collapse_mask |= collapse_mask >> blocks;
            haar1(x, n_b, blocks as usize);
        }
        for k in 0..recombine {
            collapse_mask = BIT_DEINTERLEAVE_TABLE[collapse_mask as usize] as u32;
            haar1(x, n0 >> k, 1 << k);
        }
        blocks <<= recombine;

        if let Some(lo) = lowband_out {
            let scale = ((n0 as f32) * 4194304.0).sqrt();
            for j in 0..n0 {
                lo[j] = scale * x[j];
            }
        }
        collapse_mask & ((1 << blocks) - 1)
    }

    fn decode_band_stereo(
        &mut self,
        x: &mut [f32],
        y: &mut [f32],
        n: usize,
        b: i32,
        bparam: i32,
        lowband: Option<&mut [f32]>,
        lm: i32,
        lowband_out: Option<&mut [f32]>,
        mut fill: i32,
    ) -> u32 {
        if n == 1 {
            return self.decode_unit_band(x, Some(y), lowband_out);
        }

        let orig_fill = fill;
        let s = self.decode_theta(n, b, bparam, bparam, lm, true, fill);
        fill = s.fill;
        let b = s.b;
        let theta = s.theta;
        let mid = s.mid;
        let side = s.side;

        let mut collapse_mask;
        if n == 2 {
            let side_bits = if theta != 0 && theta != 16384 {
                1 << BITRES
            } else {
                0
            };
            let mid_bits = b - side_bits;
            let swap = theta > 8192;
            self.remaining_bits -= s.qalloc + side_bits;

            let signf = if side_bits != 0 && self.rd.dec_bits(1) != 0 {
                -1.0
            } else {
                1.0
            };

            {
                let (x2, y2): (&mut [f32], &mut [f32]) = if swap { (y, x) } else { (x, y) };
                collapse_mask = self.decode_band(
                    x2, n, mid_bits, bparam, lowband, lm, lowband_out, 1.0, orig_fill,
                );
                y2[0] = -signf * x2[1];
                y2[1] = signf * x2[0];
            }

            x[0] *= mid;
            x[1] *= mid;
            y[0] *= side;
            y[1] *= side;
            let tmp0 = x[0];
            x[0] = tmp0 - y[0];
            y[0] = tmp0 + y[0];
            let tmp1 = x[1];
            x[1] = tmp1 - y[1];
            y[1] = tmp1 + y[1];
        } else {
            let mut mid_bits = 0.max(b.min((b - s.delta) / 2));
            let mut side_bits = b - mid_bits;
            self.remaining_bits -= s.qalloc;

            let rebalance = self.remaining_bits;
            if mid_bits >= side_bits {
                collapse_mask = self.decode_band(
                    x, n, mid_bits, bparam, lowband, lm, lowband_out, 1.0, fill,
                );
                let bal = mid_bits - (rebalance - self.remaining_bits);
                if bal > 3 << BITRES && theta != 0 {
                    side_bits += bal - (3 << BITRES);
                }
                collapse_mask |=
                    self.decode_band(y, n, side_bits, bparam, None, lm, None, side, fill >> bparam);
            } else {
                collapse_mask =
                    self.decode_band(y, n, side_bits, bparam, None, lm, None, side, fill >> bparam);
                let bal = side_bits - (rebalance - self.remaining_bits);
                if bal > 3 << BITRES && theta != 16384 {
                    mid_bits += bal - (3 << BITRES);
                }
                collapse_mask |= self.decode_band(
                    x, n, mid_bits, bparam, lowband, lm, lowband_out, 1.0, fill,
                );
            }
        }

        if n != 2 {
            stereo_merge(x, y, mid, n);
        }
        if s.inv {
            for v in y[..n].iter_mut() {
                *v = -*v;
            }
        }
        collapse_mask
    }

    fn fold_region(
        &self,
        lowband_offset: usize,
        n: usize,
        norm_offset: usize,
        big_m: usize,
        bblocks: i32,
        chans: usize,
        collapse_masks: &[u8],
    ) -> (Option<usize>, u32, u32) {
        let eb = &self.m.e_bands;
        if lowband_offset == 0
            || (self.spread == SPREAD_AGGRESSIVE && bblocks <= 1 && self.tf_change >= 0)
        {
            let full = (1u32 << bblocks) - 1;
            return (None, full, full);
        }

        let el = (big_m * eb[lowband_offset] as usize)
            .saturating_sub(norm_offset)
            .saturating_sub(n);

        let mut fold_start = lowband_offset;
        loop {
            fold_start -= 1;
            if big_m * eb[fold_start] as usize <= el + norm_offset {
                break;
            }
        }
        let mut fold_end = lowband_offset - 1;
        loop {
            fold_end += 1;
            if !(fold_end < self.band && (big_m * eb[fold_end] as usize) < el + norm_offset + n) {
                break;
            }
        }

        let mut x_cm = 0u32;
        let mut y_cm = 0u32;
        let mut fi = fold_start;
        loop {
            x_cm |= collapse_masks[fi * chans] as u32;
            y_cm |= collapse_masks[fi * chans + chans - 1] as u32;
            fi += 1;
            if fi >= fold_end {
                break;
            }
        }
        (Some(el), x_cm, y_cm)
    }

    fn decode_into_norm(
        &mut self,
        x: &mut [f32],
        n: usize,
        b: i32,
        bblocks: i32,
        effective_lowband: Option<usize>,
        norm: &mut [f32],
        chan_off: usize,
        lm: i32,
        out_index: usize,
        last: bool,
        lowband_buf: &mut [f32],
        fill: u32,
    ) -> u32 {
        let lowband: Option<&mut [f32]> = match effective_lowband {
            Some(el) => {
                lowband_buf[..n].copy_from_slice(&norm[chan_off + el..chan_off + el + n]);
                Some(&mut lowband_buf[..n])
            }
            None => None,
        };
        let out = chan_off + out_index;
        let lowband_out = if last { None } else { Some(&mut norm[out..out + n]) };
        self.decode_band(x, n, b, bblocks, lowband, lm, lowband_out, 1.0, fill as i32)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn quant_all_bands(
    mode: &Mode,
    start: usize,
    end: usize,
    x: &mut [f32],
    y: Option<&mut [f32]>,
    collapse_masks: &mut [u8],
    pulses: &[i32],
    short_blocks: i32,
    spread: i32,
    dual_stereo_in: bool,
    intensity: usize,
    tf_res: &[i32],
    total_bits: i32,
    balance_in: i32,
    rd: &mut RangeDecoder,
    lm: i32,
    coded_bands: usize,
    seed: &mut u32,
) {
    let eb = &mode.e_bands;
    let big_m = 1usize << lm;
    let bblocks = if short_blocks != 0 { big_m as i32 } else { 1 };
    let norm_offset = big_m * eb[start] as usize;
    let stereo = y.is_some();
    let chans = if stereo { 2 } else { 1 };

    let norm_len = big_m * eb[mode.nb_ebands - 1] as usize - norm_offset;
    let mut norm = vec![0.0f32; chans * norm_len];
    let mut lowband_buf = [0.0f32; MAX_BAND_N];

    let x_buf = x;
    let mut y_buf = y;
    let mut balance = balance_in;
    let mut dual_stereo = dual_stereo_in;
    let mut lowband_offset = 0usize;
    let mut update_lowband = true;

    let mut ctx = Ctx {
        m: mode,
        rd,
        seed: *seed,
        spread,
        intensity,
        band: start,
        tf_change: 0,
        remaining_bits: 0,
    };

    for i in start..end {
        ctx.band = i;
        let last = i == end - 1;
        let bx = big_m * eb[i] as usize;
        let n = big_m * eb[i + 1] as usize - bx;
        let tell = ctx.rd.tell_frac() as i32;

        if i != start {
            balance -= tell;
        }
        let remaining_bits = total_bits - tell - 1;
        ctx.remaining_bits = remaining_bits;

        let b = if i < coded_bands {
            let curr_balance = balance / (3.min(coded_bands - i) as i32);
            0.max(16383.min((remaining_bits + 1).min(pulses[i] + curr_balance)))
        } else {
            0
        };

        if (bx >= norm_offset + n || i == start + 1) && (update_lowband || lowband_offset == 0) {
            lowband_offset = i;
        }

        ctx.tf_change = tf_res[i];

        let (effective_lowband, mut x_cm, mut y_cm) =
            ctx.fold_region(lowband_offset, n, norm_offset, big_m, bblocks, chans, collapse_masks);

        if dual_stereo && i == intensity {
            dual_stereo = false;
            let limit = bx - norm_offset;
            for j in 0..limit {
                norm[j] = 0.5 * (norm[j] + norm[norm_len + j]);
            }
        }

        let out_index = bx - norm_offset;

        if dual_stereo {
            {
                let xc = &mut x_buf[bx..bx + n];
                x_cm = ctx.decode_into_norm(
                    xc, n, b / 2, bblocks, effective_lowband, &mut norm, 0, lm, out_index, last,
                    &mut lowband_buf, x_cm,
                );
            }
            let yref = y_buf.as_deref_mut().unwrap();
            let yc = &mut yref[bx..bx + n];
            y_cm = ctx.decode_into_norm(
                yc, n, b / 2, bblocks, effective_lowband, &mut norm, norm_len, lm, out_index, last,
                &mut lowband_buf, y_cm,
            );
        } else if stereo {
            let yref = y_buf.as_deref_mut().unwrap();
            let lowband: Option<&mut [f32]> = match effective_lowband {
                Some(el) => {
                    lowband_buf[..n].copy_from_slice(&norm[el..el + n]);
                    Some(&mut lowband_buf[..n])
                }
                None => None,
            };
            let xc = &mut x_buf[bx..bx + n];
            let yc = &mut yref[bx..bx + n];
            let out = if last {
                None
            } else {
                Some(&mut norm[out_index..out_index + n])
            };
            x_cm = ctx.decode_band_stereo(
                xc,
                yc,
                n,
                b,
                bblocks,
                lowband,
                lm,
                out,
                (x_cm | y_cm) as i32,
            );
            y_cm = x_cm;
        } else {
            let xc = &mut x_buf[bx..bx + n];
            x_cm = ctx.decode_into_norm(
                xc, n, b, bblocks, effective_lowband, &mut norm, 0, lm, out_index, last,
                &mut lowband_buf, x_cm | y_cm,
            );
            y_cm = x_cm;
        }

        collapse_masks[i * chans] = x_cm as u8;
        collapse_masks[i * chans + chans - 1] = y_cm as u8;
        balance += pulses[i] + tell;
        update_lowband = b > (n as i32) << BITRES;
    }

    *seed = ctx.seed;
}

#[allow(clippy::too_many_arguments)]
pub fn anti_collapse(
    mode: &Mode,
    x: &mut [f32],
    collapse_masks: &[u8],
    lm: i32,
    c: usize,
    size: usize,
    start: usize,
    end: usize,
    log_e: &[f32],
    prev1: &[f32],
    prev2: &[f32],
    pulses: &[i32],
    seed: &mut u32,
) {
    let nb = mode.nb_ebands;
    for i in start..end {
        let n0 = (mode.e_bands[i + 1] - mode.e_bands[i]) as usize;
        let depth = ((1 + pulses[i]) / (mode.e_bands[i + 1] - mode.e_bands[i])) >> lm;
        let thresh = 0.5 * (-0.125 * depth as f32).exp2();
        let sqrt_1 = 1.0 / ((n0 << lm) as f32).sqrt();

        for cc in 0..c {
            let mut prev1v = prev1[cc * nb + i];
            let mut prev2v = prev2[cc * nb + i];
            if c == 1 {
                prev1v = prev1v.max(prev1[nb + i]);
                prev2v = prev2v.max(prev2[nb + i]);
            }
            let ediff = (log_e[cc * nb + i] - prev1v.min(prev2v)).max(0.0);
            let mut r = 2.0 * (-ediff).exp2();
            if lm == 3 {
                r *= 1.41421356;
            }
            r = thresh.min(r) * sqrt_1;

            let base = cc * size + ((mode.e_bands[i] as usize) << lm);
            let mut renormalize = false;
            for k in 0..(1usize << lm) {
                if collapse_masks[i * c + cc] & (1 << k) == 0 {
                    for j in 0..n0 {
                        *seed = celt_lcg_rand(*seed);
                        x[base + (j << lm) + k] = if *seed & 0x8000 != 0 { r } else { -r };
                    }
                    renormalize = true;
                }
            }
            if renormalize {
                super::vq::renormalise_vector(&mut x[base..base + (n0 << lm)], n0 << lm, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denormalise_scales_by_energy() {
        let mode = Mode::new();
        let mut x = vec![0.0f32; 960];
        for v in x.iter_mut() {
            *v = 0.1;
        }
        let band_log_e = vec![0.0f32; mode.nb_ebands];
        let mut freq = vec![0.0f32; 960];
        denormalise_bands(&mode, &x, &mut freq, &band_log_e, 0, mode.nb_ebands, 8, false);
        assert!(freq.iter().all(|v| v.is_finite()));
        assert!(freq[0].abs() > 0.0);
        assert_eq!(freq[959], 0.0);
    }
}
