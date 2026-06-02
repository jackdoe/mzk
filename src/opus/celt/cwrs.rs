use crate::opus::range::{ec_ilog, RangeDecoder};

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

fn row_extend(row: &mut [u32], len: usize, mut carry: u32) {
    for j in 1..len {
        let next = row[j].wrapping_add(row[j - 1]).wrapping_add(carry);
        row[j - 1] = carry;
        carry = next;
    }
    row[len - 1] = carry;
}

fn row_reduce(row: &mut [u32], len: usize, mut carry: u32) {
    for j in 1..len {
        let next = row[j].wrapping_sub(row[j - 1]).wrapping_sub(carry);
        row[j - 1] = carry;
        carry = next;
    }
    row[len - 1] = carry;
}

fn pulse_row(n: usize, k: usize, row: &mut [u32]) -> u32 {
    let len = k + 2;
    row[0] = 0;
    row[1] = 1;
    for kk in 2..len {
        row[kk] = ((kk << 1) - 1) as u32;
    }
    for _ in 2..n {
        row_extend(&mut row[1..], k + 1, 1);
    }
    row[k] + row[k + 1]
}

#[cfg(test)]
pub fn v_size(n: usize, k: usize) -> u32 {
    if k == 0 {
        return 1;
    }
    if n == 1 {
        return 2;
    }
    let mut row = vec![0u32; k + 2];
    pulse_row(n, k, &mut row)
}

fn decode_codeword(n: usize, mut k: usize, mut index: u32, y: &mut [i32], row: &mut [u32]) -> f32 {
    let mut energy = 0.0f32;
    for slot in y.iter_mut().take(n) {
        let sign_split = row[k + 1];
        let negative = index >= sign_split;
        if negative {
            index -= sign_split;
        }
        let pulses_before = k;
        while row[k] > index {
            k -= 1;
        }
        index -= row[k];
        let pulses = (pulses_before - k) as i32;
        let value = if negative { -pulses } else { pulses };
        *slot = value;
        energy += (value * value) as f32;
        row_reduce(row, k + 2, 0);
    }
    energy
}

pub fn decode_pulses(y: &mut [i32], n: usize, k: usize, dec: &mut RangeDecoder) -> f32 {
    let mut row = vec![0u32; k + 2];
    let codebook_size = pulse_row(n, k, &mut row);
    let index = dec.dec_uint(codebook_size);
    decode_codeword(n, k, index, y, &mut row)
}

pub fn get_required_bits(bits: &mut [i16], n: usize, maxk: usize, frac: i32) {
    bits[0] = 0;
    if n == 1 {
        for k in 1..=maxk {
            bits[k] = (1 << frac) as i16;
        }
    } else {
        let mut row = vec![0u32; maxk + 2];
        pulse_row(n, maxk, &mut row);
        for k in 1..=maxk {
            bits[k] = log2_frac(row[k] + row[k + 1], frac) as i16;
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
