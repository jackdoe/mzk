pub fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut s = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    (0..len)
        .map(|_| {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) as u8
        })
        .collect()
}

pub fn each_case(count: u64, max_len: usize, mut f: impl FnMut(&[u8])) {
    for seed in 0..count {
        let len = (seed as usize * 2654435761) % (max_len + 1);
        f(&bytes(seed, len));
    }
}

pub fn for_seeds(count: u64, mut f: impl FnMut(u64)) {
    for k in 0..count {
        f(k);
    }
}

pub fn corrupt_spread(data: &[u8], mut f: impl FnMut(Vec<u8>)) {
    if data.is_empty() {
        return;
    }
    let n = ((128usize << 20) / data.len()).clamp(64, 600).min(data.len());
    let vals = [0u8, 0x01, 0x80, 0xfe, 0xff];
    for k in 0..n {
        let i = data.len() * k / n;
        for &v in &vals {
            let mut c = data.to_vec();
            c[i] = v;
            f(c);
        }
        let mut x = data.to_vec();
        x[i] ^= 0xff;
        f(x);
    }
}

pub fn truncate_points(data: &[u8], n: usize, mut f: impl FnMut(&[u8])) {
    let n = n.max(1);
    for k in 0..=n {
        let len = data.len() * k / n;
        f(&data[..len]);
    }
}

pub fn read_dir_ext(dir: &str, ext: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.to_string_lossy().ends_with(ext) {
                if let Ok(b) = std::fs::read(&p) {
                    out.push(b);
                }
            }
        }
    }
    out
}
