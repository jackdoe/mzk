use super::mp4::AlacConfig;

pub struct Alac {
    frame_length: u32,
    bit_depth: u32,
    pb: u32,
    mb: u32,
    kb: i32,
}

struct Br<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Br<'_> {
    fn read1(&mut self) -> u32 {
        let byte = self.pos >> 3;
        let b = if byte < self.d.len() { self.d[byte] } else { 0 };
        let bit = 7 - (self.pos & 7);
        self.pos += 1;
        ((b >> bit) & 1) as u32
    }

    fn read(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.read1();
        }
        v
    }

    fn read_signed(&mut self, n: u32) -> i32 {
        let v = self.read(n);
        let s = 32 - n;
        ((v << s) as i32) >> s
    }

    fn unread(&mut self) {
        self.pos -= 1;
    }
}

fn sign_extend(v: i32, bits: u32) -> i32 {
    let s = 32 - bits;
    (v << s) >> s
}

fn sign_of(v: i32) -> i32 {
    (v > 0) as i32 - (v < 0) as i32
}

fn decode_value(br: &mut Br, sample_size: u32, k: i32) -> u32 {
    let mut x = 0u32;
    while x <= 8 && br.read1() == 1 {
        x += 1;
    }
    if x > 8 {
        return br.read(sample_size);
    }
    if k >= 2 {
        let extra = br.read(k as u32);
        x = x * ((1u32 << k) - 1);
        if extra > 1 {
            x += extra - 1;
        } else {
            br.unread();
        }
    }
    x
}

impl Alac {
    pub fn new(cfg: &AlacConfig) -> Self {
        Alac {
            frame_length: cfg.frame_length,
            bit_depth: cfg.bit_depth as u32,
            pb: cfg.pb as u32,
            mb: cfg.mb as u32,
            kb: cfg.kb as i32,
        }
    }

    pub fn reset(&mut self) {}

    fn rice(&self, br: &mut Br, out: &mut [i32], n: usize, sample_size: u32, hist_mult: u32) {
        let mut history = self.mb;
        let mut sign_modifier = 0u32;
        let mut i = 0usize;
        while i < n {
            let k = (31 - ((history >> 9) + 3).leading_zeros() as i32).min(self.kb);
            let mut x = decode_value(br, sample_size, k.max(0));
            x += sign_modifier;
            sign_modifier = 0;
            out[i] = (x >> 1) as i32 ^ -((x & 1) as i32);
            i += 1;

            history = history
                .wrapping_add(x.wrapping_mul(hist_mult))
                .wrapping_sub((history.wrapping_mul(hist_mult)) >> 9);
            if x > 0xffff {
                history = 0xffff;
            }

            if history < 128 && i < n {
                sign_modifier = 1;
                let kz = (history.leading_zeros() as i32) + (((history + 16) >> 6) as i32) - 24;
                let run = decode_value(br, 16, kz.max(0)) as usize;
                for _ in 0..run {
                    if i < n {
                        out[i] = 0;
                        i += 1;
                    }
                }
                if run > 0xffff {
                    sign_modifier = 0;
                }
                history = 0;
            }
        }
    }

    fn fir(
        &self,
        err: &[i32],
        out: &mut [i32],
        n: usize,
        sample_size: u32,
        coefs: &mut [i32],
        num: usize,
        quant: u32,
    ) {
        out[0] = err[0];
        if num == 0 {
            out[1..n].copy_from_slice(&err[1..n]);
            return;
        }
        if num == 31 {
            for i in 1..n {
                out[i] = sign_extend(out[i - 1] + err[i], sample_size);
            }
            return;
        }
        for i in 1..=num {
            out[i] = sign_extend(out[i - 1] + err[i], sample_size);
        }
        let denhalf = 1i64 << (quant - 1);
        for i in (num + 1)..n {
            let top = out[i - num - 1];
            let mut sum = 0i64;
            for j in 0..num {
                sum += (out[i - 1 - j] - top) as i64 * coefs[j] as i64;
            }
            let mut e = err[i];
            let outval = ((denhalf + sum) >> quant) + top as i64 + e as i64;
            out[i] = sign_extend(outval as i32, sample_size);

            if e > 0 {
                for k in (0..num).rev() {
                    let dd = out[i - 1 - k] - top;
                    let sgn = sign_of(dd);
                    coefs[k] += sgn;
                    e -= (num - k) as i32 * ((sgn * dd) >> quant);
                    if e <= 0 {
                        break;
                    }
                }
            } else if e < 0 {
                for k in (0..num).rev() {
                    let dd = out[i - 1 - k] - top;
                    let sgn = sign_of(dd);
                    coefs[k] -= sgn;
                    e -= (num - k) as i32 * ((-(sgn * dd)) >> quant);
                    if e >= 0 {
                        break;
                    }
                }
            }
        }
    }

    fn read_predictor(&self, br: &mut Br) -> (u32, u32, u32, usize, [i32; 32]) {
        let pred_type = br.read(4);
        let quant = br.read(4);
        let ricemod = br.read(3);
        let num = br.read(5) as usize;
        let mut coefs = [0i32; 32];
        for c in coefs.iter_mut().take(num) {
            *c = br.read_signed(16);
        }
        (pred_type, quant, ricemod, num, coefs)
    }

    pub fn decode_packet(&self, pkt: &[u8]) -> Vec<f32> {
        let mut br = Br { d: pkt, pos: 0 };
        let tag = br.read(3);
        let channels = match tag {
            0 => 1usize,
            1 => 2usize,
            _ => return Vec::new(),
        };

        br.read(4);
        br.read(12);
        let partial = br.read(1);
        let bytes_shifted = br.read(2);
        let escape = br.read(1);
        let shift = bytes_shifted * 8;
        let n = if partial == 1 {
            br.read(32) as usize
        } else {
            self.frame_length as usize
        };

        let scale = 1.0 / (1u64 << (self.bit_depth - 1)) as f32;
        let mut out = vec![0.0f32; n * channels];

        if channels == 2 {
            let read_size = self.bit_depth - shift + 1;
            let mut bufa = vec![0i32; n];
            let mut bufb = vec![0i32; n];
            let mut mix_bits = 0u32;
            let mut mix_res = 0i32;
            let mut shift_a = vec![0i32; n];
            let mut shift_b = vec![0i32; n];

            if escape == 0 {
                mix_bits = br.read(8);
                mix_res = br.read(8) as i8 as i32;
                let (_, qa, rma, na, mut ca) = self.read_predictor(&mut br);
                let (_, qb, rmb, nb, mut cb) = self.read_predictor(&mut br);
                if shift != 0 {
                    for i in 0..n {
                        shift_a[i] = br.read(shift) as i32;
                        shift_b[i] = br.read(shift) as i32;
                    }
                }
                let mut erra = vec![0i32; n];
                let mut errb = vec![0i32; n];
                self.rice(&mut br, &mut erra, n, read_size, rma * self.pb / 4);
                self.fir(&erra, &mut bufa, n, read_size, &mut ca, na, qa);
                self.rice(&mut br, &mut errb, n, read_size, rmb * self.pb / 4);
                self.fir(&errb, &mut bufb, n, read_size, &mut cb, nb, qb);
            } else {
                for i in 0..n {
                    bufa[i] = br.read_signed(self.bit_depth);
                    bufb[i] = br.read_signed(self.bit_depth);
                }
            }

            for i in 0..n {
                let (mut l, mut r);
                if mix_res != 0 {
                    l = bufa[i] + bufb[i] - ((mix_res * bufb[i]) >> mix_bits);
                    r = l - bufb[i];
                } else {
                    l = bufa[i];
                    r = bufb[i];
                }
                if shift != 0 {
                    l = (l << shift) | shift_a[i];
                    r = (r << shift) | shift_b[i];
                }
                out[i * 2] = l as f32 * scale;
                out[i * 2 + 1] = r as f32 * scale;
            }
        } else {
            let read_size = self.bit_depth - shift;
            let mut buf = vec![0i32; n];
            let mut sh = vec![0i32; n];
            if escape == 0 {
                let (_, q, rm, num, mut c) = self.read_predictor(&mut br);
                if shift != 0 {
                    for s in sh.iter_mut().take(n) {
                        *s = br.read(shift) as i32;
                    }
                }
                let mut err = vec![0i32; n];
                self.rice(&mut br, &mut err, n, read_size, rm * self.pb / 4);
                self.fir(&err, &mut buf, n, read_size, &mut c, num, q);
            } else {
                for s in buf.iter_mut().take(n) {
                    *s = br.read_signed(self.bit_depth);
                }
            }
            for i in 0..n {
                let mut v = buf[i];
                if shift != 0 {
                    v = (v << shift) | sh[i];
                }
                out[i] = v as f32 * scale;
            }
        }

        out
    }
}
