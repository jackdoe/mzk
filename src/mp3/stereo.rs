use super::header::{
    is_mono, is_ms_stereo, test_i_stereo, test_mpeg1, test_ms_stereo,
};
use super::requant::ldexp_q2;
use super::sideinfo::GrInfo;

pub fn midside_stereo(grbuf: &mut [f32], base: usize, n: usize) {
    for i in 0..n {
        let a = grbuf[base + i];
        let b = grbuf[base + 576 + i];
        grbuf[base + i] = a + b;
        grbuf[base + 576 + i] = a - b;
    }
}

fn intensity_band(grbuf: &mut [f32], base: usize, n: usize, kl: f32, kr: f32) {
    for i in 0..n {
        grbuf[base + 576 + i] = grbuf[base + i] * kr;
        grbuf[base + i] = grbuf[base + i] * kl;
    }
}

fn stereo_top_band(grbuf: &[f32], rbase: usize, sfb: &[u8], nbands: usize, max_band: &mut [i32; 3]) {
    max_band[0] = -1;
    max_band[1] = -1;
    max_band[2] = -1;
    let mut r = rbase;
    for i in 0..nbands {
        let len = sfb[i] as usize;
        let mut k = 0usize;
        while k < len {
            if grbuf[r + k] != 0.0 || grbuf[r + k + 1] != 0.0 {
                max_band[i % 3] = i as i32;
                break;
            }
            k += 2;
        }
        r += len;
    }
}

fn stereo_process(
    grbuf: &mut [f32],
    mut base: usize,
    ist_pos: &[u8],
    sfb: &[u8],
    hdr: &[u8],
    max_band: &[i32; 3],
    mpeg2_sh: i32,
) {
    const PAN: [f32; 14] = [
        0.0, 1.0, 0.21132487, 0.78867513, 0.36602540, 0.63397460, 0.5, 0.5, 0.63397460, 0.36602540,
        0.78867513, 0.21132487, 1.0, 0.0,
    ];
    let max_pos: u32 = if test_mpeg1(hdr) { 7 } else { 64 };

    let mut i = 0usize;
    while sfb[i] != 0 {
        let ipos = ist_pos[i] as u32;
        let len = sfb[i] as usize;
        if i as i32 > max_band[i % 3] && ipos < max_pos {
            let s = if test_ms_stereo(hdr) { 1.41421356 } else { 1.0 };
            let (mut kl, mut kr);
            if test_mpeg1(hdr) {
                kl = PAN[2 * ipos as usize];
                kr = PAN[2 * ipos as usize + 1];
            } else {
                kl = 1.0;
                kr = ldexp_q2(1.0, (((ipos + 1) >> 1) as i32) << mpeg2_sh);
                if ipos & 1 != 0 {
                    kl = kr;
                    kr = 1.0;
                }
            }
            intensity_band(grbuf, base, len, kl * s, kr * s);
        } else if test_ms_stereo(hdr) {
            midside_stereo(grbuf, base, len);
        }
        base += len;
        i += 1;
    }
}

pub fn intensity_stereo(grbuf: &mut [f32], ist_pos: &mut [u8], gr: &[GrInfo], hdr: &[u8]) {
    let g = &gr[0];
    let n_sfb = (g.n_long_sfb + g.n_short_sfb) as usize;
    let max_blocks = if g.n_short_sfb != 0 { 3 } else { 1 };
    let mut max_band = [0i32; 3];

    stereo_top_band(grbuf, 576, g.sfbtab, n_sfb, &mut max_band);
    if g.n_long_sfb != 0 {
        let m = max_band[0].max(max_band[1]).max(max_band[2]);
        max_band[0] = m;
        max_band[1] = m;
        max_band[2] = m;
    }
    for i in 0..max_blocks {
        let default_pos: u8 = if test_mpeg1(hdr) { 3 } else { 0 };
        let itop = n_sfb - max_blocks + i;
        let prev = itop - max_blocks;
        ist_pos[itop] = if max_band[i] >= prev as i32 {
            default_pos
        } else {
            ist_pos[prev]
        };
    }
    let mpeg2_sh = (gr[1].scalefac_compress & 1) as i32;
    stereo_process(grbuf, 0, ist_pos, g.sfbtab, hdr, &max_band, mpeg2_sh);
}

pub fn reorder(grbuf: &mut [f32], gb: usize, sfb: &[u8]) {
    let mut scratch = [0.0f32; 576];
    let mut src = gb;
    let mut di = 0usize;
    let mut si = 0usize;
    loop {
        let len = sfb[si] as usize;
        if len == 0 {
            break;
        }
        for _ in 0..len {
            scratch[di] = grbuf[src];
            scratch[di + 1] = grbuf[src + len];
            scratch[di + 2] = grbuf[src + 2 * len];
            di += 3;
            src += 1;
        }
        si += 3;
        src += 2 * len;
    }
    for k in 0..di {
        grbuf[gb + k] = scratch[k];
    }
}

pub fn antialias(grbuf: &mut [f32], gb: usize, nbands: usize) {
    const AA: [[f32; 8]; 2] = [
        [
            0.85749293, 0.88174200, 0.94962865, 0.98331459, 0.99551782, 0.99916056, 0.99989920,
            0.99999316,
        ],
        [
            0.51449576, 0.47173197, 0.31337745, 0.18191320, 0.09457419, 0.04096558, 0.01419856,
            0.00369997,
        ],
    ];
    let mut base = gb;
    for _ in 0..nbands {
        for i in 0..8 {
            let u = grbuf[base + 18 + i];
            let d = grbuf[base + 17 - i];
            grbuf[base + 18 + i] = u * AA[0][i] - d * AA[1][i];
            grbuf[base + 17 - i] = u * AA[1][i] + d * AA[0][i];
        }
        base += 18;
    }
}

pub fn apply_stereo(grbuf: &mut [f32], ist_pos: &mut [u8], gr: &[GrInfo], hdr: &[u8]) {
    if is_mono(hdr) || gr.len() < 2 {
        return;
    }
    if test_i_stereo(hdr) {
        intensity_stereo(grbuf, ist_pos, gr, hdr);
    } else if is_ms_stereo(hdr) {
        midside_stereo(grbuf, 0, 576);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_intensity_does_not_index_second_granule() {
        let hdr = [0xff_u8, 0xfb, 0x90, 0xd0];
        assert!(is_mono(&hdr));
        assert!(test_i_stereo(&hdr));
        let mut grbuf = [0.0f32; 1152];
        let mut ist_pos = [0u8; 39];
        let gr = [GrInfo::default()];
        apply_stereo(&mut grbuf, &mut ist_pos, &gr, &hdr);
    }
}
