use super::cwrs::{fits_in32, get_pulses, get_required_bits};
use super::tables::*;

pub struct PulseCache {
    pub index: Vec<i16>,
    pub bits: Vec<u8>,
    pub caps: Vec<u8>,
}

pub fn compute_pulse_cache(e_bands: &[i32], nb: usize, log_n: &[i32], lm: usize) -> PulseCache {
    let mut cindex = vec![0i16; nb * (lm + 2)];
    let mut entry_n: Vec<i32> = Vec::new();
    let mut entry_k: Vec<i32> = Vec::new();
    let mut entry_i: Vec<i32> = Vec::new();
    let mut curr = 0i32;

    for i in 0..=lm + 1 {
        for j in 0..nb {
            let n = (e_bands[j + 1] - e_bands[j]) << i >> 1;
            cindex[i * nb + j] = -1;
            'find: for k in 0..=i {
                let mut nn = 0;
                while nn < nb && (k != i || nn < j) {
                    if n == (e_bands[nn + 1] - e_bands[nn]) << k >> 1 {
                        cindex[i * nb + j] = cindex[k * nb + nn];
                        break 'find;
                    }
                    nn += 1;
                }
            }
            if cindex[i * nb + j] == -1 && n != 0 {
                entry_n.push(n);
                let mut kk = 0i32;
                while fits_in32(n, get_pulses(kk + 1)) && kk < MAX_PSEUDO {
                    kk += 1;
                }
                entry_k.push(kk);
                cindex[i * nb + j] = curr as i16;
                entry_i.push(curr);
                curr += kk + 1;
            }
        }
    }

    let mut bits = vec![0u8; curr as usize];
    for e in 0..entry_n.len() {
        let base = entry_i[e] as usize;
        let mut tmp = vec![0i16; (CELT_MAX_PULSES + 1) as usize];
        get_required_bits(
            &mut tmp,
            entry_n[e] as usize,
            get_pulses(entry_k[e]) as usize,
            BITRES,
        );
        for j in 1..=entry_k[e] as usize {
            bits[base + j] = (tmp[get_pulses(j as i32) as usize] - 1) as u8;
        }
        bits[base] = entry_k[e] as u8;
    }

    let mut caps = vec![0u8; (lm + 1) * 2 * nb];
    let mut cap_idx = 0usize;
    for i in 0..=lm as i32 {
        for c in 1..=2i32 {
            for j in 0..nb {
                let mut n0 = e_bands[j + 1] - e_bands[j];
                let mut max_bits = c * (1 + MAX_FINE_BITS) << BITRES;
                if n0 << i != 1 {
                    let mut lm0 = 0i32;
                    if n0 > 2 {
                        n0 >>= 1;
                        lm0 -= 1;
                    } else if n0 <= 1 {
                        lm0 = i.min(1);
                        n0 <<= lm0;
                    }
                    let pbase = cindex[((lm0 + 1) as usize) * nb + j] as usize;
                    max_bits = bits[pbase + bits[pbase] as usize] as i32 + 1;
                    let mut nn = n0;
                    for k in 0..i - lm0 {
                        max_bits <<= 1;
                        let offset = ((log_n[j] + ((lm0 + k) << BITRES)) >> 1) - QTHETA_OFFSET;
                        let num = 459 * ((2 * nn - 1) * offset + max_bits);
                        let den = ((2 * nn - 1) << 9) - 459;
                        max_bits += ((num + (den >> 1)) / den).min(57);
                        nn <<= 1;
                    }
                    if c == 2 {
                        max_bits <<= 1;
                        let offset = ((log_n[j] + (i << BITRES)) >> 1)
                            - if nn == 2 { QTHETA_OFFSET_TWOPHASE } else { QTHETA_OFFSET };
                        let ndof = 2 * nn - 1 - (nn == 2) as i32;
                        let num = (if nn == 2 { 512 } else { 487 }) * (max_bits + ndof * offset);
                        let den = (ndof << 9) - if nn == 2 { 512 } else { 487 };
                        max_bits += ((num + (den >> 1)) / den).min(if nn == 2 { 64 } else { 61 });
                    }
                    let ndof = c * nn + if c == 2 && nn > 2 { 1 } else { 0 };
                    let mut offset = ((log_n[j] + (i << BITRES)) >> 1) - FINE_OFFSET;
                    if nn == 2 {
                        offset += 1 << BITRES >> 2;
                    }
                    let num = max_bits + ndof * offset;
                    let den = (ndof - 1) << BITRES;
                    let qb = ((num + (den >> 1)) / den).min(MAX_FINE_BITS);
                    max_bits += c * qb << BITRES;
                }
                let final_bits = (4 * max_bits / (c * ((e_bands[j + 1] - e_bands[j]) << i))) - 64;
                caps[cap_idx] = final_bits as u8;
                cap_idx += 1;
            }
        }
    }

    PulseCache {
        index: cindex,
        bits,
        caps,
    }
}

pub fn bits2pulses(cache: &PulseCache, nb: usize, band: usize, lm: i32, mut bits: i32) -> i32 {
    let lm = (lm + 1) as usize;
    let base = cache.index[lm * nb + band] as usize;
    let c = &cache.bits[base..];
    let mut lo = 0i32;
    let mut hi = c[0] as i32;
    bits -= 1;
    for _ in 0..LOG_MAX_PSEUDO {
        let mid = (lo + hi + 1) >> 1;
        if c[mid as usize] as i32 >= bits {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let lo_cost = if lo == 0 { -1 } else { c[lo as usize] as i32 };
    if bits - lo_cost <= c[hi as usize] as i32 - bits {
        lo
    } else {
        hi
    }
}

pub fn pulses2bits(cache: &PulseCache, nb: usize, band: usize, lm: i32, pulses: i32) -> i32 {
    let lm = (lm + 1) as usize;
    let base = cache.index[lm * nb + band] as usize;
    if pulses == 0 {
        0
    } else {
        cache.bits[base + pulses as usize] as i32 + 1
    }
}
