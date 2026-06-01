use super::tables::MAX_FINE_BITS;
use super::Mode;
use crate::range::RangeDecoder;

const LAPLACE_MINP: u32 = 1;
const LAPLACE_LOG_MINP: u32 = 0;
const LAPLACE_NMIN: u32 = 16;

const SMALL_ENERGY_ICDF: [u8; 3] = [2, 1, 0];

const PRED_COEF: [f32; 4] = [
    29440.0 / 32768.0,
    26112.0 / 32768.0,
    21248.0 / 32768.0,
    16384.0 / 32768.0,
];
const BETA_COEF: [f32; 4] = [
    30147.0 / 32768.0,
    22282.0 / 32768.0,
    12124.0 / 32768.0,
    6554.0 / 32768.0,
];
const BETA_INTRA: f32 = 4915.0 / 32768.0;

const E_PROB_MODEL: [[[u8; 42]; 2]; 4] = [
    [
        [
            72, 127, 65, 129, 66, 128, 65, 128, 64, 128, 62, 128, 64, 128, 64, 128, 92, 78, 92, 79,
            92, 78, 90, 79, 116, 41, 115, 40, 114, 40, 132, 26, 132, 26, 145, 17, 161, 12, 176, 10,
            177, 11,
        ],
        [
            24, 179, 48, 138, 54, 135, 54, 132, 53, 134, 56, 133, 55, 132, 55, 132, 61, 114, 70,
            96, 74, 88, 75, 88, 87, 74, 89, 66, 91, 67, 100, 59, 108, 50, 120, 40, 122, 37, 97, 43,
            78, 50,
        ],
    ],
    [
        [
            83, 78, 84, 81, 88, 75, 86, 74, 87, 71, 90, 73, 93, 74, 93, 74, 109, 40, 114, 36, 117,
            34, 117, 34, 143, 17, 145, 18, 146, 19, 162, 12, 165, 10, 178, 7, 189, 6, 190, 8, 177,
            9,
        ],
        [
            23, 178, 54, 115, 63, 102, 66, 98, 69, 99, 74, 89, 71, 91, 73, 91, 78, 89, 86, 80, 92,
            66, 93, 64, 102, 59, 103, 60, 104, 60, 117, 52, 123, 44, 138, 35, 133, 31, 97, 38, 77,
            45,
        ],
    ],
    [
        [
            61, 90, 93, 60, 105, 42, 107, 41, 110, 45, 116, 38, 113, 38, 112, 38, 124, 26, 132, 27,
            136, 19, 140, 20, 155, 14, 159, 16, 158, 18, 170, 13, 177, 10, 187, 8, 192, 6, 175, 9,
            159, 10,
        ],
        [
            21, 178, 59, 110, 71, 86, 75, 85, 84, 83, 91, 66, 88, 73, 87, 72, 92, 75, 98, 72, 105,
            58, 107, 54, 115, 52, 114, 55, 112, 56, 129, 51, 132, 40, 150, 33, 140, 29, 98, 35, 77,
            42,
        ],
    ],
    [
        [
            42, 121, 96, 66, 108, 43, 111, 40, 117, 44, 123, 32, 120, 36, 119, 33, 127, 33, 134,
            34, 139, 21, 147, 23, 152, 20, 158, 25, 154, 26, 166, 21, 173, 16, 184, 13, 184, 10,
            150, 13, 139, 15,
        ],
        [
            22, 178, 63, 114, 74, 82, 84, 83, 92, 82, 103, 62, 96, 72, 96, 67, 101, 73, 107, 72,
            113, 55, 118, 52, 125, 52, 118, 52, 117, 55, 135, 49, 137, 39, 157, 32, 145, 29, 97,
            33, 77, 40,
        ],
    ],
];

fn ec_laplace_get_freq1(fs0: u32, decay: i32) -> u32 {
    let ft = 32768 - LAPLACE_MINP * (2 * LAPLACE_NMIN) - fs0;
    ((ft as i32 * (16384 - decay)) >> 15) as u32
}

fn ec_laplace_decode(dec: &mut RangeDecoder, mut fs: u32, decay: i32) -> i32 {
    let mut val: i32 = 0;
    let fm = dec.decode_bin(15);
    let mut fl: u32 = 0;
    if fm >= fs {
        val += 1;
        fl = fs;
        fs = ec_laplace_get_freq1(fs, decay) + LAPLACE_MINP;
        while fs > LAPLACE_MINP && fm >= fl + 2 * fs {
            fs *= 2;
            fl += fs;
            fs = (((fs - 2 * LAPLACE_MINP) as i32 * decay) >> 15) as u32;
            fs += LAPLACE_MINP;
            val += 1;
        }
        if fs <= LAPLACE_MINP {
            let di = ((fm - fl) >> (LAPLACE_LOG_MINP + 1)) as i32;
            val += di;
            fl += (2 * di as u32) * LAPLACE_MINP;
        }
        if fm < fl + fs {
            val = -val;
        } else {
            fl += fs;
        }
    }
    dec.update(fl, (fl + fs).min(32768), 32768);
    val
}

pub fn decode_coarse(
    rd: &mut RangeDecoder,
    mode: &Mode,
    start: usize,
    end: usize,
    intra: bool,
    channels: usize,
    lm: usize,
    old: &mut [f32],
) {
    let prob = &E_PROB_MODEL[lm][intra as usize];
    let (coef, beta) = if intra {
        (0.0f32, BETA_INTRA)
    } else {
        (PRED_COEF[lm], BETA_COEF[lm])
    };
    let nb = mode.nb_ebands;
    let budget = (rd.storage() * 8) as i32;
    let mut prev = [0.0f32; 2];
    for i in start..end {
        for c in 0..channels {
            let tell = rd.tell();
            let qi: i32 = if budget - tell >= 15 {
                let pi = 2 * i.min(20);
                ec_laplace_decode(rd, (prob[pi] as u32) << 7, (prob[pi + 1] as i32) << 6)
            } else if budget - tell >= 2 {
                let q = rd.dec_icdf(&SMALL_ENERGY_ICDF, 2) as i32;
                (q >> 1) ^ -(q & 1)
            } else if budget - tell >= 1 {
                -(rd.dec_bit_logp(1) as i32)
            } else {
                -1
            };
            let q = qi as f32;
            let idx = i + c * nb;
            old[idx] = (-9.0f32).max(old[idx]);
            let tmp = coef * old[idx] + prev[c] + q;
            old[idx] = tmp;
            prev[c] = prev[c] + q - beta * q;
        }
    }
}

pub fn decode_fine(
    rd: &mut RangeDecoder,
    mode: &Mode,
    start: usize,
    end: usize,
    fine_quant: &[i32],
    channels: usize,
    old: &mut [f32],
) {
    let nb = mode.nb_ebands;
    for i in start..end {
        if fine_quant[i] <= 0 {
            continue;
        }
        for c in 0..channels {
            let q2 = rd.dec_bits(fine_quant[i] as u32) as f32;
            let offset =
                (q2 + 0.5) * (1u32 << (14 - fine_quant[i])) as f32 * (1.0 / 16384.0) - 0.5;
            old[i + c * nb] += offset;
        }
    }
}

pub fn decode_final(
    rd: &mut RangeDecoder,
    mode: &Mode,
    start: usize,
    end: usize,
    fine_quant: &[i32],
    fine_priority: &[i32],
    mut bits_left: i32,
    channels: usize,
    old: &mut [f32],
) {
    let nb = mode.nb_ebands;
    for prio in 0..2 {
        let mut i = start;
        while i < end && bits_left >= channels as i32 {
            if fine_quant[i] >= MAX_FINE_BITS || fine_priority[i] != prio {
                i += 1;
                continue;
            }
            for c in 0..channels {
                let q2 = rd.dec_bits(1) as f32;
                let offset = (q2 - 0.5) * (1u32 << (14 - fine_quant[i] - 1)) as f32 * (1.0 / 16384.0);
                old[i + c * nb] += offset;
                bits_left -= 1;
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_energy_fills_all_bands_finite() {
        let mode = Mode::new();
        let buf = [0x33u8; 64];
        let mut rd = RangeDecoder::new(&buf);
        let mut old = vec![0.0f32; mode.nb_ebands * 2];
        decode_coarse(&mut rd, &mode, 0, mode.nb_ebands, true, 1, 3, &mut old);
        assert!(old.iter().all(|v| v.is_finite()));
    }
}
