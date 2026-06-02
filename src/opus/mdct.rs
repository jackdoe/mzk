use crate::fft::Fft;

pub struct Mdct {
    n: usize,
    overlap: usize,
    window: Vec<f32>,
    trig: Vec<Vec<f32>>,
    ffts: Vec<Fft>,
}

impl Mdct {
    pub fn new(n: usize, maxshift: usize, overlap: usize) -> Self {
        let mut window = vec![0.0f32; overlap];
        for i in 0..overlap {
            let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / overlap as f64).sin();
            window[i] = (0.5 * std::f64::consts::PI * s * s).sin() as f32;
        }
        let mut trig = Vec::with_capacity(maxshift + 1);
        let mut ffts = Vec::with_capacity(maxshift + 1);
        let mut nn = n;
        let mut n2 = n >> 1;
        for _ in 0..=maxshift {
            let mut t = vec![0.0f32; n2];
            for i in 0..n2 {
                t[i] = (2.0 * std::f64::consts::PI * (i as f64 + 0.125) / nn as f64).cos() as f32;
            }
            trig.push(t);
            ffts.push(Fft::new(nn >> 2));
            n2 >>= 1;
            nn >>= 1;
        }
        Mdct {
            n,
            overlap,
            window,
            trig,
            ffts,
        }
    }

    pub fn window(&self) -> &[f32] {
        &self.window
    }

    pub fn out_len(&self, shift: usize) -> usize {
        let n = self.n >> shift;
        (n >> 1) + self.overlap / 2
    }

    pub fn backward(&self, input: &[f32], stride: usize, shift: usize, out: &mut [f32]) {
        let n = self.n >> shift;
        let n2 = n >> 1;
        let n4 = n >> 2;
        let trig = &self.trig[shift];
        let ov2 = self.overlap / 2;

        let mut f_buf = [(0.0f32, 0.0f32); 480];
        let f = &mut f_buf[..n4];
        for i in 0..n4 {
            let x1 = input[stride * (2 * i)];
            let x2 = input[stride * (n2 - 1 - 2 * i)];
            let ti = trig[i];
            let tn4 = trig[n4 + i];
            let yr = x2 * ti + x1 * tn4;
            let yi = x1 * ti - x2 * tn4;
            f[i] = (yi, yr);
        }
        self.ffts[shift].forward(f);

        let half = (n4 + 1) >> 1;
        for i in 0..half {
            let lo = f[i];
            let re = lo.1;
            let im = lo.0;
            let t0 = trig[i];
            let t1 = trig[n4 + i];
            let yr = re * t0 + im * t1;
            let yi = re * t1 - im * t0;

            let hi = f[n4 - 1 - i];
            let re2 = hi.1;
            let im2 = hi.0;
            let t0b = trig[n4 - i - 1];
            let t1b = trig[n2 - i - 1];
            let yr2 = re2 * t0b + im2 * t1b;
            let yi2 = re2 * t1b - im2 * t0b;

            out[ov2 + 2 * i] = yr;
            out[ov2 + (n2 - 1 - 2 * i)] = yi;
            out[ov2 + (n2 - 2 - 2 * i)] = yr2;
            out[ov2 + (2 * i + 1)] = yi2;
        }

        let ov = self.overlap;
        for i in 0..ov / 2 {
            let x1 = out[ov - 1 - i];
            let x2 = out[i];
            let wp1 = self.window[i];
            let wp2 = self.window[ov - 1 - i];
            out[i] = x2 * wp2 - x1 * wp1;
            out[ov - 1 - i] = x2 * wp1 + x1 * wp2;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_power_complementary() {
        let m = Mdct::new(1920, 3, 120);
        let ov = 120;
        for i in 0..ov {
            let a = m.window()[i];
            let b = m.window()[ov - 1 - i];
            assert!((a * a + b * b - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn inverse_runs_and_is_finite() {
        let m = Mdct::new(1920, 3, 120);
        let freq = vec![0.5f32; 960];
        let mut out = vec![0.0f32; m.out_len(0)];
        m.backward(&freq, 1, 0, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn short_block_runs() {
        let m = Mdct::new(1920, 3, 120);
        let freq = vec![0.1f32; 960];
        let mut out = vec![0.0f32; m.out_len(3)];
        m.backward(&freq, 8, 3, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
