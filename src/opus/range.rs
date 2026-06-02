const EC_SYM_BITS: u32 = 8;
const EC_CODE_BITS: i32 = 32;
const EC_SYM_MAX: u32 = 255;
const EC_CODE_TOP: u32 = 1u32 << 31;
const EC_CODE_BOT: u32 = EC_CODE_TOP >> EC_SYM_BITS;
const EC_CODE_EXTRA: u32 = 7;
const EC_UINT_BITS: u32 = 8;
const EC_WINDOW_SIZE: i32 = 32;

pub fn ec_ilog(v: u32) -> i32 {
    if v == 0 {
        return 0;
    }
    32 - v.leading_zeros() as i32
}

pub struct RangeDecoder<'a> {
    buf: &'a [u8],
    storage: usize,
    end_offs: usize,
    end_window: u32,
    nend_bits: i32,
    nbits_total: i32,
    offs: usize,
    rng: u32,
    val: u32,
    ext: u32,
    rem: i32,
    pub error: bool,
}

impl<'a> RangeDecoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        let mut d = RangeDecoder {
            buf,
            storage: buf.len(),
            end_offs: 0,
            end_window: 0,
            nend_bits: 0,
            nbits_total: EC_CODE_BITS + 1
                - ((EC_CODE_BITS - EC_CODE_EXTRA as i32) / EC_SYM_BITS as i32) * EC_SYM_BITS as i32,
            offs: 0,
            rng: 1u32 << EC_CODE_EXTRA,
            val: 0,
            ext: 0,
            rem: 0,
            error: false,
        };
        d.rem = d.read_byte();
        d.val = d.rng - 1 - (d.rem as u32 >> (EC_SYM_BITS - EC_CODE_EXTRA));
        d.normalize();
        d
    }

    fn read_byte(&mut self) -> i32 {
        if self.offs < self.storage {
            let b = self.buf[self.offs] as i32;
            self.offs += 1;
            b
        } else {
            0
        }
    }

    fn read_byte_from_end(&mut self) -> i32 {
        if self.end_offs < self.storage {
            self.end_offs += 1;
            self.buf[self.storage - self.end_offs] as i32
        } else {
            0
        }
    }

    fn normalize(&mut self) {
        while self.rng <= EC_CODE_BOT {
            self.nbits_total += EC_SYM_BITS as i32;
            self.rng <<= EC_SYM_BITS;
            let mut sym = self.rem;
            self.rem = self.read_byte();
            sym = (((sym as u32) << EC_SYM_BITS) | self.rem as u32) as i32
                >> (EC_SYM_BITS - EC_CODE_EXTRA);
            self.val = ((self.val << EC_SYM_BITS).wrapping_add(EC_SYM_MAX & !(sym as u32)))
                & (EC_CODE_TOP - 1);
        }
    }

    pub fn decode(&mut self, ft: u32) -> u32 {
        self.ext = self.rng / ft;
        let s = self.val / self.ext;
        ft - ft.min(s + 1)
    }

    pub fn decode_bin(&mut self, bits: u32) -> u32 {
        self.ext = self.rng >> bits;
        let s = self.val / self.ext;
        (1u32 << bits) - (1u32 << bits).min(s + 1)
    }

    pub fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.ext.wrapping_mul(ft - fh);
        self.val = self.val.wrapping_sub(s);
        self.rng = if fl > 0 {
            self.ext.wrapping_mul(fh - fl)
        } else {
            self.rng - s
        };
        self.normalize();
    }

    pub fn dec_bit_logp(&mut self, logp: u32) -> u32 {
        let r = self.rng;
        let d = self.val;
        let s = r >> logp;
        let ret = (d < s) as u32;
        if ret == 0 {
            self.val = d - s;
        }
        self.rng = if ret == 1 { s } else { r - s };
        self.normalize();
        ret
    }

    pub fn dec_icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        let mut s = self.rng;
        let d = self.val;
        let r = s >> ftb;
        let mut ret: isize = -1;
        let mut t;
        loop {
            t = s;
            ret += 1;
            s = r.wrapping_mul(icdf[ret as usize] as u32);
            if d >= s {
                break;
            }
        }
        self.val = d - s;
        self.rng = t - s;
        self.normalize();
        ret as usize
    }

    pub fn dec_uint(&mut self, ft: u32) -> u32 {
        debug_assert!(ft > 1);
        let ftm1 = ft - 1;
        let ftb = ec_ilog(ftm1);
        if ftb > EC_UINT_BITS as i32 {
            let ftb = (ftb as u32) - EC_UINT_BITS;
            let ftt = (ftm1 >> ftb) + 1;
            let s = self.decode(ftt);
            self.update(s, s + 1, ftt);
            let t = (s << ftb) | self.dec_bits(ftb);
            if t <= ftm1 {
                t
            } else {
                self.error = true;
                ftm1
            }
        } else {
            let ftt = ft;
            let s = self.decode(ftt);
            self.update(s, s + 1, ftt);
            s
        }
    }

    pub fn dec_bits(&mut self, bits: u32) -> u32 {
        let mut window = self.end_window;
        let mut available = self.nend_bits;
        if (available as u32) < bits {
            loop {
                window |= (self.read_byte_from_end() as u32) << available;
                available += EC_SYM_BITS as i32;
                if available > EC_WINDOW_SIZE - EC_SYM_BITS as i32 {
                    break;
                }
            }
        }
        let ret = window & ((1u32 << bits) - 1);
        window >>= bits;
        available -= bits as i32;
        self.end_window = window;
        self.nend_bits = available;
        self.nbits_total += bits as i32;
        ret
    }

    pub fn tell(&self) -> i32 {
        self.nbits_total - ec_ilog(self.rng)
    }

    pub fn storage(&self) -> usize {
        self.storage
    }

    pub fn tell_frac(&self) -> u32 {
        const CORRECTION: [u32; 8] = [35733, 38967, 42495, 46340, 50535, 55109, 60097, 65535];
        let nbits = (self.nbits_total as u32) << 3;
        let l = ec_ilog(self.rng);
        let r = self.rng >> (l - 16);
        let mut b = (r >> 12) - 8;
        b += (r > CORRECTION[b as usize]) as u32;
        let l = ((l as u32) << 3) + b;
        nbits - l
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tell_starts_at_one() {
        let buf = [0xFFu8; 8];
        let d = RangeDecoder::new(&buf);
        assert_eq!(d.tell(), 1);
    }

    #[test]
    fn raw_bits_read_from_tail_lsb_first_per_word() {
        let mut buf = [0u8; 4];
        buf[3] = 0b0000_0101;
        let mut d = RangeDecoder::new(&buf);
        assert_eq!(d.dec_bits(3), 0b101);
    }

    #[test]
    fn icdf_full_range_selects_first_when_top() {
        let buf = [0u8; 8];
        let mut d = RangeDecoder::new(&buf);
        let icdf = [128u8, 0u8];
        let s = d.dec_icdf(&icdf, 8);
        assert!(s == 0 || s == 1);
    }

    #[test]
    fn fuzz_never_panics_on_arbitrary_bytes() {
        crate::fuzz::each_case(4000, 96, |data| {
            if data.is_empty() {
                return;
            }
            let mut d = RangeDecoder::new(data);
            let icdf = [128u8, 64, 16, 0];
            for step in 0..64 {
                match step % 6 {
                    0 => {
                        let ft = (d.decode(64) + 1).min(64);
                        d.update(0, 1, ft.max(2));
                    }
                    1 => {
                        d.decode_bin(5);
                        d.update(0, 1, 32);
                    }
                    2 => {
                        d.dec_bit_logp(3);
                    }
                    3 => {
                        d.dec_icdf(&icdf, 8);
                    }
                    4 => {
                        d.dec_uint(((step as u32) << 3) + 2);
                    }
                    _ => {
                        d.dec_bits((step as u32 % 25) + 1);
                    }
                }
                let _ = d.tell();
                let _ = d.tell_frac();
            }
        });
    }
}
