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
