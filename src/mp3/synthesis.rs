use super::tables::SYNTH_DTBL;
use std::f32::consts::PI;
use std::sync::OnceLock;

fn cos_matrix() -> &'static [[f32; 32]; 64] {
    static M: OnceLock<[[f32; 32]; 64]> = OnceLock::new();
    M.get_or_init(|| {
        let mut n = [[0.0f32; 32]; 64];
        for i in 0..64 {
            for j in 0..32 {
                n[i][j] = ((16 + i) as f32 * (2 * j + 1) as f32 * (PI / 64.0)).cos();
            }
        }
        n
    })
}

pub fn subband_synthesis(
    grbuf: &[f32],
    v: &mut [f32; 1024],
    off: &mut usize,
    out: &mut [f32],
    ch: usize,
    nch: usize,
) {
    let n = cos_matrix();
    for ss in 0..18 {
        *off = (*off + 1024 - 64) & 1023;
        let base = *off;
        for (i, row) in n.iter().enumerate() {
            let mut sum = 0.0;
            for j in 0..32 {
                sum += row[j] * grbuf[j * 18 + ss];
            }
            v[base + i] = sum;
        }
        let mut u = [0.0f32; 512];
        for i in 0..8 {
            for j in 0..32 {
                u[(i << 6) + j] = v[(base + (i << 7) + j) & 1023];
                u[(i << 6) + j + 32] = v[(base + (i << 7) + j + 96) & 1023];
            }
        }
        for i in 0..512 {
            u[i] *= SYNTH_DTBL[i];
        }
        for i in 0..32 {
            let mut sum = 0.0;
            for j in 0..16 {
                sum += u[(j << 5) + i];
            }
            out[(ss * 32 + i) * nch + ch] = sum;
        }
    }
}
