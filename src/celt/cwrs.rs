use crate::range::{ec_ilog, RangeDecoder};

pub fn log2_frac(val: u32, frac: i32) -> i32 {
    let mut l = ec_ilog(val);
    if val & (val - 1) != 0 {
        let mut v = if l > 16 {
            ((val - 1) >> (l - 16)) + 1
        } else {
            val << (16 - l)
        };
        l = (l - 1) << frac;
        let mut f = frac;
        loop {
            let b = (v >> 16) as i32;
            l += b << f;
            v = (v + b as u32) >> b;
            v = (v.wrapping_mul(v) + 0x7FFF) >> 15;
            let cont = f > 0;
            f -= 1;
            if !cont {
                break;
            }
        }
        l + (v > 0x8000) as i32
    } else {
        (l - 1) << frac
    }
}

pub fn get_pulses(i: i32) -> i32 {
    if i < 8 {
        i
    } else {
        (8 + (i & 7)) << ((i >> 3) - 1)
    }
}

pub fn fits_in32(n: i32, k: i32) -> bool {
    const MAXN: [i16; 15] = [
        32767, 32767, 32767, 1476, 283, 109, 60, 40, 29, 24, 20, 18, 16, 14, 13,
    ];
    const MAXK: [i16; 15] = [
        32767, 32767, 32767, 32767, 1172, 238, 95, 53, 36, 27, 22, 18, 16, 15, 13,
    ];
    if n >= 14 {
        if k >= 14 {
            false
        } else {
            n <= MAXN[k as usize] as i32
        }
    } else {
        k <= MAXK[n as usize] as i32
    }
}

fn unext(ui: &mut [u32], len: usize, mut ui0: u32) {
    let mut j = 1;
    loop {
        let ui1 = ui[j].wrapping_add(ui[j - 1]).wrapping_add(ui0);
        ui[j - 1] = ui0;
        ui0 = ui1;
        j += 1;
        if j >= len {
            break;
        }
    }
    ui[len - 1] = ui0;
}

fn uprev(ui: &mut [u32], len: usize, mut ui0: u32) {
    let mut j = 1;
    loop {
        let ui1 = ui[j].wrapping_sub(ui[j - 1]).wrapping_sub(ui0);
        ui[j - 1] = ui0;
        ui0 = ui1;
        j += 1;
        if j >= len {
            break;
        }
    }
    ui[len - 1] = ui0;
}

fn ncwrs_urow(n: usize, k: usize, u: &mut [u32]) -> u32 {
    let len = k + 2;
    u[0] = 0;
    u[1] = 1;
    let mut kk = 2;
    while kk < len {
        u[kk] = ((kk << 1) - 1) as u32;
        kk += 1;
    }
    for _ in 2..n {
        unext(&mut u[1..], k + 1, 1);
    }
    u[k] + u[k + 1]
}

#[cfg(test)]
pub fn v_size(n: usize, k: usize) -> u32 {
    if k == 0 {
        return 1;
    }
    if n == 1 {
        return 2;
    }
    let mut u = vec![0u32; k + 2];
    ncwrs_urow(n, k, &mut u)
}

fn cwrsi(n: usize, mut k: usize, mut i: u32, y: &mut [i32], u: &mut [u32]) -> f32 {
    let mut yy = 0.0f32;
    let mut j = 0;
    loop {
        let p = u[k + 1];
        let neg = i >= p;
        if neg {
            i -= p;
        }
        let yj = k;
        let mut p2 = u[k];
        while p2 > i {
            k -= 1;
            p2 = u[k];
        }
        i -= p2;
        let pulses = (yj - k) as i32;
        let val = if neg { -pulses } else { pulses };
        y[j] = val;
        yy += (val * val) as f32;
        uprev(u, k + 2, 0);
        j += 1;
        if j >= n {
            break;
        }
    }
    yy
}

pub fn decode_pulses(y: &mut [i32], n: usize, k: usize, dec: &mut RangeDecoder) -> f32 {
    let mut u = vec![0u32; k + 2];
    let nc = ncwrs_urow(n, k, &mut u);
    let idx = dec.dec_uint(nc);
    cwrsi(n, k, idx, y, &mut u)
}

pub fn get_required_bits(bits: &mut [i16], n: usize, maxk: usize, frac: i32) {
    bits[0] = 0;
    if n == 1 {
        for k in 1..=maxk {
            bits[k] = (1 << frac) as i16;
        }
    } else {
        let mut u = vec![0u32; maxk + 2];
        ncwrs_urow(n, maxk, &mut u);
        for k in 1..=maxk {
            bits[k] = log2_frac(u[k] + u[k + 1], frac) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v_size_recurrence() {
        assert_eq!(v_size(1, 0), 1);
        assert_eq!(v_size(2, 1), 4);
        assert_eq!(v_size(2, 2), 8);
        assert_eq!(v_size(3, 3), 38);
        assert_eq!(v_size(9, 9), 864146);
    }

    #[test]
    fn log2_frac_powers() {
        assert_eq!(log2_frac(1, 3), 0);
        assert_eq!(log2_frac(2, 3), 8);
        assert_eq!(log2_frac(4, 3), 16);
    }
}
