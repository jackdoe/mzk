use super::cwrs::decode_pulses;
use crate::range::RangeDecoder;

const EPSILON: f32 = 1e-15;
const SPREAD_FACTOR: [i32; 3] = [15, 10, 5];

fn exp_rotation1(x: &mut [f32], len: usize, stride: usize, c: f32, s: f32) {
    let ms = -s;
    for i in 0..len - stride {
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 + ms * x2;
    }
    let start = len as isize - 2 * stride as isize - 1;
    let mut idx = start;
    while idx >= 0 {
        let i = idx as usize;
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 + ms * x2;
        idx -= 1;
    }
}

fn exp_rotation(x: &mut [f32], len: usize, dir: i32, stride: usize, k: i32, spread: i32) {
    if 2 * k >= len as i32 || spread == 0 {
        return;
    }
    let factor = SPREAD_FACTOR[(spread - 1) as usize];
    let gain = len as f32 / (len as i32 + factor * k) as f32;
    let theta = 0.5 * gain * gain;
    let half_pi = 0.5 * std::f32::consts::PI;
    let c = (half_pi * theta).cos();
    let s = (half_pi * (1.0 - theta)).cos();
    let mut stride2 = 0usize;
    if len >= 8 * stride {
        stride2 = 1;
        while (stride2 * stride2 + stride2) * stride + (stride >> 2) < len {
            stride2 += 1;
        }
    }
    let len2 = len / stride;
    for i in 0..stride {
        let seg = &mut x[i * len2..i * len2 + len2];
        if dir < 0 {
            if stride2 != 0 {
                exp_rotation1(seg, len2, stride2, s, c);
            }
            exp_rotation1(seg, len2, 1, c, s);
        } else {
            exp_rotation1(seg, len2, 1, c, -s);
            if stride2 != 0 {
                exp_rotation1(seg, len2, stride2, s, -c);
            }
        }
    }
}

fn normalise_residual(iy: &[i32], x: &mut [f32], n: usize, ryy: f32, gain: f32) {
    let g = gain / ryy.sqrt();
    for i in 0..n {
        x[i] = g * iy[i] as f32;
    }
}

fn extract_collapse_mask(iy: &[i32], n: usize, b: usize) -> u32 {
    if b <= 1 {
        return 1;
    }
    let n0 = n / b;
    let mut mask = 0u32;
    for i in 0..b {
        let mut tmp = 0i32;
        for j in 0..n0 {
            tmp |= iy[i * n0 + j];
        }
        mask |= ((tmp != 0) as u32) << i;
    }
    mask
}

pub fn alg_unquant(
    x: &mut [f32],
    n: usize,
    k: i32,
    spread: i32,
    b: usize,
    dec: &mut RangeDecoder,
    gain: f32,
) -> u32 {
    let mut iy = vec![0i32; n];
    let ryy = decode_pulses(&mut iy, n, k as usize, dec);
    normalise_residual(&iy, x, n, ryy, gain);
    exp_rotation(x, n, -1, b, k, spread);
    extract_collapse_mask(&iy, n, b)
}

pub fn renormalise_vector(x: &mut [f32], n: usize, gain: f32) {
    let mut e = EPSILON;
    for i in 0..n {
        e += x[i] * x[i];
    }
    let g = gain / e.sqrt();
    for i in 0..n {
        x[i] *= g;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_shape_is_unit_norm() {
        let buf = [0x9Au8; 128];
        let mut rd = RangeDecoder::new(&buf);
        let mut x = [0.0f32; 16];
        let mask = alg_unquant(&mut x, 16, 5, 2, 1, &mut rd, 1.0);
        let e: f32 = x.iter().map(|v| v * v).sum();
        assert!((e - 1.0).abs() < 1e-3);
        assert_eq!(mask, 1);
    }
}
