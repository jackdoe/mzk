use super::aac_tables::*;
use super::mp4::AacConfig;
use crate::error::{Error, Result};
use crate::fft::Fft;
use std::sync::OnceLock;

const SF_OFFSET: i32 = 100;

struct Br<'a> {
    d: &'a [u8],
    pos: usize,
    limit: usize,
}

impl<'a> Br<'a> {
    fn new(d: &'a [u8]) -> Self {
        Br {
            d,
            pos: 0,
            limit: d.len() * 8,
        }
    }
    fn bit(&mut self) -> u32 {
        let byte = self.pos >> 3;
        let b = if byte < self.d.len() { self.d[byte] } else { 0 };
        let v = ((b >> (7 - (self.pos & 7))) & 1) as u32;
        self.pos += 1;
        v
    }
    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }
    fn left(&self) -> usize {
        self.limit.saturating_sub(self.pos)
    }
    fn peek(&self, n: u32) -> u32 {
        let base = self.pos >> 3;
        let mut acc = 0u64;
        for i in 0..5 {
            let b = self.d.get(base + i).copied().unwrap_or(0) as u64;
            acc = (acc << 8) | b;
        }
        let off = (self.pos & 7) as u32;
        ((acc >> (40 - off - n)) & ((1u64 << n) - 1)) as u32
    }
    fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }
}

const HUFF_ROOT_BITS: u32 = 10;

struct Huff {
    root: Vec<u32>,
    long: Vec<(u32, u8, u16)>,
}

impl Huff {
    fn new(codes: &[u32], bits: &[u8]) -> Self {
        let mut root = vec![0u32; 1 << HUFF_ROOT_BITS];
        let mut long = Vec::new();
        for i in 0..codes.len() {
            let len = bits[i] as u32;
            let code = codes[i];
            let sym = i as u16;
            if len <= HUFF_ROOT_BITS {
                let shift = HUFF_ROOT_BITS - len;
                let entry = ((sym as u32) << 8) | len;
                for s in (code << shift)..((code + 1) << shift) {
                    root[s as usize] = entry;
                }
            } else {
                long.push((code, bits[i], sym));
            }
        }
        Huff { root, long }
    }

    fn decode(&self, br: &mut Br) -> u16 {
        let entry = self.root[br.peek(HUFF_ROOT_BITS) as usize];
        let len = entry & 0xff;
        if len != 0 {
            br.skip(len);
            return (entry >> 8) as u16;
        }
        for &(code, bits, sym) in &self.long {
            if br.peek(bits as u32) == code {
                br.skip(bits as u32);
                return sym;
            }
        }
        0
    }
}

fn spec_tables() -> &'static Vec<Huff> {
    static M: OnceLock<Vec<Huff>> = OnceLock::new();
    M.get_or_init(|| {
        (0..11)
            .map(|i| {
                let codes: Vec<u32> = AAC_SPEC_CODES[i].iter().map(|&c| c as u32).collect();
                Huff::new(&codes, AAC_SPEC_BITS[i])
            })
            .collect()
    })
}

fn scf_table() -> &'static Huff {
    static M: OnceLock<Huff> = OnceLock::new();
    M.get_or_init(|| Huff::new(&AAC_SCF_CODES, &AAC_SCF_BITS))
}

fn pow43() -> &'static Vec<f32> {
    static P: OnceLock<Vec<f32>> = OnceLock::new();
    P.get_or_init(|| (0..8192).map(|i| (i as f32).powf(4.0 / 3.0)).collect())
}

fn iquant(q: i32) -> f32 {
    let a = q.unsigned_abs() as usize;
    let m = if a < 8192 {
        pow43()[a]
    } else {
        (a as f32).powf(4.0 / 3.0)
    };
    if q < 0 {
        -m
    } else {
        m
    }
}

const CB_DIM: [usize; 12] = [0, 4, 4, 4, 4, 2, 2, 2, 2, 2, 2, 2];
const CB_MOD: [i32; 12] = [0, 3, 3, 3, 3, 9, 9, 8, 8, 13, 13, 17];
const CB_OFF: [i32; 12] = [0, 1, 1, 0, 0, 4, 4, 0, 0, 0, 0, 0];
fn cb_unsigned(cb: usize) -> bool {
    matches!(cb, 3 | 4 | 7 | 8 | 9 | 10 | 11)
}

fn escape(br: &mut Br) -> i32 {
    let mut n = 4;
    while br.bit() == 1 {
        n += 1;
    }
    ((1i32 << n) + br.bits(n) as i32) as i32
}

fn decode_tuple(br: &mut Br, cb: usize, out: &mut [i32]) {
    let dim = CB_DIM[cb];
    let m = CB_MOD[cb];
    let off = CB_OFF[cb];
    let idx = spec_tables()[cb - 1].decode(br) as i32;
    if dim == 4 {
        out[0] = idx / (m * m * m) - off;
        out[1] = (idx / (m * m)) % m - off;
        out[2] = (idx / m) % m - off;
        out[3] = idx % m - off;
    } else {
        out[0] = idx / m - off;
        out[1] = idx % m - off;
    }
    if cb_unsigned(cb) {
        for v in out.iter_mut().take(dim) {
            if *v != 0 && br.bit() == 1 {
                *v = -*v;
            }
        }
    }
    if cb == 11 {
        for v in out.iter_mut().take(dim) {
            if v.abs() == 16 {
                let s = v.signum();
                *v = s * escape(br);
            }
        }
    }
}

struct Windows {
    sine_l: Vec<f32>,
    kbd_l: Vec<f32>,
    sine_s: Vec<f32>,
    kbd_s: Vec<f32>,
}

fn i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let mut k = 1.0;
    loop {
        term *= (x / 2.0) * (x / 2.0) / (k * k);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
        k += 1.0;
    }
    sum
}

fn kbd(n: usize, alpha: f64) -> Vec<f32> {
    let m = n / 2;
    let mut wp = vec![0.0f64; m + 1];
    let mut acc = 0.0;
    for i in 0..=m {
        let r = 2.0 * i as f64 / m as f64 - 1.0;
        let x = std::f64::consts::PI * alpha * (1.0 - r * r).max(0.0).sqrt();
        acc += i0(x);
        wp[i] = acc;
    }
    let denom = wp[m];
    let mut w = vec![0.0f32; n];
    for nn in 0..m {
        w[nn] = (wp[nn] / denom).sqrt() as f32;
        w[n - 1 - nn] = w[nn];
    }
    w
}

fn sine(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (std::f64::consts::PI / n as f64 * (i as f64 + 0.5)).sin() as f32)
        .collect()
}

fn windows() -> &'static Windows {
    static W: OnceLock<Windows> = OnceLock::new();
    W.get_or_init(|| Windows {
        sine_l: sine(2048),
        kbd_l: kbd(2048, 4.0),
        sine_s: sine(256),
        kbd_s: kbd(256, 6.0),
    })
}

struct ImdctPlan {
    trig: Vec<f32>,
    fft: Fft,
}

impl ImdctPlan {
    fn new(n: usize) -> Self {
        let l = n / 4;
        let w = std::f64::consts::PI * 2.0 / n as f64;
        let mut trig = vec![0.0f32; 4 * l];
        for r in 0..l {
            let pa = w * r as f64;
            let qa = w * (r as f64 + 0.25);
            trig[4 * r] = pa.cos() as f32;
            trig[4 * r + 1] = pa.sin() as f32;
            trig[4 * r + 2] = qa.cos() as f32;
            trig[4 * r + 3] = qa.sin() as f32;
        }
        ImdctPlan { trig, fft: Fft::new(l) }
    }
}

fn imdct_plan(n: usize) -> &'static ImdctPlan {
    if n == 2048 {
        static P: OnceLock<ImdctPlan> = OnceLock::new();
        P.get_or_init(|| ImdctPlan::new(2048))
    } else {
        static P: OnceLock<ImdctPlan> = OnceLock::new();
        P.get_or_init(|| ImdctPlan::new(256))
    }
}

fn imdct(spec: &[f32], out: &mut [f32]) {
    let n = out.len();
    let m = n / 2;
    let l = n / 4;
    let p = imdct_plan(n);
    let trig = &p.trig;

    let mut fbuf = [(0.0f32, 0.0f32); 512];
    let f = &mut fbuf[..l];
    for r in 0..l {
        let x1 = spec[2 * r];
        let x2 = spec[m - 1 - 2 * r];
        let pc = trig[4 * r];
        let ps = trig[4 * r + 1];
        f[r] = (x1 * pc + x2 * ps, x2 * pc - x1 * ps);
    }
    p.fft.forward(f);

    let mut dbuf = [0.0f32; 1024];
    let d = &mut dbuf[..m];
    for nn in 0..l {
        let ur = f[nn].0;
        let ui = f[nn].1;
        let qc = trig[4 * nn + 2];
        let qs = trig[4 * nn + 3];
        d[2 * nn] = ur * qc + ui * qs;
        d[m - 1 - 2 * nn] = ur * qs - ui * qc;
    }

    let s = 2.0 / n as f32;
    let half = m / 2;
    for nn in 0..half {
        out[nn] = s * d[nn + half];
    }
    for nn in half..3 * half {
        out[nn] = -s * d[3 * half - 1 - nn];
    }
    for nn in 3 * half..n {
        out[nn] = -s * d[nn - 3 * half];
    }
}

struct Ics {
    window_sequence: u32,
    window_shape: u32,
    max_sfb: usize,
    num_groups: usize,
    group_len: [usize; 8],
    num_windows: usize,
    win_size: usize,
    swb: &'static [u16],
    num_swb: usize,
    sfb_cb: Vec<Vec<u8>>,
    scf: Vec<Vec<i32>>,
    tns: Tns,
    coef: Vec<f32>,
}

#[derive(Default)]
struct Tns {
    present: bool,
    n_filt: [usize; 8],
    coef_res: [u32; 8],
    length: [[usize; 4]; 8],
    order: [[usize; 4]; 8],
    direction: [[bool; 4]; 8],
    coefs: [[[f32; 32]; 4]; 8],
}

pub struct Aac {
    srindex: usize,
    channels: usize,
    overlap: Vec<Vec<f32>>,
    prev_shape: Vec<u32>,
}

impl Aac {
    pub fn new(cfg: &AacConfig) -> Result<Self> {
        if cfg.audio_object_type != 2 {
            return Err(Error::Unsupported("m4a: only AAC-LC (object type 2)"));
        }
        if cfg.frame_length != 1024 {
            return Err(Error::Unsupported("m4a: only 1024 frame length"));
        }
        let srindex = AAC_SR_INDEX
            .iter()
            .position(|&r| r == cfg.sample_rate)
            .unwrap_or(4);
        let ch = cfg.channels.max(1);
        Ok(Aac {
            srindex,
            channels: ch,
            overlap: vec![vec![0.0; 1024]; ch],
            prev_shape: vec![0; ch],
        })
    }

    pub fn reset(&mut self) {
        for o in self.overlap.iter_mut() {
            o.iter_mut().for_each(|v| *v = 0.0);
        }
        self.prev_shape.iter_mut().for_each(|v| *v = 0);
    }

    fn read_ics_info(&self, br: &mut Br) -> Ics {
        br.bit(); // ics_reserved
        let window_sequence = br.bits(2);
        let window_shape = br.bit();
        let short = window_sequence == 2;
        let mut group_len = [1usize; 8];
        let mut num_groups = 1;
        let (max_sfb, num_windows, win_size, swb, num_swb);
        if short {
            max_sfb = br.bits(4) as usize;
            let grouping = br.bits(7);
            num_windows = 8;
            win_size = 128;
            swb = AAC_SWB_OFFSET_128[self.srindex];
            num_swb = AAC_NUM_SWB_128[self.srindex] as usize;
            let mut g = 0;
            for i in 0..7 {
                if grouping & (1 << (6 - i)) != 0 {
                    group_len[g] += 1;
                } else {
                    g += 1;
                }
            }
            num_groups = g + 1;
        } else {
            max_sfb = br.bits(6) as usize;
            if br.bit() == 1 {
                // predictor_data_present: AAC-LC shouldn't, but skip ics predictor bits
                // predictor_reset(1) + bits; handled defensively as none
            }
            num_windows = 1;
            win_size = 1024;
            swb = AAC_SWB_OFFSET_1024[self.srindex];
            num_swb = AAC_NUM_SWB_1024[self.srindex] as usize;
        }
        Ics {
            window_sequence,
            window_shape,
            max_sfb,
            num_groups,
            group_len,
            num_windows,
            win_size,
            swb,
            num_swb,
            sfb_cb: Vec::new(),
            scf: Vec::new(),
            tns: Tns::default(),
            coef: vec![0.0; 1024],
        }
    }

    fn read_section_data(&self, br: &mut Br, ics: &mut Ics) {
        let bits = if ics.window_sequence == 2 { 3 } else { 5 };
        let esc = (1u32 << bits) - 1;
        ics.sfb_cb = vec![vec![0u8; ics.max_sfb]; ics.num_groups];
        for g in 0..ics.num_groups {
            let mut k = 0;
            while k < ics.max_sfb {
                let cb = br.bits(4) as u8;
                let mut len = 0u32;
                loop {
                    let inc = br.bits(bits);
                    len += inc;
                    if inc != esc {
                        break;
                    }
                }
                for _ in 0..len {
                    if k < ics.max_sfb {
                        ics.sfb_cb[g][k] = cb;
                        k += 1;
                    }
                }
            }
        }
    }

    fn read_scalefactors(&self, br: &mut Br, ics: &mut Ics, global_gain: i32) {
        let map = scf_table();
        ics.scf = vec![vec![0i32; ics.max_sfb]; ics.num_groups];
        let mut scf = global_gain;
        let mut is_pos = 0i32;
        let mut noise = global_gain - 90 - 256;
        let mut noise_first = true;
        for g in 0..ics.num_groups {
            for sfb in 0..ics.max_sfb {
                let cb = ics.sfb_cb[g][sfb];
                ics.scf[g][sfb] = match cb {
                    0 => 0,
                    14 | 15 => {
                        is_pos += map.decode(br) as i32 - 60;
                        is_pos
                    }
                    13 => {
                        if noise_first {
                            noise_first = false;
                            noise += br.bits(9) as i32 - 256;
                        } else {
                            noise += map.decode(br) as i32 - 60;
                        }
                        noise
                    }
                    _ => {
                        scf += map.decode(br) as i32 - 60;
                        scf
                    }
                };
            }
        }
    }

    fn read_tns(&self, br: &mut Br, ics: &mut Ics) {
        let mut t = Tns {
            present: true,
            ..Default::default()
        };
        let (lbits, obits) = if ics.window_sequence == 2 {
            (1u32, 3u32)
        } else {
            (6, 5)
        };
        let sbits = if ics.window_sequence == 2 { 4 } else { 6 };
        for w in 0..ics.num_windows {
            t.n_filt[w] = br.bits(if ics.window_sequence == 2 { 1 } else { 2 }) as usize;
            let _ = lbits;
            let _ = obits;
            if t.n_filt[w] > 0 {
                t.coef_res[w] = br.bit();
                for f in 0..t.n_filt[w] {
                    t.length[w][f] = br.bits(sbits) as usize;
                    let order = br.bits(if ics.window_sequence == 2 { 3 } else { 5 }) as usize;
                    t.order[w][f] = order;
                    if order > 0 {
                        t.direction[w][f] = br.bit() == 1;
                        let compress = br.bit();
                        let cbits = 3 + t.coef_res[w] - compress;
                        let sign_off = 1i32 << (cbits - 1);
                        for i in 0..order {
                            let raw = br.bits(cbits) as i32;
                            let c = if raw >= sign_off { raw - 2 * sign_off } else { raw };
                            t.coefs[w][f][i] = c as f32;
                        }
                    }
                }
            }
        }
        ics.tns = t;
    }

    fn read_spectral(&self, br: &mut Br, ics: &mut Ics) {
        let mut win = 0;
        for g in 0..ics.num_groups {
            let glen = ics.group_len[g];
            for sfb in 0..ics.max_sfb {
                let cb = ics.sfb_cb[g][sfb] as usize;
                let lo = ics.swb[sfb] as usize;
                let hi = ics.swb[sfb + 1] as usize;
                let width = hi - lo;
                if cb == 0 || cb == 13 || cb == 14 || cb == 15 {
                    continue;
                }
                let dim = CB_DIM[cb];
                for w in 0..glen {
                    let base = (win + w) * ics.win_size + lo;
                    let mut tup = [0i32; 4];
                    let mut k = 0;
                    while k < width {
                        decode_tuple(br, cb, &mut tup);
                        for d in 0..dim {
                            ics.coef[base + k + d] = tup[d] as f32;
                        }
                        k += dim;
                    }
                }
            }
            win += glen;
        }
    }

    fn dequant(&self, ics: &mut Ics) {
        let mut win = 0;
        for g in 0..ics.num_groups {
            let glen = ics.group_len[g];
            for sfb in 0..ics.max_sfb {
                let cb = ics.sfb_cb[g][sfb];
                if cb == 0 || cb >= 13 {
                    continue;
                }
                let gain = 2.0f32.powf(0.25 * (ics.scf[g][sfb] - SF_OFFSET) as f32);
                let lo = ics.swb[sfb] as usize;
                let hi = ics.swb[sfb + 1] as usize;
                for w in 0..glen {
                    let base = (win + w) * ics.win_size;
                    for k in lo..hi {
                        let v = ics.coef[base + k];
                        ics.coef[base + k] = iquant(v as i32) * gain;
                    }
                }
            }
            win += glen;
        }
    }

    fn apply_tns(&self, ics: &mut Ics) {
        if !ics.tns.present {
            return;
        }
        let max_band = if ics.window_sequence == 2 {
            AAC_TNS_MAX_128[self.srindex]
        } else {
            AAC_TNS_MAX_1024[self.srindex]
        } as usize;
        for w in 0..ics.num_windows {
            let mut start_sfb = 0;
            for f in 0..ics.tns.n_filt[w] {
                let order = ics.tns.order[w][f];
                let end_sfb = (start_sfb + ics.tns.length[w][f]).min(ics.num_swb.min(max_band));
                if order == 0 {
                    start_sfb = end_sfb;
                    continue;
                }
                let lpc = tns_lpc(&ics.tns.coefs[w][f], order, ics.tns.coef_res[w]);
                let bottom = ics.swb[start_sfb.min(ics.num_swb)] as usize;
                let top = ics.swb[end_sfb.min(ics.num_swb)] as usize;
                let base = w * ics.win_size;
                let size = top - bottom;
                if size == 0 {
                    start_sfb = end_sfb;
                    continue;
                }
                let (mut idx, inc): (i64, i64) = if ics.tns.direction[w][f] {
                    ((top - 1) as i64, -1)
                } else {
                    (bottom as i64, 1)
                };
                for _ in 0..size {
                    let mut y = ics.coef[base + idx as usize];
                    for o in 1..=order {
                        let j = idx - inc * o as i64;
                        if (bottom as i64..top as i64).contains(&j) {
                            y -= lpc[o - 1] * ics.coef[base + j as usize];
                        }
                    }
                    ics.coef[base + idx as usize] = y;
                    idx += inc;
                }
                start_sfb = end_sfb;
            }
        }
    }

    fn filterbank(&mut self, ch: usize, ics: &Ics, out: &mut [f32]) {
        let w = windows();
        let prev_shape = self.prev_shape[ch];
        let cur_shape = ics.window_shape;
        let mut buf = vec![0.0f32; 2048];

        if ics.window_sequence == 2 {
            let mut z = [0.0f32; 256];
            let swin = if cur_shape == 1 { &w.kbd_s } else { &w.sine_s };
            for win in 0..8 {
                let spec = &ics.coef[win * 128..win * 128 + 128];
                imdct(spec, &mut z);
                let off = 448 + win * 128;
                for n in 0..256 {
                    buf[off + n] += z[n] * swin[n];
                }
            }
        } else {
            let mut z = vec![0.0f32; 2048];
            imdct(&ics.coef[..1024], &mut z);
            let left = if prev_shape == 1 { &w.kbd_l } else { &w.sine_l };
            let right = if cur_shape == 1 { &w.kbd_l } else { &w.sine_l };
            let lshort = if prev_shape == 1 { &w.kbd_s } else { &w.sine_s };
            let rshort = if cur_shape == 1 { &w.kbd_s } else { &w.sine_s };
            match ics.window_sequence {
                0 => {
                    for n in 0..1024 {
                        buf[n] = z[n] * left[n];
                    }
                    for n in 1024..2048 {
                        buf[n] = z[n] * right[n];
                    }
                }
                1 => {
                    for n in 0..1024 {
                        buf[n] = z[n] * left[n];
                    }
                    for n in 1024..1472 {
                        buf[n] = z[n];
                    }
                    for n in 1472..1600 {
                        buf[n] = z[n] * rshort[n - 1472 + 128];
                    }
                }
                3 => {
                    for n in 448..576 {
                        buf[n] = z[n] * lshort[n - 448];
                    }
                    for n in 576..1024 {
                        buf[n] = z[n];
                    }
                    for n in 1024..2048 {
                        buf[n] = z[n] * right[n];
                    }
                }
                _ => {}
            }
        }

        for n in 0..1024 {
            out[n] = (buf[n] + self.overlap[ch][n]) * (1.0 / 32768.0);
            self.overlap[ch][n] = buf[1024 + n];
        }
        self.prev_shape[ch] = cur_shape;
    }

    fn decode_ics(&self, br: &mut Br, common: Option<&Ics>) -> Ics {
        let global_gain = br.bits(8) as i32;
        let mut ics = match common {
            Some(c) => Ics {
                window_sequence: c.window_sequence,
                window_shape: c.window_shape,
                max_sfb: c.max_sfb,
                num_groups: c.num_groups,
                group_len: c.group_len,
                num_windows: c.num_windows,
                win_size: c.win_size,
                swb: c.swb,
                num_swb: c.num_swb,
                sfb_cb: Vec::new(),
                scf: Vec::new(),
                tns: Tns::default(),
                coef: vec![0.0; 1024],
            },
            None => self.read_ics_info(br),
        };
        self.read_section_data(br, &mut ics);
        self.read_scalefactors(br, &mut ics, global_gain);
        if br.bit() == 1 {
            self.read_pulse(br);
        }
        if br.bit() == 1 {
            self.read_tns(br, &mut ics);
        }
        if br.bit() == 1 {
            // gain_control_data_present: not valid for LC
        }
        self.read_spectral(br, &mut ics);
        self.dequant(&mut ics);
        self.apply_tns(&mut ics);
        ics
    }

    fn read_pulse(&self, br: &mut Br) {
        let n = br.bits(2);
        br.bits(6); // pulse_start_sfb
        for _ in 0..=n {
            br.bits(5);
            br.bits(4);
        }
    }

    pub fn decode_packet(&mut self, pkt: &[u8]) -> Vec<f32> {
        let mut br = Br::new(pkt);
        let ch = self.channels;
        let mut out = vec![0.0f32; 1024 * ch];

        while br.left() >= 3 {
            let id = br.bits(3);
            match id {
                0 | 3 => {
                    // SCE / LFE
                    br.bits(4); // instance tag
                    let ics = self.decode_ics(&mut br, None);
                    let mut chan = vec![0.0f32; 1024];
                    self.filterbank(0, &ics, &mut chan);
                    for n in 0..1024 {
                        out[n * ch] = chan[n];
                    }
                }
                1 => {
                    // CPE
                    br.bits(4);
                    let common = br.bit() == 1;
                    let mut shared = None;
                    let mut ms = (0u32, Vec::new());
                    if common {
                        let info = self.read_ics_info(&mut br);
                        let ms_mask = br.bits(2);
                        let mut ms_used = Vec::new();
                        if ms_mask == 1 {
                            for _ in 0..info.num_groups * info.max_sfb {
                                ms_used.push(br.bit() == 1);
                            }
                        }
                        ms = (ms_mask, ms_used);
                        shared = Some(info);
                    }
                    let mut left = self.decode_ics(&mut br, shared.as_ref());
                    let mut right = self.decode_ics(&mut br, shared.as_ref());
                    apply_ms(&mut left, &mut right, ms.0, &ms.1);
                    apply_is(&left, &mut right, ms.0, &ms.1);
                    if ch >= 2 {
                        let mut l = vec![0.0f32; 1024];
                        let mut r = vec![0.0f32; 1024];
                        self.filterbank(0, &left, &mut l);
                        self.filterbank(1, &right, &mut r);
                        for n in 0..1024 {
                            out[n * ch] = l[n];
                            out[n * ch + 1] = r[n];
                        }
                    }
                }
                4 => {
                    // DSE
                    br.bits(4);
                    let align = br.bit();
                    let mut cnt = br.bits(8);
                    if cnt == 255 {
                        cnt += br.bits(8);
                    }
                    if align == 1 {
                        br.pos = (br.pos + 7) & !7;
                    }
                    for _ in 0..cnt {
                        br.bits(8);
                    }
                }
                6 => {
                    // FIL
                    let mut cnt = br.bits(4);
                    if cnt == 15 {
                        cnt += br.bits(8) - 1;
                    }
                    for _ in 0..cnt {
                        br.bits(8);
                    }
                }
                7 => break, // END
                _ => break,
            }
        }
        out
    }
}

fn apply_ms(left: &mut Ics, right: &mut Ics, ms_mask: u32, ms_used: &[bool]) {
    if ms_mask == 0 {
        return;
    }
    let mut win = 0;
    let mut idx = 0;
    for g in 0..left.num_groups {
        let glen = left.group_len[g];
        for sfb in 0..left.max_sfb {
            let on = ms_mask == 2 || (ms_mask == 1 && ms_used.get(idx).copied().unwrap_or(false));
            idx += 1;
            let cbr = right.sfb_cb[g][sfb];
            if on && cbr != 0 && cbr < 13 {
                let lo = left.swb[sfb] as usize;
                let hi = left.swb[sfb + 1] as usize;
                for w in 0..glen {
                    let base = (win + w) * left.win_size;
                    for k in lo..hi {
                        let l = left.coef[base + k];
                        let r = right.coef[base + k];
                        left.coef[base + k] = l + r;
                        right.coef[base + k] = l - r;
                    }
                }
            }
        }
        win += glen;
    }
}

fn apply_is(left: &Ics, right: &mut Ics, ms_mask: u32, ms_used: &[bool]) {
    let mut win = 0;
    let mut idx = 0;
    for g in 0..right.num_groups {
        let glen = right.group_len[g];
        for sfb in 0..right.max_sfb {
            let cb = right.sfb_cb[g][sfb];
            let ms_on =
                ms_mask == 1 && ms_used.get(idx).copied().unwrap_or(false);
            idx += 1;
            if cb == 14 || cb == 15 {
                let scale = 2.0f32.powf(-0.25 * right.scf[g][sfb] as f32);
                let mut sign = if cb == 15 { -1.0 } else { 1.0 };
                if ms_on {
                    sign = -sign;
                }
                let lo = right.swb[sfb] as usize;
                let hi = right.swb[sfb + 1] as usize;
                for w in 0..glen {
                    let base = (win + w) * right.win_size;
                    for k in lo..hi {
                        right.coef[base + k] = sign * scale * left.coef[base + k];
                    }
                }
            }
        }
        win += glen;
    }
}

fn tns_lpc(quant: &[f32; 32], order: usize, coef_res: u32) -> Vec<f32> {
    let sign_bits = coef_res + 3;
    let _ = sign_bits;
    let step = std::f32::consts::PI / if coef_res == 1 { 8.0 } else { 4.0 };
    let mut parcor = vec![0.0f32; order];
    for i in 0..order {
        parcor[i] = (step * quant[i]).sin();
    }
    let mut lpc = vec![0.0f32; order];
    let mut tmp = vec![0.0f32; order];
    for m in 0..order {
        let k = parcor[m];
        tmp[m] = k;
        for i in 0..m {
            tmp[i] = lpc[i] + k * lpc[m - 1 - i];
        }
        for i in 0..=m {
            lpc[i] = tmp[i];
        }
    }
    lpc
}

const AAC_SR_INDEX: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn imdct_naive(spec: &[f32], out: &mut [f32]) {
        let n = out.len();
        let n2 = n / 2;
        let n0 = (n2 + 1) as f64 / 2.0;
        let scale = 2.0 / n as f64;
        for nn in 0..n {
            let mut s = 0.0f64;
            for k in 0..n2 {
                let a = std::f64::consts::PI * 2.0 / n as f64 * (nn as f64 + n0) * (k as f64 + 0.5);
                s += spec[k] as f64 * scale * a.cos();
            }
            out[nn] = s as f32;
        }
    }

    fn check(n: usize) {
        let m = n / 2;
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut spec = vec![0.0f32; m];
        for v in spec.iter_mut() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *v = (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0;
        }
        let mut a = vec![0.0f32; n];
        let mut b = vec![0.0f32; n];
        imdct_naive(&spec, &mut a);
        imdct(&spec, &mut b);
        let maxd = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        let peak = a.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(maxd < 1e-3 * peak.max(1.0), "n={n} maxd={maxd} peak={peak}");
    }

    #[test]
    fn fast_imdct_matches_naive() {
        check(2048);
        check(256);
    }
}
