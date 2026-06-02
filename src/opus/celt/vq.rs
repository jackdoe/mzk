use super::cwrs::decode_pulses;
use crate::opus::range::RangeDecoder;

const EPSILON: f32 = 1e-15;
const SPREAD_FACTOR: [i32; 3] = [15, 10, 5];

fn rotate_pass(x: &mut [f32], len: usize, stride: usize, c: f32, s: f32) {
    let ms = -s;
    for i in 0..len - stride {
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 + ms * x2;
    }
    let mut idx = len as isize - 2 * stride as isize - 1;
    while idx >= 0 {
        let i = idx as usize;
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 + ms * x2;
        idx -= 1;
    }
}

fn undo_spreading(x: &mut [f32], len: usize, stride: usize, k: i32, spread: i32) {
    if 2 * k >= len as i32 || spread == 0 {
        return;
    }
    let factor = SPREAD_FACTOR[(spread - 1) as usize];
    let gain = len as f32 / (len as i32 + factor * k) as f32;
    let theta = 0.5 * gain * gain;
    let half_pi = 0.5 * std::f32::consts::PI;
    let c = (half_pi * theta).cos();
    let s = (half_pi * (1.0 - theta)).cos();

    let mut interleave = 0usize;
    if len >= 8 * stride {
        interleave = 1;
        while (interleave * interleave + interleave) * stride + (stride >> 2) < len {
            interleave += 1;
        }
    }
    let seg_len = len / stride;
    for i in 0..stride {
        let seg = &mut x[i * seg_len..i * seg_len + seg_len];
        if interleave != 0 {
            rotate_pass(seg, seg_len, interleave, s, c);
        }
        rotate_pass(seg, seg_len, 1, c, s);
    }
}

fn scale_to_unit_norm(iy: &[i32], x: &mut [f32], n: usize, energy: f32, gain: f32) {
    let g = gain / energy.sqrt();
    for i in 0..n {
        x[i] = g * iy[i] as f32;
    }
}

fn collapse_mask(iy: &[i32], n: usize, blocks: usize) -> u32 {
    if blocks <= 1 {
        return 1;
    }
    let per_block = n / blocks;
    let mut mask = 0u32;
    for b in 0..blocks {
        let mut any = 0i32;
        for j in 0..per_block {
            any |= iy[b * per_block + j];
        }
        mask |= ((any != 0) as u32) << b;
    }
    mask
}

pub fn alg_unquant(
    x: &mut [f32],
    n: usize,
    k: i32,
    spread: i32,
    blocks: usize,
    dec: &mut RangeDecoder,
    gain: f32,
) -> u32 {
    let mut iy = vec![0i32; n];
    let energy = decode_pulses(&mut iy, n, k as usize, dec);
    scale_to_unit_norm(&iy, x, n, energy, gain);
    undo_spreading(x, n, blocks, k, spread);
    collapse_mask(&iy, n, blocks)
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
