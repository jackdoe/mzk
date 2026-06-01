use super::bands::denormalise_bands;
use crate::celt::Mode;

const COMBFILTER_MINPERIOD: i32 = 15;
const SIG_SAT: f32 = 300000000.0;
const GAINS: [[f32; 3]; 3] = [
    [0.3066406250, 0.2170410156, 0.1296386719],
    [0.4638671875, 0.2680664062, 0.0],
    [0.7998046875, 0.1000976562, 0.0],
];

pub struct PostFilter {
    pub period: i32,
    pub gain: f32,
    pub tapset: i32,
}

impl PostFilter {
    pub fn none() -> Self {
        PostFilter {
            period: 0,
            gain: 0.0,
            tapset: 0,
        }
    }
}

const DECODE_BUFFER_SIZE: usize = 2048;

pub struct Synth {
    overlap: usize,
    decode_mem: Vec<Vec<f32>>,
    preemph: Vec<f32>,
    pf_period: i32,
    pf_period_old: i32,
    pf_gain: f32,
    pf_gain_old: f32,
    pf_tapset: i32,
    pf_tapset_old: i32,
}

impl Synth {
    pub fn new(channels: usize) -> Self {
        Synth {
            overlap: 0,
            decode_mem: (0..channels).map(|_| Vec::new()).collect(),
            preemph: vec![0.0; channels],
            pf_period: 0,
            pf_period_old: 0,
            pf_gain: 0.0,
            pf_gain_old: 0.0,
            pf_tapset: 0,
            pf_tapset_old: 0,
        }
    }

    pub fn reset(&mut self) {
        for h in self.decode_mem.iter_mut() {
            for v in h.iter_mut() {
                *v = 0.0;
            }
        }
        for v in self.preemph.iter_mut() {
            *v = 0.0;
        }
        self.pf_period = 0;
        self.pf_period_old = 0;
        self.pf_gain = 0.0;
        self.pf_gain_old = 0.0;
        self.pf_tapset = 0;
        self.pf_tapset_old = 0;
    }

    pub fn process(
        &mut self,
        mode: &Mode,
        x: &mut [f32],
        band_log_e: &[f32],
        start: usize,
        eff_end: usize,
        lm: i32,
        transient: bool,
        silence: bool,
        channels: usize,
        pf: &PostFilter,
        pcm_out: &mut [f32],
    ) {
        let overlap = mode.overlap;
        let nb_ebands = mode.nb_ebands;
        let n = mode.short_mdct << lm;
        let m = 1usize << lm;
        let mem_len = DECODE_BUFFER_SIZE + overlap;
        let out_off = DECODE_BUFFER_SIZE - n;

        let (b, nb, shift) = if transient {
            (m, mode.short_mdct, mode.max_lm)
        } else {
            (1usize, mode.short_mdct << lm, mode.max_lm - lm as usize)
        };

        if self.overlap != overlap {
            self.overlap = overlap;
            for h in self.decode_mem.iter_mut() {
                *h = vec![0.0; mem_len];
            }
        }

        let out_len = mode.mdct.out_len(shift);

        for c in 0..channels {
            let mem = &mut self.decode_mem[c];
            mem.copy_within(n..mem_len, 0);
            for v in mem[mem_len - n..].iter_mut() {
                *v = 0.0;
            }

            let mut freq = vec![0.0f32; n];
            denormalise_bands(
                mode,
                &x[c * n..c * n + n],
                &mut freq,
                &band_log_e[c * nb_ebands..c * nb_ebands + nb_ebands],
                start,
                eff_end,
                m,
                silence,
            );

            for bb in 0..b {
                let base = out_off + nb * bb;
                mode.mdct
                    .backward(&freq[bb..], b, shift, &mut mem[base..base + out_len]);
            }
            for i in out_off..out_off + n {
                mem[i] = saturate(mem[i]);
            }
        }

        let pf_pitch = pf.period;
        let pf_gain = pf.gain;
        let pf_tapset = pf.tapset;
        for c in 0..channels {
            let mem = &mut self.decode_mem[c];
            let p_old = self.pf_period_old.max(COMBFILTER_MINPERIOD);
            let p_cur = self.pf_period.max(COMBFILTER_MINPERIOD);
            comb_filter(
                mem,
                out_off,
                out_off,
                p_old,
                p_cur,
                mode.short_mdct,
                self.pf_gain_old,
                self.pf_gain,
                self.pf_tapset_old,
                self.pf_tapset,
                mode.mdct.window(),
                overlap,
            );
            if lm != 0 {
                comb_filter(
                    mem,
                    out_off + mode.short_mdct,
                    out_off + mode.short_mdct,
                    p_cur,
                    pf_pitch.max(COMBFILTER_MINPERIOD),
                    n - mode.short_mdct,
                    self.pf_gain,
                    pf_gain,
                    self.pf_tapset,
                    pf_tapset,
                    mode.mdct.window(),
                    overlap,
                );
            }
        }
        self.pf_period_old = self.pf_period;
        self.pf_gain_old = self.pf_gain;
        self.pf_tapset_old = self.pf_tapset;
        self.pf_period = pf_pitch;
        self.pf_gain = pf_gain;
        self.pf_tapset = pf_tapset;
        if lm != 0 {
            self.pf_period_old = self.pf_period;
            self.pf_gain_old = self.pf_gain;
            self.pf_tapset_old = self.pf_tapset;
        }

        let coef0 = 0.85f32;
        for c in 0..channels {
            let mem = &self.decode_mem[c];
            let mut m0 = self.preemph[c];
            for j in 0..n {
                let tmp = mem[out_off + j] + 1e-30 + m0;
                m0 = coef0 * tmp;
                pcm_out[j * channels + c] = tmp * (1.0 / 32768.0);
            }
            self.preemph[c] = m0;
        }
    }
}


fn saturate(x: f32) -> f32 {
    if x > SIG_SAT {
        SIG_SAT
    } else if x < -SIG_SAT {
        -SIG_SAT
    } else {
        x
    }
}

fn comb_filter(
    buf: &mut [f32],
    y_off: usize,
    x_off: usize,
    t0_in: i32,
    t1_in: i32,
    n: usize,
    g0: f32,
    g1: f32,
    tapset0: i32,
    tapset1: i32,
    window: &[f32],
    overlap_in: usize,
) {
    if g0 == 0.0 && g1 == 0.0 {
        if y_off != x_off {
            for i in 0..n {
                buf[y_off + i] = buf[x_off + i];
            }
        }
        return;
    }
    let t0 = t0_in.max(COMBFILTER_MINPERIOD) as usize;
    let t1 = t1_in.max(COMBFILTER_MINPERIOD) as usize;
    let g00 = g0 * GAINS[tapset0 as usize][0];
    let g01 = g0 * GAINS[tapset0 as usize][1];
    let g02 = g0 * GAINS[tapset0 as usize][2];
    let g10 = g1 * GAINS[tapset1 as usize][0];
    let g11 = g1 * GAINS[tapset1 as usize][1];
    let g12 = g1 * GAINS[tapset1 as usize][2];

    let mut overlap = overlap_in;
    if g0 == g1 && t0 == t1 && tapset0 == tapset1 {
        overlap = 0;
    }

    for i in 0..overlap {
        let f = window[i] * window[i];
        let xi = buf[x_off + i];
        let v = xi
            + (1.0 - f) * g00 * buf[x_off + i - t0]
            + (1.0 - f) * g01 * (buf[x_off + i - t0 + 1] + buf[x_off + i - t0 - 1])
            + (1.0 - f) * g02 * (buf[x_off + i - t0 + 2] + buf[x_off + i - t0 - 2])
            + f * g10 * buf[x_off + i - t1]
            + f * g11 * (buf[x_off + i - t1 + 1] + buf[x_off + i - t1 - 1])
            + f * g12 * (buf[x_off + i - t1 + 2] + buf[x_off + i - t1 - 2]);
        buf[y_off + i] = saturate(v);
    }

    if g1 == 0.0 {
        if y_off != x_off {
            for i in overlap..n {
                buf[y_off + i] = buf[x_off + i];
            }
        }
        return;
    }

    for i in overlap..n {
        let xi = buf[x_off + i];
        let v = xi
            + g10 * buf[x_off + i - t1]
            + g11 * (buf[x_off + i - t1 + 1] + buf[x_off + i - t1 - 1])
            + g12 * (buf[x_off + i - t1 + 2] + buf[x_off + i - t1 - 2]);
        buf[y_off + i] = saturate(v);
    }
}
