mod cwrs;
mod rate;
mod allocation;
mod bands;
mod vq;
mod energy;
mod synth;
mod tables;

use crate::mdct::Mdct;
use crate::range::RangeDecoder;
use cwrs::log2_frac;
use rate::PulseCache;
use synth::{PostFilter, Synth};
pub use tables::*;

const SPREAD_ICDF: [u8; 4] = [25, 23, 2, 0];
const TAPSET_ICDF: [u8; 3] = [2, 1, 0];
const TRIM_ICDF: [u8; 11] = [126, 124, 119, 109, 87, 41, 19, 9, 4, 2, 0];
const TF_SELECT_TABLE: [[i8; 8]; 4] = [
    [0, -1, 0, -1, 0, -1, 0, -1],
    [0, -1, 0, -2, 1, 0, 1, -1],
    [0, -2, 0, -3, 2, 0, 1, -1],
    [0, -2, 0, -3, 3, 0, 1, -1],
];

pub struct Mode {
    pub e_bands: Vec<i32>,
    pub nb_ebands: usize,
    pub log_n: Vec<i32>,
    pub alloc_vectors: Vec<u8>,
    pub cache: PulseCache,
    pub mdct: Mdct,
    pub frame: usize,
    pub overlap: usize,
    pub max_lm: usize,
    pub short_mdct: usize,
}

impl Mode {
    pub fn new() -> Self {
        let e_bands: Vec<i32> = EBANDS_5MS.to_vec();
        let nb_ebands = NB_EBANDS;
        let log_n: Vec<i32> = (0..nb_ebands)
            .map(|i| log2_frac((e_bands[i + 1] - e_bands[i]) as u32, BITRES))
            .collect();
        let mut alloc_vectors = vec![0u8; BITALLOC_SIZE * nb_ebands];
        for i in 0..BITALLOC_SIZE {
            for j in 0..nb_ebands {
                alloc_vectors[i * nb_ebands + j] = BAND_ALLOC[i][j];
            }
        }
        let cache = rate::compute_pulse_cache(&e_bands, nb_ebands, &log_n, MAX_LM);
        let mdct = Mdct::new(MDCT_N, MAX_LM, OVERLAP);
        Mode {
            e_bands,
            nb_ebands,
            log_n,
            alloc_vectors,
            cache,
            mdct,
            frame: FRAME,
            overlap: OVERLAP,
            max_lm: MAX_LM,
            short_mdct: SHORT_MDCT,
        }
    }
}

fn tf_decode(
    start: usize,
    end: usize,
    is_transient: bool,
    lm: i32,
    rd: &mut RangeDecoder,
) -> Vec<i32> {
    let mut tf_res = vec![0i32; end];
    let budget0 = (rd.storage() * 8) as i32;
    let mut tell = rd.tell();
    let mut logp = if is_transient { 2 } else { 4 };
    let tf_select_rsv = (lm > 0 && tell + logp + 1 <= budget0) as i32;
    let budget = budget0 - tf_select_rsv;
    let mut tf_changed = 0i32;
    let mut curr = 0i32;
    for i in start..end {
        if tell + logp <= budget {
            curr ^= rd.dec_bit_logp(logp as u32) as i32;
            tell = rd.tell();
            tf_changed |= curr;
        }
        tf_res[i] = curr;
        logp = if is_transient { 4 } else { 5 };
    }
    let it = is_transient as usize;
    let lmu = lm as usize;
    let mut tf_select = 0usize;
    if tf_select_rsv == 1
        && TF_SELECT_TABLE[lmu][4 * it + tf_changed as usize]
            != TF_SELECT_TABLE[lmu][4 * it + 2 + tf_changed as usize]
    {
        tf_select = rd.dec_bit_logp(1) as usize;
    }
    for i in start..end {
        tf_res[i] = TF_SELECT_TABLE[lmu][4 * it + 2 * tf_select + tf_res[i] as usize] as i32;
    }
    tf_res
}

pub struct DecoderState {
    old_band_e: Vec<f32>,
    old_log_e: Vec<f32>,
    old_log_e2: Vec<f32>,
    synth: Synth,
    rng: u32,
}

impl DecoderState {
    pub fn new(channels: usize) -> Self {
        let n = channels * NB_EBANDS;
        DecoderState {
            old_band_e: vec![0.0; n],
            old_log_e: vec![-28.0; n],
            old_log_e2: vec![-28.0; n],
            synth: Synth::new(channels),
            rng: 0,
        }
    }

    pub fn reset(&mut self) {
        for v in self.old_band_e.iter_mut() {
            *v = 0.0;
        }
        for v in self.old_log_e.iter_mut() {
            *v = -28.0;
        }
        for v in self.old_log_e2.iter_mut() {
            *v = -28.0;
        }
        self.synth.reset();
        self.rng = 0;
    }
}

pub fn decode_frame(state: &mut DecoderState, mode: &Mode, frame: &[u8], stereo: bool) -> Vec<f32> {
    let nb = mode.nb_ebands;
    let c = if stereo { 2 } else { 1 };
    let lm = mode.max_lm as i32;
    let m = 1usize << lm;
    let n = mode.frame;
    let start = 0usize;
    let end = nb;
    let eb = &mode.e_bands;
    let len = frame.len();

    let mut pcm = vec![0.0f32; n * c];

    let mut rd = RangeDecoder::new(frame);
    let total_bits = (len * 8) as i32;
    let mut tell = rd.tell();
    let silence = if tell >= total_bits {
        true
    } else if tell == 1 {
        rd.dec_bit_logp(15) == 1
    } else {
        false
    };

    if silence {
        for v in state.old_band_e.iter_mut() {
            *v = -28.0;
        }
        state.old_log_e2.clone_from(&state.old_log_e);
        state.old_log_e.clone_from(&state.old_band_e);
        let mut xy = vec![0.0f32; c * n];
        let pf = PostFilter::none();
        state
            .synth
            .process(mode, &mut xy, &state.old_band_e, start, end, lm, false, true, c, &pf, &mut pcm);
        return pcm;
    }

    let mut pf_pitch = 0i32;
    let mut pf_gain = 0.0f32;
    let mut pf_tapset = 0i32;
    if start == 0 && tell + 16 <= total_bits {
        if rd.dec_bit_logp(1) == 1 {
            let octave = rd.dec_uint(6);
            pf_pitch = (16i32 << octave) + rd.dec_bits(4 + octave) as i32 - 1;
            let qg = rd.dec_bits(3) as i32;
            if rd.tell() + 2 <= total_bits {
                pf_tapset = rd.dec_icdf(&TAPSET_ICDF, 2) as i32;
            }
            pf_gain = 0.09375 * (qg + 1) as f32;
        }
        tell = rd.tell();
    }

    let is_transient = if lm > 0 && tell + 3 <= total_bits {
        rd.dec_bit_logp(3) == 1
    } else {
        false
    };
    let short_blocks = if is_transient { m as i32 } else { 0 };
    tell = rd.tell();
    let intra = if tell + 3 <= total_bits {
        rd.dec_bit_logp(3) == 1
    } else {
        false
    };

    energy::decode_coarse(&mut rd, mode, start, end, intra, c, lm as usize, &mut state.old_band_e);

    let tf_res = tf_decode(start, end, is_transient, lm, &mut rd);

    tell = rd.tell();
    let spread = if tell + 4 <= total_bits {
        rd.dec_icdf(&SPREAD_ICDF, 5) as i32
    } else {
        2
    };

    let cap = allocation::init_caps(mode, lm as usize, c);

    let mut offsets = vec![0i32; nb];
    let mut dynalloc_logp = 6i32;
    let mut total_frac = total_bits << BITRES;
    let mut tellf = rd.tell_frac() as i32;
    for i in start..end {
        let width = c as i32 * (eb[i + 1] - eb[i]) << lm;
        let quanta = (width << BITRES).min((6 << BITRES).max(width));
        let mut loop_logp = dynalloc_logp;
        let mut boost = 0i32;
        while tellf + (loop_logp << BITRES) < total_frac && boost < cap[i] {
            let flag = rd.dec_bit_logp(loop_logp as u32);
            tellf = rd.tell_frac() as i32;
            if flag == 0 {
                break;
            }
            boost += quanta;
            total_frac -= quanta;
            loop_logp = 1;
        }
        offsets[i] = boost;
        if boost > 0 {
            dynalloc_logp = (dynalloc_logp - 1).max(2);
        }
    }

    let alloc_trim = if tellf + (6 << BITRES) <= total_frac {
        rd.dec_icdf(&TRIM_ICDF, 7) as i32
    } else {
        5
    };

    let mut bits = ((len as i32) * 8 << BITRES) - rd.tell_frac() as i32 - 1;
    let anti_collapse_rsv = if is_transient && lm >= 2 && bits >= ((lm + 2) << BITRES) {
        1 << BITRES
    } else {
        0
    };
    bits -= anti_collapse_rsv;

    let alloc = allocation::clt_compute_allocation(
        mode, start, end, &offsets, &cap, alloc_trim, bits, c as i32, lm, &mut rd,
    );

    energy::decode_fine(&mut rd, mode, start, end, &alloc.fine_bits, c, &mut state.old_band_e);

    let mut xy = vec![0.0f32; c * n];
    let mut collapse = vec![0u8; c * nb];
    let total_band_bits = (len as i32) * (8 << BITRES) - anti_collapse_rsv;
    {
        let (x0, x1) = xy.split_at_mut(n);
        let y = if c == 2 { Some(x1) } else { None };
        bands::quant_all_bands(
            mode,
            start,
            end,
            x0,
            y,
            &mut collapse,
            &alloc.pulses,
            short_blocks,
            spread,
            alloc.dual_stereo,
            alloc.intensity,
            &tf_res,
            total_band_bits,
            alloc.balance,
            &mut rd,
            lm,
            alloc.coded_bands,
            &mut state.rng,
        );
    }

    let anti_collapse_on = if anti_collapse_rsv > 0 {
        rd.dec_bits(1) == 1
    } else {
        false
    };

    let bits_left = (len as i32) * 8 - rd.tell();
    energy::decode_final(
        &mut rd,
        mode,
        start,
        end,
        &alloc.fine_bits,
        &alloc.fine_priority,
        bits_left,
        c,
        &mut state.old_band_e,
    );

    if anti_collapse_on {
        bands::anti_collapse(
            mode,
            &mut xy,
            &collapse,
            lm,
            c,
            n,
            start,
            end,
            &state.old_band_e,
            &state.old_log_e,
            &state.old_log_e2,
            &alloc.pulses,
            &mut state.rng,
        );
    }

    let pf = PostFilter {
        period: pf_pitch,
        gain: pf_gain,
        tapset: pf_tapset,
    };
    state.synth.process(
        mode,
        &mut xy,
        &state.old_band_e,
        start,
        end,
        lm,
        is_transient,
        false,
        c,
        &pf,
        &mut pcm,
    );

    if !is_transient {
        state.old_log_e2.clone_from(&state.old_log_e);
        state.old_log_e.clone_from(&state.old_band_e);
    } else {
        for i in 0..c * nb {
            state.old_log_e[i] = state.old_log_e[i].min(state.old_band_e[i]);
        }
    }

    pcm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eband_edges_match_rfc() {
        assert_eq!(EBANDS_5MS[0], 0);
        assert_eq!(EBANDS_5MS[21], 100);
        assert_eq!(EBANDS_5MS.len(), 22);
    }

    #[test]
    fn mode_has_21_bands_and_960_frame() {
        let m = Mode::new();
        assert_eq!(m.nb_ebands, 21);
        assert_eq!(m.frame, 960);
        assert_eq!(*m.e_bands.last().unwrap(), 100);
    }

    #[test]
    fn decodes_tiny_within_rms_tolerance() {
        let opus = match std::fs::read("tests/fixtures/tiny.opus") {
            Ok(d) => d,
            Err(_) => return,
        };
        let want_bytes = std::fs::read("tests/fixtures/tiny.f32le").unwrap();
        let want: Vec<f32> = want_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();

        let stream = crate::ogg::OpusStream::parse(&opus).unwrap();
        let mode = Mode::new();
        let mut state = DecoderState::new(2);
        let mut got: Vec<f32> = Vec::new();
        for pkt in &stream.packets {
            let cfg = crate::toc::Config::parse(pkt).unwrap();
            let frame = decode_frame(&mut state, &mode, cfg.frame, cfg.stereo);
            got.extend_from_slice(&frame);
        }
        let skip = stream.head.pre_skip as usize * 2;
        let got = &got[skip.min(got.len())..];
        let nn = got.len().min(want.len());
        assert!(nn > 48000, "decoded too little: {nn}");
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for i in 0..nn {
            let d = (got[i] - want[i]) as f64;
            num += d * d;
            den += (want[i] as f64).powi(2);
        }
        let rms = (num / den.max(1e-9)).sqrt();
        assert!(rms < 0.05, "relative RMS {rms} too high");
    }

    #[test]
    fn cache_caps_populated() {
        let m = Mode::new();
        assert_eq!(m.cache.caps.len(), (MAX_LM + 1) * 2 * NB_EBANDS);
        assert!(m.cache.caps.iter().any(|&c| c > 0));
        assert!(!m.cache.bits.is_empty());
    }
}
