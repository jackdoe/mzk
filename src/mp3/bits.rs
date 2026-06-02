pub struct Bits<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
    pub limit: usize,
}

impl<'a> Bits<'a> {
    pub fn new(buf: &'a [u8], bytes: usize) -> Self {
        Bits {
            buf,
            pos: 0,
            limit: bytes * 8,
        }
    }

    pub fn get_bits(&mut self, n: u32) -> u32 {
        let s = (self.pos & 7) as u32;
        let mut shl = n as i32 + s as i32;
        let mut p = self.pos >> 3;
        self.pos += n as usize;
        if self.pos > self.limit {
            return 0;
        }
        let mut next = (self.buf[p] & (255u8 >> s)) as u32;
        p += 1;
        let mut cache = 0u32;
        loop {
            shl -= 8;
            if shl <= 0 {
                break;
            }
            cache |= next << shl;
            next = self.buf[p] as u32;
            p += 1;
        }
        cache | (next >> (-shl) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_get_bits_never_panics() {
        crate::fuzz::each_case(8000, 256, |data| {
            if data.is_empty() {
                return;
            }
            let mut bs = Bits::new(data, data.len());
            for step in 0..512 {
                let n = (step as u32 % 24) + 1;
                let _ = bs.get_bits(n);
                if bs.pos > bs.limit + 64 {
                    break;
                }
            }
        });
    }
}
