use super::tables::{BITRES, QTHETA_OFFSET, QTHETA_OFFSET_TWOPHASE};
use super::Mode;

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

struct BandCtx<'a> {
    m: &'a Mode,
    i: usize,
    intensity: usize,
    spread: i32,
    tf_change: i32,
    remaining_bits: i32,
    seed: u32,
}

struct SplitCtx {
    inv: bool,
    imid: i32,
    iside: i32,
    delta: i32,
    itheta: i32,
    qalloc: i32,
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
    let mid2 = mid;
    let el = mid2 * mid2 + side - 2.0 * xp;
    let er = mid2 * mid2 + side + 2.0 * xp;
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
    let mut tmp = vec![0.0f32; n];
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
    let mut tmp = vec![0.0f32; n];
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

pub fn haar1(x: &mut [f32], n0: usize, stride: usize) {
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

fn compute_theta(
    ctx: &mut BandCtx,
    sctx: &mut SplitCtx,
    n: usize,
    b: &mut i32,
    bparam: i32,
    b0: i32,
    lm: i32,
    stereo: bool,
    fill: &mut i32,
    rd: &mut crate::range::RangeDecoder,
) {
    let i = ctx.i;
    let pulse_cap = ctx.m.log_n[i] + lm * (1 << BITRES);
    let offset = (pulse_cap >> 1)
        - if stereo && n == 2 {
            QTHETA_OFFSET_TWOPHASE
        } else {
            QTHETA_OFFSET
        };
    let mut qn = compute_qn(n as i32, *b, offset, pulse_cap, stereo);
    if stereo && i >= ctx.intensity {
        qn = 1;
    }
    let tell = rd.tell_frac() as i32;
    let mut itheta: i32 = 0;
    let mut inv = false;

    if qn != 1 {
        let qh = qn >> 1;
        if stereo && n > 2 {
            let p0 = 3i32;
            let x0 = qn / 2;
            let ft = p0 * (x0 + 1) + x0;
            let fs = rd.decode(ft as u32) as i32;
            let xv;
            if fs < (x0 + 1) * p0 {
                xv = fs / p0;
            } else {
                xv = x0 + 1 + (fs - (x0 + 1) * p0);
            }
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
            rd.update(fl as u32, fh as u32, ft as u32);
            itheta = xv;
        } else if b0 > 1 || stereo {
            itheta = rd.dec_uint((qn + 1) as u32) as i32;
        } else {
            let ft = (qh + 1) * (qh + 1);
            let fm = rd.decode(ft as u32) as i32;
            let fl;
            let fs;
            if fm < (qh * (qh + 1) >> 1) {
                itheta = ((isqrt32(8 * fm as u32 + 1) as i32) - 1) >> 1;
                fs = itheta + 1;
                fl = itheta * (itheta + 1) >> 1;
            } else {
                itheta = (2 * (qn + 1) - isqrt32(8 * (ft - fm - 1) as u32 + 1) as i32) >> 1;
                fs = qn + 1 - itheta;
                fl = ft - ((qn + 1 - itheta) * (qn + 2 - itheta) >> 1);
            }
            rd.update(fl as u32, (fl + fs) as u32, ft as u32);
        }
        itheta = (itheta * 16384) / qn;
    } else if stereo {
        if *b > 2 << BITRES && ctx.remaining_bits > 2 << BITRES {
            inv = rd.dec_bit_logp(2) != 0;
        } else {
            inv = false;
        }
        itheta = 0;
    }
    let qalloc = rd.tell_frac() as i32 - tell;
    *b -= qalloc;

    let imid;
    let iside;
    let delta;
    if itheta == 0 {
        imid = 32767;
        iside = 0;
        *fill &= (1 << bparam) - 1;
        delta = -16384;
    } else if itheta == 16384 {
        imid = 0;
        iside = 32767;
        *fill &= ((1 << bparam) - 1) << bparam;
        delta = 16384;
    } else {
        imid = bitexact_cos(itheta);
        iside = bitexact_cos(16384 - itheta);
        delta = frac_mul16(((n as i32) - 1) << 7, bitexact_log2tan(iside, imid));
    }

    sctx.inv = inv;
    sctx.imid = imid;
    sctx.iside = iside;
    sctx.delta = delta;
    sctx.itheta = itheta;
    sctx.qalloc = qalloc;
}

fn quant_band_n1(
    ctx: &mut BandCtx,
    x: &mut [f32],
    y: Option<&mut [f32]>,
    lowband_out: Option<&mut [f32]>,
    rd: &mut crate::range::RangeDecoder,
) -> u32 {
    let stereo = y.is_some();
    let mut chans: [Option<&mut [f32]>; 2] = [Some(x), y];
    let count = if stereo { 2 } else { 1 };
    let mut x0_first = 0.0f32;
    for c in 0..count {
        let mut sign = 0u32;
        if ctx.remaining_bits >= 1 << BITRES {
            sign = rd.dec_bits(1);
            ctx.remaining_bits -= 1 << BITRES;
        }
        let v = if sign != 0 { -1.0 } else { 1.0 };
        if let Some(ch) = chans[c].as_mut() {
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

fn quant_partition(
    ctx: &mut BandCtx,
    x: &mut [f32],
    n: usize,
    b: i32,
    bparam: i32,
    lowband: Option<&[f32]>,
    lm: i32,
    gain: f32,
    fill: i32,
    rd: &mut crate::range::RangeDecoder,
) -> u32 {
    let mut n = n;
    let mut b = b;
    let mut bparam = bparam;
    let mut lm = lm;
    let mut fill = fill;
    let b0 = bparam;
    let mut cm: u32 = 0;

    let i = ctx.i;
    let cache_base = ctx.m.cache.index[((lm + 1) as usize) * ctx.m.nb_ebands + i] as usize;
    let cache = &ctx.m.cache.bits[cache_base..];
    if lm != -1 && b > cache[cache[0] as usize] as i32 + 12 && n > 2 {
        n >>= 1;
        let (xl, xr) = x.split_at_mut(n);
        lm -= 1;
        if bparam == 1 {
            fill = (fill & 1) | (fill << 1);
        }
        bparam = (bparam + 1) >> 1;

        let mut sctx = SplitCtx {
            inv: false,
            imid: 0,
            iside: 0,
            delta: 0,
            itheta: 0,
            qalloc: 0,
        };
        compute_theta(ctx, &mut sctx, n, &mut b, bparam, b0, lm, false, &mut fill, rd);
        let imid = sctx.imid;
        let iside = sctx.iside;
        let mut delta = sctx.delta;
        let itheta = sctx.itheta;
        let qalloc = sctx.qalloc;
        let mid = (1.0 / 32768.0) * imid as f32;
        let side = (1.0 / 32768.0) * iside as f32;

        if b0 > 1 && (itheta & 0x3fff) != 0 {
            if itheta > 8192 {
                delta -= delta >> (4 - lm);
            } else {
                delta = 0.min(delta + ((n as i32) << BITRES >> (5 - lm)));
            }
        }
        let mut mbits = 0.max(b.min((b - delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= qalloc;

        let next_lowband2: Option<&[f32]> = lowband.map(|lb| &lb[n..]);

        let mut rebalance = ctx.remaining_bits;
        if mbits >= sbits {
            cm = quant_partition(ctx, xl, n, mbits, bparam, lowband, lm, gain * mid, fill, rd);
            rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 0 {
                sbits += rebalance - (3 << BITRES);
            }
            cm |= quant_partition(
                ctx,
                xr,
                n,
                sbits,
                bparam,
                next_lowband2,
                lm,
                gain * side,
                fill >> bparam,
                rd,
            ) << (b0 >> 1);
        } else {
            cm = quant_partition(
                ctx,
                xr,
                n,
                sbits,
                bparam,
                next_lowband2,
                lm,
                gain * side,
                fill >> bparam,
                rd,
            ) << (b0 >> 1);
            rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 16384 {
                mbits += rebalance - (3 << BITRES);
            }
            cm |= quant_partition(ctx, xl, n, mbits, bparam, lowband, lm, gain * mid, fill, rd);
        }
    } else {
        let mut q = super::rate::bits2pulses(&ctx.m.cache, ctx.m.nb_ebands, i, lm, b);
        let mut curr_bits = super::rate::pulses2bits(&ctx.m.cache, ctx.m.nb_ebands, i, lm, q);
        ctx.remaining_bits -= curr_bits;
        while ctx.remaining_bits < 0 && q > 0 {
            ctx.remaining_bits += curr_bits;
            q -= 1;
            curr_bits = super::rate::pulses2bits(&ctx.m.cache, ctx.m.nb_ebands, i, lm, q);
            ctx.remaining_bits -= curr_bits;
        }

        if q != 0 {
            let k = super::cwrs::get_pulses(q);
            cm = super::vq::alg_unquant(x, n, k, ctx.spread, bparam as usize, rd, gain);
        } else {
            let cm_mask = ((1u32 << bparam) - 1) as u32;
            fill &= cm_mask as i32;
            if fill == 0 {
                for v in x[..n].iter_mut() {
                    *v = 0.0;
                }
            } else {
                match lowband {
                    None => {
                        for j in 0..n {
                            ctx.seed = celt_lcg_rand(ctx.seed);
                            x[j] = (ctx.seed as i32 >> 20) as f32;
                        }
                        cm = cm_mask;
                    }
                    Some(lb) => {
                        for j in 0..n {
                            ctx.seed = celt_lcg_rand(ctx.seed);
                            let tmp = if ctx.seed & 0x8000 != 0 {
                                1.0 / 256.0
                            } else {
                                -1.0 / 256.0
                            };
                            x[j] = lb[j] + tmp;
                        }
                        cm = fill as u32;
                    }
                }
                super::vq::renormalise_vector(x, n, gain);
            }
        }
    }
    cm
}

fn quant_band(
    ctx: &mut BandCtx,
    x: &mut [f32],
    n: usize,
    b: i32,
    bparam: i32,
    lowband: Option<&[f32]>,
    lm: i32,
    lowband_out: Option<&mut [f32]>,
    gain: f32,
    lowband_scratch: Option<&mut [f32]>,
    fill: i32,
    rd: &mut crate::range::RangeDecoder,
) -> u32 {
    let n0 = n;
    let mut n_b = n;
    let b0 = bparam;
    let mut bparam = bparam;
    let mut fill = fill;
    let mut time_divide = 0i32;
    let mut recombine = 0i32;
    let long_blocks = b0 == 1;
    let mut tf_change = ctx.tf_change;

    n_b /= bparam as usize;

    if n == 1 {
        return quant_band_n1(ctx, x, None, lowband_out, rd);
    }

    if tf_change > 0 {
        recombine = tf_change;
    }

    let mut lowband_buf: Option<&[f32]> = lowband;
    if let (Some(scratch), Some(lb)) = (lowband_scratch, lowband) {
        if recombine != 0 || ((n_b & 1) == 0 && tf_change < 0) || b0 > 1 {
            scratch[..n].copy_from_slice(&lb[..n]);
            lowband_buf = Some(&scratch[..n]);
        }
    }

    let mut lowband_owned: Option<Vec<f32>> = lowband_buf.map(|lb| lb[..n].to_vec());

    for k in 0..recombine {
        if let Some(lo) = lowband_owned.as_mut() {
            haar1(lo, n >> k, 1 << k);
        }
        fill = BIT_INTERLEAVE_TABLE[(fill & 0xF) as usize] as i32
            | (BIT_INTERLEAVE_TABLE[(fill >> 4) as usize] as i32) << 2;
    }
    bparam >>= recombine;
    n_b <<= recombine;

    while (n_b & 1) == 0 && tf_change < 0 {
        if let Some(lo) = lowband_owned.as_mut() {
            haar1(lo, n_b, bparam as usize);
        }
        fill |= fill << bparam;
        bparam <<= 1;
        n_b >>= 1;
        time_divide += 1;
        tf_change += 1;
    }
    let b0b = bparam;
    let n_b0 = n_b;

    if b0b > 1 {
        if let Some(lo) = lowband_owned.as_mut() {
            deinterleave_hadamard(
                lo,
                n_b >> recombine,
                (b0b << recombine) as usize,
                long_blocks,
            );
        }
    }

    let mut cm = quant_partition(
        ctx,
        x,
        n,
        b,
        b0b,
        lowband_owned.as_deref(),
        lm,
        gain,
        fill,
        rd,
    );

    if b0b > 1 {
        interleave_hadamard(x, n_b >> recombine, (b0b << recombine) as usize, long_blocks);
    }

    let mut n_b = n_b0;
    let mut bb = b0b;
    for _ in 0..time_divide {
        bb >>= 1;
        n_b <<= 1;
        cm |= cm >> bb;
        haar1(x, n_b, bb as usize);
    }

    for k in 0..recombine {
        cm = BIT_DEINTERLEAVE_TABLE[cm as usize] as u32;
        haar1(x, n0 >> k, 1 << k);
    }
    bb <<= recombine;

    if let Some(lo) = lowband_out {
        let nn = ((n0 as f32) * 4194304.0).sqrt();
        for j in 0..n0 {
            lo[j] = nn * x[j];
        }
    }
    cm &= (1 << bb) - 1;
    cm
}

fn quant_band_stereo(
    ctx: &mut BandCtx,
    x: &mut [f32],
    y: &mut [f32],
    n: usize,
    b: i32,
    bparam: i32,
    lowband: Option<&[f32]>,
    lm: i32,
    lowband_out: Option<&mut [f32]>,
    lowband_scratch: Option<&mut [f32]>,
    fill: i32,
    rd: &mut crate::range::RangeDecoder,
) -> u32 {
    let mut fill = fill;
    let mut cm: u32;

    if n == 1 {
        return quant_band_n1(ctx, x, Some(y), lowband_out, rd);
    }

    let orig_fill = fill;

    let mut b = b;
    let mut sctx = SplitCtx {
        inv: false,
        imid: 0,
        iside: 0,
        delta: 0,
        itheta: 0,
        qalloc: 0,
    };
    compute_theta(ctx, &mut sctx, n, &mut b, bparam, bparam, lm, true, &mut fill, rd);
    let inv = sctx.inv;
    let imid = sctx.imid;
    let iside = sctx.iside;
    let delta = sctx.delta;
    let itheta = sctx.itheta;
    let qalloc = sctx.qalloc;
    let mid = (1.0 / 32768.0) * imid as f32;
    let side = (1.0 / 32768.0) * iside as f32;

    if n == 2 {
        let mut sign = 0u32;
        let mut mbits = b;
        let mut sbits = 0;
        if itheta != 0 && itheta != 16384 {
            sbits = 1 << BITRES;
        }
        mbits -= sbits;
        let c = itheta > 8192;
        ctx.remaining_bits -= qalloc + sbits;

        if sbits != 0 {
            sign = rd.dec_bits(1);
        }
        let signf = 1.0 - 2.0 * sign as f32;

        {
            let (x2, y2): (&mut [f32], &mut [f32]) = if c { (y, x) } else { (x, y) };
            cm = quant_band(
                ctx,
                x2,
                n,
                mbits,
                bparam,
                lowband,
                lm,
                lowband_out,
                1.0,
                lowband_scratch,
                orig_fill,
                rd,
            );
            y2[0] = -signf * x2[1];
            y2[1] = signf * x2[0];
        }

        x[0] = mid * x[0];
        x[1] = mid * x[1];
        y[0] = side * y[0];
        y[1] = side * y[1];
        let tmp0 = x[0];
        x[0] = tmp0 - y[0];
        y[0] = tmp0 + y[0];
        let tmp1 = x[1];
        x[1] = tmp1 - y[1];
        y[1] = tmp1 + y[1];
    } else {
        let mut mbits = 0.max(b.min((b - delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= qalloc;

        let mut rebalance = ctx.remaining_bits;
        if mbits >= sbits {
            cm = quant_band(
                ctx,
                x,
                n,
                mbits,
                bparam,
                lowband,
                lm,
                lowband_out,
                1.0,
                lowband_scratch,
                fill,
                rd,
            );
            rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 0 {
                sbits += rebalance - (3 << BITRES);
            }
            cm |= quant_band(
                ctx,
                y,
                n,
                sbits,
                bparam,
                None,
                lm,
                None,
                side,
                None,
                fill >> bparam,
                rd,
            );
        } else {
            cm = quant_band(
                ctx,
                y,
                n,
                sbits,
                bparam,
                None,
                lm,
                None,
                side,
                None,
                fill >> bparam,
                rd,
            );
            rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 16384 {
                mbits += rebalance - (3 << BITRES);
            }
            cm |= quant_band(
                ctx,
                x,
                n,
                mbits,
                bparam,
                lowband,
                lm,
                lowband_out,
                1.0,
                lowband_scratch,
                fill,
                rd,
            );
        }
    }

    if n != 2 {
        stereo_merge(x, y, mid, n);
    }
    if inv {
        for j in 0..n {
            y[j] = -y[j];
        }
    }
    cm
}

fn special_hybrid_folding(
    m: &Mode,
    norm: &mut [f32],
    norm2: &mut [f32],
    start: usize,
    big_m: usize,
    dual_stereo: bool,
) {
    let eb = &m.e_bands;
    let n1 = big_m * (eb[start + 1] - eb[start]) as usize;
    let n2 = big_m * (eb[start + 2] - eb[start + 1]) as usize;
    let count = n2 - n1;
    for j in 0..count {
        norm[n1 + j] = norm[2 * n1 - n2 + j];
    }
    if dual_stereo {
        for j in 0..count {
            norm2[n1 + j] = norm2[2 * n1 - n2 + j];
        }
    }
}

pub fn quant_all_bands(
    mode: &crate::celt::Mode,
    start: usize,
    end: usize,
    x: &mut [f32],
    y: Option<&mut [f32]>,
    collapse_masks: &mut [u8],
    pulses: &[i32],
    short_blocks: i32,
    spread: i32,
    dual_stereo: bool,
    intensity: usize,
    tf_res: &[i32],
    total_bits: i32,
    balance: i32,
    rd: &mut crate::range::RangeDecoder,
    lm: i32,
    coded_bands: usize,
    seed: &mut u32,
) {
    let m = mode;
    let eb = &m.e_bands;
    let big_m = 1usize << lm;
    let bblocks = if short_blocks != 0 { big_m as i32 } else { 1 };
    let norm_offset = big_m * eb[start] as usize;
    let stereo = y.is_some();
    let chans = if stereo { 2 } else { 1 };

    let norm_len = big_m * eb[m.nb_ebands - 1] as usize - norm_offset;
    let mut norm_buf = vec![0.0f32; chans * norm_len];
    let mut scratch = vec![0.0f32; big_m * (eb[m.nb_ebands] as usize - eb[m.nb_ebands - 1] as usize)];

    let x_buf = x;
    let mut y_buf = y;

    let mut balance = balance;
    let mut dual_stereo = dual_stereo;
    let mut lowband_offset = 0usize;
    let mut update_lowband = true;

    let mut ctx = BandCtx {
        m,
        i: start,
        intensity,
        spread,
        tf_change: 0,
        remaining_bits: 0,
        seed: *seed,
    };

    for i in start..end {
        ctx.i = i;
        let last = i == end - 1;
        let n = big_m * eb[i + 1] as usize - big_m * eb[i] as usize;
        let bx = big_m * eb[i] as usize;
        let tell = rd.tell_frac() as i32;

        if i != start {
            balance -= tell;
        }
        let remaining_bits = total_bits - tell - 1;
        ctx.remaining_bits = remaining_bits;
        let b;
        if i <= coded_bands.wrapping_sub(1) && i + 1 <= coded_bands {
            let curr_balance = balance / (3.min(coded_bands - i) as i32);
            b = 0.max(16383.min((remaining_bits + 1).min(pulses[i] + curr_balance)));
        } else {
            b = 0;
        }

        if ((big_m * eb[i] as usize) >= norm_offset + n || i == start + 1)
            && (update_lowband || lowband_offset == 0)
        {
            lowband_offset = i;
        }
        if i == start + 1 {
            let (n1part, n2part) = norm_buf.split_at_mut(norm_len);
            special_hybrid_folding(m, n1part, n2part, start, big_m, dual_stereo);
        }

        let tf_change = tf_res[i];
        ctx.tf_change = tf_change;

        let use_norm_band = i >= m.nb_ebands;

        let mut x_cm: u32;
        let mut y_cm: u32;

        let effective_lowband: Option<usize>;
        if lowband_offset != 0 && (spread != SPREAD_AGGRESSIVE || bblocks > 1 || tf_change < 0) {
            let el = (big_m * eb[lowband_offset] as usize)
                .saturating_sub(norm_offset)
                .saturating_sub(n);
            let mut fold_start2 = lowband_offset;
            loop {
                fold_start2 -= 1;
                if !(big_m * eb[fold_start2] as usize > el + norm_offset) {
                    break;
                }
            }
            let mut fold_end = lowband_offset - 1;
            loop {
                fold_end += 1;
                if !(fold_end < i && (big_m * eb[fold_end] as usize) < el + norm_offset + n) {
                    break;
                }
            }
            x_cm = 0;
            y_cm = 0;
            let mut fi = fold_start2;
            loop {
                x_cm |= collapse_masks[fi * chans] as u32;
                y_cm |= collapse_masks[fi * chans + chans - 1] as u32;
                fi += 1;
                if fi >= fold_end {
                    break;
                }
            }
            effective_lowband = Some(el);
        } else {
            x_cm = (1u32 << bblocks) - 1;
            y_cm = (1u32 << bblocks) - 1;
            effective_lowband = None;
        }

        if dual_stereo && i == intensity {
            dual_stereo = false;
            let limit = big_m * eb[i] as usize - norm_offset;
            for j in 0..limit {
                norm_buf[j] = 0.5 * (norm_buf[j] + norm_buf[norm_len + j]);
            }
        }

        let out_index = big_m * eb[i] as usize - norm_offset;

        if dual_stereo {
            {
                let xc = &mut x_buf[bx..bx + n];
                x_cm = quant_band_mono(
                    &mut ctx,
                    xc,
                    n,
                    b / 2,
                    bblocks,
                    effective_lowband,
                    &mut norm_buf,
                    0,
                    lm,
                    out_index,
                    last,
                    &mut scratch,
                    x_cm,
                    rd,
                    use_norm_band,
                );
            }
            {
                let yref = y_buf.as_deref_mut().unwrap();
                let yc = &mut yref[bx..bx + n];
                y_cm = quant_band_mono(
                    &mut ctx,
                    yc,
                    n,
                    b / 2,
                    bblocks,
                    effective_lowband,
                    &mut norm_buf,
                    norm_len,
                    lm,
                    out_index,
                    last,
                    &mut scratch,
                    y_cm,
                    rd,
                    use_norm_band,
                );
            }
        } else if stereo {
            let yref = y_buf.as_deref_mut().unwrap();
            let lowband_vec: Option<Vec<f32>> =
                effective_lowband.map(|el| norm_buf[el..el + n].to_vec());
            let scratch_opt: Option<&mut [f32]> = if last || use_norm_band {
                None
            } else {
                Some(&mut scratch[..])
            };
            if use_norm_band {
                let nb = norm_buf[..n].to_vec();
                let mut xb = nb.clone();
                let mut yb = nb.clone();
                x_cm = quant_band_stereo(
                    &mut ctx,
                    &mut xb,
                    &mut yb,
                    n,
                    b,
                    bblocks,
                    lowband_vec.as_deref(),
                    lm,
                    None,
                    None,
                    (x_cm | y_cm) as i32,
                    rd,
                );
            } else {
                let xc = &mut x_buf[bx..bx + n];
                let yc = &mut yref[bx..bx + n];
                let mut out_tmp = vec![0.0f32; n];
                x_cm = quant_band_stereo(
                    &mut ctx,
                    xc,
                    yc,
                    n,
                    b,
                    bblocks,
                    lowband_vec.as_deref(),
                    lm,
                    if last { None } else { Some(&mut out_tmp) },
                    scratch_opt,
                    (x_cm | y_cm) as i32,
                    rd,
                );
                if !last {
                    norm_buf[out_index..out_index + n].copy_from_slice(&out_tmp);
                }
            }
            y_cm = x_cm;
        } else {
            let xc = &mut x_buf[bx..bx + n];
            x_cm = quant_band_mono(
                &mut ctx,
                xc,
                n,
                b,
                bblocks,
                effective_lowband,
                &mut norm_buf,
                0,
                lm,
                out_index,
                last,
                &mut scratch,
                x_cm | y_cm,
                rd,
                use_norm_band,
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

fn quant_band_mono(
    ctx: &mut BandCtx,
    x: &mut [f32],
    n: usize,
    b: i32,
    bblocks: i32,
    effective_lowband: Option<usize>,
    norm_buf: &mut [f32],
    norm_chan_off: usize,
    lm: i32,
    out_index: usize,
    last: bool,
    scratch: &mut [f32],
    fill: u32,
    rd: &mut crate::range::RangeDecoder,
    use_norm_band: bool,
) -> u32 {
    let lowband_vec: Option<Vec<f32>> =
        effective_lowband.map(|el| norm_buf[norm_chan_off + el..norm_chan_off + el + n].to_vec());
    let mut out_tmp = vec![0.0f32; n];
    let scratch_opt: Option<&mut [f32]> = if last || use_norm_band {
        None
    } else {
        Some(scratch)
    };
    let cm = quant_band(
        ctx,
        x,
        n,
        b,
        bblocks,
        lowband_vec.as_deref(),
        lm,
        if last { None } else { Some(&mut out_tmp) },
        1.0,
        scratch_opt,
        fill as i32,
        rd,
    );
    if !last {
        norm_buf[norm_chan_off + out_index..norm_chan_off + out_index + n].copy_from_slice(&out_tmp);
    }
    cm
}

pub fn anti_collapse(
    mode: &crate::celt::Mode,
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
    let m = mode;
    let nb = m.nb_ebands;
    for i in start..end {
        let n0 = (m.e_bands[i + 1] - m.e_bands[i]) as usize;
        let depth = ((1 + pulses[i]) / (m.e_bands[i + 1] - m.e_bands[i])) >> lm;
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
            r = thresh.min(r);
            r *= sqrt_1;

            let base = cc * size + ((m.e_bands[i] as usize) << lm);
            let mut renormalize = false;
            for k in 0..(1usize << lm) {
                if collapse_masks[i * c + cc] & (1 << k) == 0 {
                    for j in 0..n0 {
                        *seed = celt_lcg_rand(*seed);
                        let v = if *seed & 0x8000 != 0 { r } else { -r };
                        x[base + (j << lm) + k] = v;
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
        for i in 0..960 {
            x[i] = 0.1;
        }
        let band_log_e = vec![0.0f32; mode.nb_ebands];
        let mut freq = vec![0.0f32; 960];
        denormalise_bands(&mode, &x, &mut freq, &band_log_e, 0, mode.nb_ebands, 8, false);
        assert!(freq.iter().all(|v| v.is_finite()));
        assert!(freq[0].abs() > 0.0);
        assert_eq!(freq[959], 0.0);
    }
}
