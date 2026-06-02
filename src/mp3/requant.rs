use super::bits::Bits;
use super::header::{is_ms_stereo, test_i_stereo, test_mpeg1};
use super::sideinfo::GrInfo;
use super::tables::SCF_PARTITIONS;

const BITS_DEQUANTIZER_OUT: i32 = 0;
const MAX_SCFI: i32 = 48;

fn read_scalefactors(
    scf: &mut [u8],
    ist_pos: &mut [u8],
    scf_size: &[u8; 4],
    scf_count: &[u8],
    bs: &mut Bits,
    mut scfsi: i32,
) {
    let mut so = 0usize;
    let mut io = 0usize;
    let mut i = 0usize;
    while i < 4 && scf_count[i] != 0 {
        let cnt = scf_count[i] as usize;
        if scfsi & 8 != 0 {
            for k in 0..cnt {
                scf[so + k] = ist_pos[io + k];
            }
        } else {
            let bits = scf_size[i] as u32;
            if bits == 0 {
                for k in 0..cnt {
                    scf[so + k] = 0;
                    ist_pos[io + k] = 0;
                }
            } else {
                let max_scf = if scfsi < 0 { (1i32 << bits) - 1 } else { -1 };
                for k in 0..cnt {
                    let s = bs.get_bits(bits) as i32;
                    ist_pos[io + k] = if s == max_scf { 255 } else { s as u8 };
                    scf[so + k] = s as u8;
                }
            }
        }
        io += cnt;
        so += cnt;
        scfsi *= 2;
        i += 1;
    }
    scf[so] = 0;
    scf[so + 1] = 0;
    scf[so + 2] = 0;
}

pub fn ldexp_q2(mut y: f32, mut exp_q2: i32) -> f32 {
    const EXPFRAC: [f32; 4] = [9.31322575e-10, 7.83145814e-10, 6.58544508e-10, 5.53767716e-10];
    loop {
        let e = (30 * 4).min(exp_q2);
        y *= EXPFRAC[(e & 3) as usize] * ((1i32 << 30) >> (e >> 2)) as f32;
        exp_q2 -= e;
        if exp_q2 <= 0 {
            break;
        }
    }
    y
}

fn pow_43(x: i32) -> f32 {
    (x as f32).powf(4.0 / 3.0)
}

pub fn requantize(is: &[i32; 576], scf: &[f32], sfbtab: &[u8], xr: &mut [f32]) {
    let mut i = 0usize;
    let mut band = 0usize;
    while sfbtab[band] != 0 && i < 576 {
        let gain = scf[band];
        for _ in 0..sfbtab[band] {
            let v = is[i];
            xr[i] = if v < 0 {
                -pow_43(-v)
            } else {
                pow_43(v)
            } * gain;
            i += 1;
        }
        band += 1;
    }
    for x in xr.iter_mut().take(576).skip(i) {
        *x = 0.0;
    }
}

pub fn decode_scalefactors(
    hdr: &[u8],
    ist_pos: &mut [u8],
    bs: &mut Bits,
    gr: &GrInfo,
    scf: &mut [f32],
    ch: usize,
) {
    let part_idx = (gr.n_short_sfb != 0) as usize + (gr.n_long_sfb == 0) as usize;
    let mut part_off = 0usize;
    let mut scf_size = [0u8; 4];
    let mut iscf = [0u8; 40];
    let scf_shift = gr.scalefac_scale as i32 + 1;
    let mut scfsi = gr.scfsi as i32;

    if test_mpeg1(hdr) {
        const SCFC_DECODE: [u8; 16] = [0, 1, 2, 3, 12, 5, 6, 7, 9, 10, 11, 13, 14, 15, 18, 19];
        let part = SCFC_DECODE[gr.scalefac_compress as usize] as i32;
        scf_size[0] = (part >> 2) as u8;
        scf_size[1] = scf_size[0];
        scf_size[2] = (part & 3) as u8;
        scf_size[3] = scf_size[2];
    } else {
        const MOD: [u8; 24] = [
            5, 5, 4, 4, 5, 5, 4, 1, 4, 3, 1, 1, 5, 6, 6, 1, 4, 4, 4, 1, 4, 3, 1, 1,
        ];
        let ist = (test_i_stereo(hdr) && ch != 0) as usize;
        let mut sfc = (gr.scalefac_compress >> ist) as i32;
        let mut k = ist * 3 * 4;
        while sfc >= 0 {
            let mut modprod = 1i32;
            for i in (0..4).rev() {
                scf_size[i] = (sfc / modprod % MOD[k + i] as i32) as u8;
                modprod *= MOD[k + i] as i32;
            }
            sfc -= modprod;
            k += 4;
        }
        part_off = k;
        scfsi = -16;
    }

    read_scalefactors(
        &mut iscf,
        ist_pos,
        &scf_size,
        &SCF_PARTITIONS[part_idx][part_off..],
        bs,
        scfsi,
    );

    if gr.n_short_sfb != 0 {
        let sh = 3 - scf_shift;
        let nl = gr.n_long_sfb as usize;
        let mut i = 0usize;
        while i < gr.n_short_sfb as usize {
            for j in 0..3 {
                iscf[nl + i + j] =
                    iscf[nl + i + j].wrapping_add(((gr.subblock_gain[j] as i32) << sh) as u8);
            }
            i += 3;
        }
    } else if gr.preflag != 0 {
        const PREAMP: [u8; 10] = [1, 1, 1, 1, 2, 2, 3, 3, 3, 2];
        for i in 0..10 {
            iscf[11 + i] = iscf[11 + i].wrapping_add(PREAMP[i]);
        }
    }

    let gain_exp = gr.global_gain as i32 + BITS_DEQUANTIZER_OUT * 4 - 210
        - if is_ms_stereo(hdr) { 2 } else { 0 };
    let gain = ldexp_q2((1i32 << (MAX_SCFI / 4)) as f32, MAX_SCFI - gain_exp);
    let n = (gr.n_long_sfb + gr.n_short_sfb) as usize;
    for i in 0..n {
        scf[i] = ldexp_q2(gain, (iscf[i] as i32) << scf_shift);
    }
}
