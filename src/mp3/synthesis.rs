use super::tables::SYNTH_WINDOW;

const SEC: [f32; 24] = [
    10.19000816, 0.50060302, 0.50241929, 3.40760851, 0.50547093, 0.52249861, 2.05778098,
    0.51544732, 0.56694406, 1.48416460, 0.53104258, 0.64682180, 1.16943991, 0.55310392,
    0.78815460, 0.97256821, 0.58293498, 1.06067765, 0.83934963, 0.62250412, 1.72244716,
    0.74453628, 0.67480832, 5.10114861,
];

fn scale_pcm(s: f32) -> f32 {
    s * (1.0 / 32768.0)
}

fn dct_ii(grbuf: &mut [f32], base: usize, n: usize) {
    for k in 0..n {
        let yb = base + k;
        let mut t = [0.0f32; 32];
        for i in 0..8 {
            let x0 = grbuf[yb + i * 18];
            let x1 = grbuf[yb + (15 - i) * 18];
            let x2 = grbuf[yb + (16 + i) * 18];
            let x3 = grbuf[yb + (31 - i) * 18];
            let t0 = x0 + x3;
            let t1 = x1 + x2;
            let t2 = (x1 - x2) * SEC[3 * i];
            let t3 = (x0 - x3) * SEC[3 * i + 1];
            t[i] = t0 + t1;
            t[8 + i] = (t0 - t1) * SEC[3 * i + 2];
            t[16 + i] = t3 + t2;
            t[24 + i] = (t3 - t2) * SEC[3 * i + 2];
        }
        for r in 0..4 {
            let o = r * 8;
            let (x0i, x1i, x2i, x3i, x4i, x5i, x6i, x7i) = (
                t[o], t[o + 1], t[o + 2], t[o + 3], t[o + 4], t[o + 5], t[o + 6], t[o + 7],
            );
            let mut x0 = x0i;
            let mut x1 = x1i;
            let mut x2 = x2i;
            let mut x3 = x3i;
            let x4;
            let mut x5;
            let mut x6;
            let mut x7;
            let xt;
            xt = x0 - x7i;
            x0 += x7i;
            x7 = x1 - x6i;
            x1 += x6i;
            x6 = x2 - x5i;
            x2 += x5i;
            x5 = x3 - x4i;
            x3 += x4i;
            x4 = x0 - x3;
            x0 += x3;
            x3 = x1 - x2;
            x1 += x2;
            t[o] = x0 + x1;
            t[o + 4] = (x0 - x1) * 0.70710677;
            x5 += x6;
            x6 = (x6 + x7) * 0.70710677;
            x7 += xt;
            x3 = (x3 + x4) * 0.70710677;
            x5 -= x7 * 0.198912367;
            x7 += x5 * 0.382683432;
            x5 -= x7 * 0.198912367;
            let x0b = xt - x6;
            let xtb = xt + x6;
            t[o + 1] = (xtb + x7) * 0.50979561;
            t[o + 2] = (x4 + x3) * 0.54119611;
            t[o + 3] = (x0b - x5) * 0.60134488;
            t[o + 5] = (x0b + x5) * 0.89997619;
            t[o + 6] = (x4 - x3) * 1.30656302;
            t[o + 7] = (xtb - x7) * 2.56291556;
        }
        let mut y = yb;
        for i in 0..7 {
            grbuf[y] = t[i];
            grbuf[y + 18] = t[16 + i] + t[24 + i] + t[24 + i + 1];
            grbuf[y + 36] = t[8 + i] + t[8 + i + 1];
            grbuf[y + 54] = t[16 + i + 1] + t[24 + i] + t[24 + i + 1];
            y += 4 * 18;
        }
        grbuf[y] = t[7];
        grbuf[y + 18] = t[16 + 7] + t[24 + 7];
        grbuf[y + 36] = t[8 + 7];
        grbuf[y + 54] = t[24 + 7];
    }
}

fn synth_pair(pcm: &mut [f32], pb: usize, nch: usize, z: &[f32], zb: usize) {
    let mut a = (z[zb + 14 * 64] - z[zb]) * 29.0;
    a += (z[zb + 64] + z[zb + 13 * 64]) * 213.0;
    a += (z[zb + 12 * 64] - z[zb + 2 * 64]) * 459.0;
    a += (z[zb + 3 * 64] + z[zb + 11 * 64]) * 2037.0;
    a += (z[zb + 10 * 64] - z[zb + 4 * 64]) * 5153.0;
    a += (z[zb + 5 * 64] + z[zb + 9 * 64]) * 6574.0;
    a += (z[zb + 8 * 64] - z[zb + 6 * 64]) * 37489.0;
    a += z[zb + 7 * 64] * 75038.0;
    pcm[pb] = scale_pcm(a);

    let z2 = zb + 2;
    let mut a = z[z2 + 14 * 64] * 104.0;
    a += z[z2 + 12 * 64] * 1567.0;
    a += z[z2 + 10 * 64] * 9727.0;
    a += z[z2 + 8 * 64] * 64019.0;
    a += z[z2 + 6 * 64] * -9975.0;
    a += z[z2 + 4 * 64] * -45.0;
    a += z[z2 + 2 * 64] * 146.0;
    a += z[z2] * -5.0;
    pcm[pb + 16 * nch] = scale_pcm(a);
}

fn synth(
    grbuf: &[f32],
    xl: usize,
    nch: usize,
    pcm: &mut [f32],
    pb: usize,
    lins: &mut [f32],
    lb: usize,
) {
    let xr = xl + 576 * (nch - 1);
    let dl = pb;
    let dr = pb + (nch - 1);
    let zb = lb + 15 * 64;

    lins[zb + 4 * 15] = grbuf[xl + 18 * 16];
    lins[zb + 4 * 15 + 1] = grbuf[xr + 18 * 16];
    lins[zb + 4 * 15 + 2] = grbuf[xl];
    lins[zb + 4 * 15 + 3] = grbuf[xr];

    lins[zb + 4 * 31] = grbuf[xl + 1 + 18 * 16];
    lins[zb + 4 * 31 + 1] = grbuf[xr + 1 + 18 * 16];
    lins[zb + 4 * 31 + 2] = grbuf[xl + 1];
    lins[zb + 4 * 31 + 3] = grbuf[xr + 1];

    synth_pair(pcm, dr, nch, lins, lb + 4 * 15 + 1);
    synth_pair(pcm, dr + 32 * nch, nch, lins, lb + 4 * 15 + 64 + 1);
    synth_pair(pcm, dl, nch, lins, lb + 4 * 15);
    synth_pair(pcm, dl + 32 * nch, nch, lins, lb + 4 * 15 + 64);

    let mut wi = 0usize;
    for i in (0..15).rev() {
        let mut a = [0.0f32; 4];
        let mut b = [0.0f32; 4];

        lins[zb + 4 * i] = grbuf[xl + 18 * (31 - i)];
        lins[zb + 4 * i + 1] = grbuf[xr + 18 * (31 - i)];
        lins[zb + 4 * i + 2] = grbuf[xl + 1 + 18 * (31 - i)];
        lins[zb + 4 * i + 3] = grbuf[xr + 1 + 18 * (31 - i)];
        lins[zb + 4 * (i + 16)] = grbuf[xl + 1 + 18 * (1 + i)];
        lins[zb + 4 * (i + 16) + 1] = grbuf[xr + 1 + 18 * (1 + i)];
        lins[(zb as i32 + 4 * (i as i32 - 16) + 2) as usize] = grbuf[xl + 18 * (1 + i)];
        lins[(zb as i32 + 4 * (i as i32 - 16) + 3) as usize] = grbuf[xr + 18 * (1 + i)];

        let mut stage = |k: usize, mode: u8, a: &mut [f32; 4], b: &mut [f32; 4]| {
            let w0 = SYNTH_WINDOW[wi];
            let w1 = SYNTH_WINDOW[wi + 1];
            wi += 2;
            let vz = (zb as i32 + 4 * i as i32 - (k as i32) * 64) as usize;
            let vy = (zb as i32 + 4 * i as i32 - ((15 - k) as i32) * 64) as usize;
            for j in 0..4 {
                let pz = lins[vz + j];
                let py = lins[vy + j];
                match mode {
                    0 => {
                        b[j] = pz * w1 + py * w0;
                        a[j] = pz * w0 - py * w1;
                    }
                    1 => {
                        b[j] += pz * w1 + py * w0;
                        a[j] += pz * w0 - py * w1;
                    }
                    _ => {
                        b[j] += pz * w1 + py * w0;
                        a[j] += py * w1 - pz * w0;
                    }
                }
            }
        };

        stage(0, 0, &mut a, &mut b);
        stage(1, 2, &mut a, &mut b);
        stage(2, 1, &mut a, &mut b);
        stage(3, 2, &mut a, &mut b);
        stage(4, 1, &mut a, &mut b);
        stage(5, 2, &mut a, &mut b);
        stage(6, 1, &mut a, &mut b);
        stage(7, 2, &mut a, &mut b);

        pcm[dr + (15 - i) * nch] = scale_pcm(a[1]);
        pcm[dr + (17 + i) * nch] = scale_pcm(b[1]);
        pcm[dl + (15 - i) * nch] = scale_pcm(a[0]);
        pcm[dl + (17 + i) * nch] = scale_pcm(b[0]);
        pcm[dr + (47 - i) * nch] = scale_pcm(a[3]);
        pcm[dr + (49 + i) * nch] = scale_pcm(b[3]);
        pcm[dl + (47 - i) * nch] = scale_pcm(a[2]);
        pcm[dl + (49 + i) * nch] = scale_pcm(b[2]);
    }
}

pub fn synth_granule(
    qmf_state: &mut [f32],
    grbuf: &mut [f32],
    nbands: usize,
    nch: usize,
    pcm: &mut [f32],
    lins: &mut [f32],
) {
    for i in 0..nch {
        dct_ii(grbuf, 576 * i, nbands);
    }
    lins[..15 * 64].copy_from_slice(&qmf_state[..15 * 64]);
    let mut i = 0usize;
    while i < nbands {
        synth(grbuf, i, nch, pcm, 32 * nch * i, lins, i * 64);
        i += 2;
    }
    if nch == 1 {
        let mut j = 0usize;
        while j < 15 * 64 {
            qmf_state[j] = lins[nbands * 64 + j];
            j += 2;
        }
    } else {
        qmf_state[..15 * 64].copy_from_slice(&lins[nbands * 64..nbands * 64 + 15 * 64]);
    }
}
