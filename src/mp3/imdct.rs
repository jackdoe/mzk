use super::sideinfo::{SHORT_BLOCK_TYPE, STOP_BLOCK_TYPE};

const TWID9: [f32; 18] = [
    0.73727734, 0.79335334, 0.84339145, 0.88701083, 0.92387953, 0.95371695, 0.97629601, 0.99144486,
    0.99904822, 0.67559021, 0.60876143, 0.53729961, 0.46174861, 0.38268343, 0.30070580, 0.21643961,
    0.13052619, 0.04361938,
];

const TWID3: [f32; 6] = [
    0.79335334, 0.92387953, 0.99144486, 0.60876143, 0.38268343, 0.13052619,
];

const MDCT_WINDOW: [[f32; 18]; 2] = [
    [
        0.99904822, 0.99144486, 0.97629601, 0.95371695, 0.92387953, 0.88701083, 0.84339145,
        0.79335334, 0.73727734, 0.04361938, 0.13052619, 0.21643961, 0.30070580, 0.38268343,
        0.46174861, 0.53729961, 0.60876143, 0.67559021,
    ],
    [
        1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.99144486, 0.92387953, 0.79335334, 0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.13052619, 0.38268343, 0.60876143,
    ],
];

fn dct3_9(y: &mut [f32]) {
    let (s0, s2, s4, s6, s8) = (y[0], y[2], y[4], y[6], y[8]);
    let t0 = s0 + s6 * 0.5;
    let mut s0 = s0 - s6;
    let t4 = (s4 + s2) * 0.93969262;
    let t2 = (s8 + s2) * 0.76604444;
    let s6b = (s4 - s8) * 0.17364818;
    let s4b = s4 + s8 - s2;

    let s2b = s0 - s4b * 0.5;
    y[4] = s4b + s0;
    let s8b = t0 - t2 + s6b;
    s0 = t0 - t4 + t2;
    let s4c = t0 + t4 - s6b;

    let (s1, mut s3, s5, s7) = (y[1], y[3], y[5], y[7]);
    s3 *= 0.86602540;
    let t0b = (s5 + s1) * 0.98480775;
    let t4b = (s5 - s7) * 0.34202014;
    let t2b = (s1 + s7) * 0.64278761;
    let s1b = (s1 - s5 - s7) * 0.86602540;

    let s5b = t0b - s3 - t2b;
    let s7b = t4b - s3 - t0b;
    let s3b = t4b + s3 - t2b;

    y[0] = s4c - s7b;
    y[1] = s2b + s1b;
    y[2] = s0 - s3b;
    y[3] = s8b + s5b;
    y[5] = s8b - s5b;
    y[6] = s0 + s3b;
    y[7] = s2b - s1b;
    y[8] = s4c + s7b;
}

fn imdct36(grbuf: &mut [f32], gb: usize, overlap: &mut [f32], ob: usize, window: &[f32], nbands: usize) {
    for j in 0..nbands {
        let g = gb + j * 18;
        let o = ob + j * 9;
        let mut co = [0.0f32; 9];
        let mut si = [0.0f32; 9];
        co[0] = -grbuf[g];
        si[0] = grbuf[g + 17];
        for i in 0..4 {
            si[8 - 2 * i] = grbuf[g + 4 * i + 1] - grbuf[g + 4 * i + 2];
            co[1 + 2 * i] = grbuf[g + 4 * i + 1] + grbuf[g + 4 * i + 2];
            si[7 - 2 * i] = grbuf[g + 4 * i + 4] - grbuf[g + 4 * i + 3];
            co[2 + 2 * i] = -(grbuf[g + 4 * i + 3] + grbuf[g + 4 * i + 4]);
        }
        dct3_9(&mut co);
        dct3_9(&mut si);
        si[1] = -si[1];
        si[3] = -si[3];
        si[5] = -si[5];
        si[7] = -si[7];

        for i in 0..9 {
            let ovl = overlap[o + i];
            let sum = co[i] * TWID9[9 + i] + si[i] * TWID9[i];
            overlap[o + i] = co[i] * TWID9[i] - si[i] * TWID9[9 + i];
            grbuf[g + i] = ovl * window[i] - sum * window[9 + i];
            grbuf[g + 17 - i] = ovl * window[9 + i] + sum * window[i];
        }
    }
}

fn idct3(x0: f32, x1: f32, x2: f32, dst: &mut [f32; 3]) {
    let m1 = x1 * 0.86602540;
    let a1 = x0 - x2 * 0.5;
    dst[1] = x0 + x2;
    dst[0] = a1 + m1;
    dst[2] = a1 - m1;
}

fn imdct12(x: &[f32], xb: usize, dst: &mut [f32], db: usize, overlap: &mut [f32], ob: usize) {
    let mut co = [0.0f32; 3];
    let mut si = [0.0f32; 3];
    idct3(-x[xb], x[xb + 6] + x[xb + 3], x[xb + 12] + x[xb + 9], &mut co);
    idct3(x[xb + 15], x[xb + 12] - x[xb + 9], x[xb + 6] - x[xb + 3], &mut si);
    si[1] = -si[1];

    for i in 0..3 {
        let ovl = overlap[ob + i];
        let sum = co[i] * TWID3[3 + i] + si[i] * TWID3[i];
        overlap[ob + i] = co[i] * TWID3[i] - si[i] * TWID3[3 + i];
        dst[db + i] = ovl * TWID3[2 - i] - sum * TWID3[5 - i];
        dst[db + 5 - i] = ovl * TWID3[5 - i] + sum * TWID3[2 - i];
    }
}

fn imdct_short(grbuf: &mut [f32], gb: usize, overlap: &mut [f32], ob: usize, nbands: usize) {
    for n in 0..nbands {
        let g = gb + n * 18;
        let o = ob + n * 9;
        let mut tmp = [0.0f32; 18];
        tmp.copy_from_slice(&grbuf[g..g + 18]);
        for i in 0..6 {
            grbuf[g + i] = overlap[o + i];
        }
        imdct12(&tmp, 0, grbuf, g + 6, overlap, o + 6);
        imdct12(&tmp, 1, grbuf, g + 12, overlap, o + 6);
        imdct12_into_overlap(&tmp, 2, overlap, o, o + 6);
    }
}

fn imdct12_into_overlap(x: &[f32], xb: usize, overlap: &mut [f32], db: usize, ob: usize) {
    let mut co = [0.0f32; 3];
    let mut si = [0.0f32; 3];
    idct3(-x[xb], x[xb + 6] + x[xb + 3], x[xb + 12] + x[xb + 9], &mut co);
    idct3(x[xb + 15], x[xb + 12] - x[xb + 9], x[xb + 6] - x[xb + 3], &mut si);
    si[1] = -si[1];

    let mut dst = [0.0f32; 6];
    let mut ov = [0.0f32; 3];
    for i in 0..3 {
        let ovl = overlap[ob + i];
        let sum = co[i] * TWID3[3 + i] + si[i] * TWID3[i];
        ov[i] = co[i] * TWID3[i] - si[i] * TWID3[3 + i];
        dst[i] = ovl * TWID3[2 - i] - sum * TWID3[5 - i];
        dst[5 - i] = ovl * TWID3[5 - i] + sum * TWID3[2 - i];
    }
    for i in 0..6 {
        overlap[db + i] = dst[i];
    }
    for i in 0..3 {
        overlap[ob + i] = ov[i];
    }
}

fn change_sign(grbuf: &mut [f32]) {
    let mut b = 0usize;
    let mut base = 18usize;
    while b < 32 {
        let mut i = 1usize;
        while i < 18 {
            grbuf[base + i] = -grbuf[base + i];
            i += 2;
        }
        b += 2;
        base += 36;
    }
}

pub fn imdct_gr(grbuf: &mut [f32], overlap: &mut [f32], block_type: u8, n_long_bands: usize) {
    let mut gb = 0usize;
    let mut ob = 0usize;
    if n_long_bands != 0 {
        imdct36(grbuf, gb, overlap, ob, &MDCT_WINDOW[0], n_long_bands);
        gb += 18 * n_long_bands;
        ob += 9 * n_long_bands;
    }
    if block_type == SHORT_BLOCK_TYPE {
        imdct_short(grbuf, gb, overlap, ob, 32 - n_long_bands);
    } else {
        let w = (block_type == STOP_BLOCK_TYPE) as usize;
        imdct36(grbuf, gb, overlap, ob, &MDCT_WINDOW[w], 32 - n_long_bands);
    }
    change_sign(grbuf);
}
