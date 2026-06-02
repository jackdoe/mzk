use super::tables::{BITRES, MAX_FINE_BITS};
use super::Mode;
use crate::opus::range::RangeDecoder;

const ALLOC_STEPS: i32 = 6;

const LOG2_FRAC_TABLE: [i32; 24] = [
    0, 8, 13, 16, 19, 21, 23, 24, 26, 27, 28, 29, 30, 31, 32, 32, 33, 34, 34, 35, 36, 36, 37, 37,
];

pub struct Alloc {
    pub pulses: Vec<i32>,
    pub fine_bits: Vec<i32>,
    pub fine_priority: Vec<i32>,
    pub intensity: usize,
    pub dual_stereo: bool,
    pub coded_bands: usize,
    pub balance: i32,
}

pub fn init_caps(mode: &Mode, lm: usize, c: usize) -> Vec<i32> {
    let nb = mode.nb_ebands;
    (0..nb)
        .map(|i| {
            let n = (mode.e_bands[i + 1] - mode.e_bands[i]) << lm;
            (mode.cache.caps[nb * (2 * lm + c - 1) + i] as i32 + 64) * c as i32 * n >> 2
        })
        .collect()
}

fn interp_bits2pulses(
    mode: &Mode,
    start: usize,
    end: usize,
    skip_start: usize,
    bits1: &[i32],
    bits2: &[i32],
    thresh: &[i32],
    cap: &[i32],
    mut total: i32,
    skip_rsv: i32,
    mut intensity_rsv: i32,
    dual_stereo_rsv: i32,
    c: i32,
    lm: i32,
    rd: &mut RangeDecoder,
    bits: &mut [i32],
    ebits: &mut [i32],
    fine_priority: &mut [i32],
) -> (usize, i32, usize, bool) {
    let eb = &mode.e_bands;
    let alloc_floor = c << BITRES;
    let stereo = (c > 1) as i32;
    let log_m = lm << BITRES;

    let mut lo = 0i32;
    let mut hi = 1 << ALLOC_STEPS;
    for _ in 0..ALLOC_STEPS {
        let mid = (lo + hi) >> 1;
        let mut psum = 0i32;
        let mut done = false;
        let mut j = end;
        while j > start {
            j -= 1;
            let tmp = bits1[j] + (mid * bits2[j] >> ALLOC_STEPS);
            if tmp >= thresh[j] || done {
                done = true;
                psum += tmp.min(cap[j]);
            } else if tmp >= alloc_floor {
                psum += alloc_floor;
            }
        }
        if psum > total {
            hi = mid;
        } else {
            lo = mid;
        }
    }

    let mut psum = 0i32;
    let mut done = false;
    let mut j = end;
    while j > start {
        j -= 1;
        let mut tmp = bits1[j] + (lo * bits2[j] >> ALLOC_STEPS);
        if tmp < thresh[j] && !done {
            tmp = if tmp >= alloc_floor { alloc_floor } else { 0 };
        } else {
            done = true;
        }
        tmp = tmp.min(cap[j]);
        bits[j] = tmp;
        psum += tmp;
    }

    let mut coded_bands = end;
    loop {
        let jj = coded_bands - 1;
        if jj <= skip_start {
            total += skip_rsv;
            break;
        }
        let mut left = total - psum;
        let percoeff = left / (eb[coded_bands] - eb[start]);
        left -= (eb[coded_bands] - eb[start]) * percoeff;
        let rem = (left - (eb[jj] - eb[start])).max(0);
        let band_width = eb[coded_bands] - eb[jj];
        let mut band_bits = bits[jj] + percoeff * band_width + rem;
        if band_bits >= thresh[jj].max(alloc_floor + (1 << BITRES)) {
            if rd.dec_bit_logp(1) == 1 {
                break;
            }
            psum += 1 << BITRES;
            band_bits -= 1 << BITRES;
        }
        psum -= bits[jj] + intensity_rsv;
        if intensity_rsv > 0 {
            intensity_rsv = LOG2_FRAC_TABLE[jj - start];
        }
        psum += intensity_rsv;
        if band_bits >= alloc_floor {
            psum += alloc_floor;
            bits[jj] = alloc_floor;
        } else {
            bits[jj] = 0;
        }
        coded_bands -= 1;
    }

    let mut intensity = 0usize;
    let mut dual_stereo = false;
    if intensity_rsv > 0 {
        intensity = start + rd.dec_uint((coded_bands + 1 - start) as u32) as usize;
    }
    let mut dsr = dual_stereo_rsv;
    if intensity <= start {
        total += dsr;
        dsr = 0;
    }
    if dsr > 0 {
        dual_stereo = rd.dec_bit_logp(1) == 1;
    }

    let mut left = total - psum;
    let percoeff = left / (eb[coded_bands] - eb[start]);
    left -= (eb[coded_bands] - eb[start]) * percoeff;
    for j in start..coded_bands {
        bits[j] += percoeff * (eb[j + 1] - eb[j]);
    }
    for j in start..coded_bands {
        let tmp = left.min(eb[j + 1] - eb[j]);
        bits[j] += tmp;
        left -= tmp;
    }

    let mut balance = 0i32;
    for j in start..coded_bands {
        let n0 = eb[j + 1] - eb[j];
        let n = n0 << lm;
        let bit = bits[j] + balance;
        let mut excess;
        if n > 1 {
            excess = (bit - cap[j]).max(0);
            bits[j] = bit - excess;
            let den = c * n
                + if c == 2 && n > 2 && !dual_stereo && (j as usize) < intensity {
                    1
                } else {
                    0
                };
            let nclogn = den * (mode.log_n[j] + log_m);
            let mut offset = (nclogn >> 1) - den * super::tables::FINE_OFFSET;
            if n == 2 {
                offset += den << BITRES >> 2;
            }
            if bits[j] + offset < den * 2 << BITRES {
                offset += nclogn >> 2;
            } else if bits[j] + offset < den * 3 << BITRES {
                offset += nclogn >> 3;
            }
            ebits[j] = (bits[j] + offset + (den << (BITRES - 1))).max(0);
            ebits[j] = ebits[j] / den >> BITRES;
            if c * ebits[j] > (bits[j] >> BITRES) {
                ebits[j] = bits[j] >> stereo >> BITRES;
            }
            ebits[j] = ebits[j].min(MAX_FINE_BITS);
            fine_priority[j] = (ebits[j] * (den << BITRES) >= bits[j] + offset) as i32;
            bits[j] -= c * ebits[j] << BITRES;
        } else {
            excess = (bit - (c << BITRES)).max(0);
            bits[j] = bit - excess;
            ebits[j] = 0;
            fine_priority[j] = 1;
        }
        if excess > 0 {
            let extra_fine = (excess >> (stereo + BITRES)).min(MAX_FINE_BITS - ebits[j]);
            ebits[j] += extra_fine;
            let extra_bits = extra_fine * c << BITRES;
            fine_priority[j] = (extra_bits >= excess - balance) as i32;
            excess -= extra_bits;
        }
        balance = excess;
    }

    for j in coded_bands..end {
        ebits[j] = bits[j] >> stereo >> BITRES;
        bits[j] = 0;
        fine_priority[j] = (ebits[j] < 1) as i32;
    }

    (coded_bands, balance, intensity, dual_stereo)
}

pub fn clt_compute_allocation(
    mode: &Mode,
    start: usize,
    end: usize,
    offsets: &[i32],
    cap: &[i32],
    alloc_trim: i32,
    mut total: i32,
    c: i32,
    lm: i32,
    rd: &mut RangeDecoder,
) -> Alloc {
    let len = mode.nb_ebands;
    let eb = &mode.e_bands;
    total = total.max(0);
    let mut skip_start = start;
    let skip_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
    total -= skip_rsv;
    let mut intensity_rsv = 0i32;
    let mut dual_stereo_rsv = 0i32;
    if c == 2 {
        intensity_rsv = LOG2_FRAC_TABLE[end - start];
        if intensity_rsv > total {
            intensity_rsv = 0;
        } else {
            total -= intensity_rsv;
            dual_stereo_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
            total -= dual_stereo_rsv;
        }
    }

    let mut thresh = vec![0i32; len];
    let mut trim_offset = vec![0i32; len];
    for j in start..end {
        thresh[j] = (c << BITRES).max((3 * (eb[j + 1] - eb[j]) << lm << BITRES) >> 4);
        trim_offset[j] = c * (eb[j + 1] - eb[j]) * (alloc_trim - 5 - lm) * (end as i32 - j as i32 - 1)
            * (1 << (lm + BITRES))
            >> 6;
        if (eb[j + 1] - eb[j]) << lm == 1 {
            trim_offset[j] -= c << BITRES;
        }
    }

    let nb_alloc = super::tables::BITALLOC_SIZE as i32;
    let mut lo = 1i32;
    let mut hi = nb_alloc - 1;
    loop {
        let mut done = false;
        let mut psum = 0i32;
        let mid = (lo + hi) >> 1;
        let mut j = end;
        while j > start {
            j -= 1;
            let n = eb[j + 1] - eb[j];
            let mut bitsj = c * n * (mode.alloc_vectors[mid as usize * len + j] as i32) << lm >> 2;
            if bitsj > 0 {
                bitsj = (bitsj + trim_offset[j]).max(0);
            }
            bitsj += offsets[j];
            if bitsj >= thresh[j] || done {
                done = true;
                psum += bitsj.min(cap[j]);
            } else if bitsj >= c << BITRES {
                psum += c << BITRES;
            }
        }
        if psum > total {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
        if lo > hi {
            break;
        }
    }
    hi = lo;
    lo -= 1;

    let mut bits1 = vec![0i32; len];
    let mut bits2 = vec![0i32; len];
    for j in start..end {
        let n = eb[j + 1] - eb[j];
        let mut b1 = c * n * (mode.alloc_vectors[lo as usize * len + j] as i32) << lm >> 2;
        let mut b2 = if hi >= nb_alloc {
            cap[j]
        } else {
            c * n * (mode.alloc_vectors[hi as usize * len + j] as i32) << lm >> 2
        };
        if b1 > 0 {
            b1 = (b1 + trim_offset[j]).max(0);
        }
        if b2 > 0 {
            b2 = (b2 + trim_offset[j]).max(0);
        }
        if lo > 0 {
            b1 += offsets[j];
        }
        b2 += offsets[j];
        if offsets[j] > 0 {
            skip_start = j;
        }
        b2 = (b2 - b1).max(0);
        bits1[j] = b1;
        bits2[j] = b2;
    }

    let mut pulses = vec![0i32; len];
    let mut ebits = vec![0i32; len];
    let mut fine_priority = vec![0i32; len];
    let (coded_bands, balance, intensity, dual_stereo) = interp_bits2pulses(
        mode,
        start,
        end,
        skip_start,
        &bits1,
        &bits2,
        &thresh,
        cap,
        total,
        skip_rsv,
        intensity_rsv,
        dual_stereo_rsv,
        c,
        lm,
        rd,
        &mut pulses,
        &mut ebits,
        &mut fine_priority,
    );

    Alloc {
        pulses,
        fine_bits: ebits,
        fine_priority,
        intensity,
        dual_stereo,
        coded_bands,
        balance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_runs() {
        let mode = Mode::new();
        let buf = [0x55u8; 400];
        let mut rd = RangeDecoder::new(&buf);
        let cap = init_caps(&mode, 3, 2);
        let offsets = vec![0i32; mode.nb_ebands];
        let a = clt_compute_allocation(&mode, 0, mode.nb_ebands, &offsets, &cap, 5, 200 * 8 * 8, 2, 3, &mut rd);
        assert_eq!(a.pulses.len(), mode.nb_ebands);
        assert!(a.intensity <= mode.nb_ebands);
        assert!(a.coded_bands <= mode.nb_ebands);
    }
}
