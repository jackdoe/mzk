type C = (f64, f64);

fn cmul(a: C, b: C) -> C {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

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
        let mut tw = Vec::with_capacity(n);
        for k in 0..n {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            tw.push((a.cos(), a.sin()));
        }
        Fft { n, tw }
    }

    fn rec(&self, x: &[C], n: usize) -> Vec<C> {
        if n == 1 {
            return vec![x[0]];
        }
        let r = smallest_factor(n);
        let m = n / r;
        let mut fsubs: Vec<Vec<C>> = Vec::with_capacity(r);
        for j in 0..r {
            let sub: Vec<C> = (0..m).map(|i| x[j + i * r]).collect();
            fsubs.push(self.rec(&sub, m));
        }
        let scale = self.n / n;
        let mut out = vec![(0.0, 0.0); n];
        for k in 0..m {
            for q in 0..r {
                let kq = k + m * q;
                let mut sum = (0.0, 0.0);
                for j in 0..r {
                    let e = (j * kq) % n;
                    let w = self.tw[e * scale % self.n];
                    let t = cmul(fsubs[j][k], w);
                    sum.0 += t.0;
                    sum.1 += t.1;
                }
                out[kq] = sum;
            }
        }
        out
    }

    pub fn forward(&self, data: &mut [(f32, f32)]) {
        let x: Vec<C> = data.iter().map(|&(r, i)| (r as f64, i as f64)).collect();
        let y = self.rec(&x, self.n);
        for (d, s) in data.iter_mut().zip(y.iter()) {
            *d = (s.0 as f32, s.1 as f32);
        }
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
