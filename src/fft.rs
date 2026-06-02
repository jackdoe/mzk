type C = (f32, f32);

const MAX_N: usize = 512;

fn smallest_factor(n: usize) -> usize {
    for &p in &[2usize, 3, 5] {
        if n % p == 0 {
            return p;
        }
    }
    n
}

pub struct Fft {
    n: usize,
    tw: Vec<C>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n <= MAX_N);
        let mut tw = Vec::with_capacity(n);
        for k in 0..n {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            tw.push((a.cos() as f32, a.sin() as f32));
        }
        Fft { n, tw }
    }

    fn rec(&self, x: &[C], out: &mut [C], n: usize, stride: usize) {
        if n == 1 {
            out[0] = x[0];
            return;
        }
        let r = smallest_factor(n);
        let m = n / r;
        for j in 0..r {
            self.rec(&x[j * stride..], &mut out[j * m..j * m + m], m, stride * r);
        }
        let scale = self.n / n;
        let mut t = [(0.0f32, 0.0f32); 5];
        for k in 0..m {
            for (j, slot) in t[..r].iter_mut().enumerate() {
                *slot = out[j * m + k];
            }
            for q in 0..r {
                let kq = k + m * q;
                let mut sre = 0.0f32;
                let mut sim = 0.0f32;
                for j in 0..r {
                    let w = self.tw[(j * kq) % n * scale];
                    let a = t[j];
                    sre += a.0 * w.0 - a.1 * w.1;
                    sim += a.0 * w.1 + a.1 * w.0;
                }
                out[kq] = (sre, sim);
            }
        }
    }

    pub fn forward(&self, data: &mut [(f32, f32)]) {
        let n = self.n;
        let mut scratch = [(0.0f32, 0.0f32); MAX_N];
        let inp = &mut scratch[..n];
        inp.copy_from_slice(&data[..n]);
        self.rec(inp, &mut data[..n], n, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(x: &[(f32, f32)], inv: bool) -> Vec<(f32, f32)> {
        let n = x.len();
        let s = if inv { 1.0 } else { -1.0 };
        (0..n)
            .map(|k| {
                let mut re = 0.0f64;
                let mut im = 0.0f64;
                for (j, &(xr, xi)) in x.iter().enumerate() {
                    let a = s * 2.0 * std::f64::consts::PI * (k * j) as f64 / n as f64;
                    let (sa, ca) = a.sin_cos();
                    re += xr as f64 * ca - xi as f64 * sa;
                    im += xr as f64 * sa + xi as f64 * ca;
                }
                (re as f32, im as f32)
            })
            .collect()
    }

    #[test]
    fn matches_naive_dft() {
        for &n in &[30usize, 60, 120, 240] {
            let x: Vec<(f32, f32)> = (0..n)
                .map(|i| ((i as f32 * 0.3).sin(), (i as f32 * 0.17).cos()))
                .collect();
            let plan = Fft::new(n);
            let mut y = x.clone();
            plan.forward(&mut y);
            let r = naive(&x, false);
            for (a, b) in y.iter().zip(r.iter()) {
                assert!(
                    (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2,
                    "n={n}"
                );
            }
        }
    }

}
