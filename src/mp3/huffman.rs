use super::bits::Bits;
use super::requant::pow_43;
use super::sideinfo::GrInfo;
use super::tables::{HUFF_LINBITS, HUFF_TAB32, HUFF_TAB33, HUFF_TABINDEX, HUFF_TABS, POW43};

struct Cache<'a> {
    buf: &'a [u8],
    next: usize,
    cache: u32,
    sh: i32,
}

impl<'a> Cache<'a> {
    fn peek(&self, n: u32) -> u32 {
        self.cache >> (32 - n)
    }
    fn flush(&mut self, n: u32) {
        self.cache <<= n;
        self.sh += n as i32;
    }
    fn check(&mut self) {
        while self.sh >= 0 {
            self.cache |= (self.buf[self.next] as u32) << self.sh;
            self.next += 1;
            self.sh -= 8;
        }
    }
    fn bspos(&self) -> i32 {
        self.next as i32 * 8 - 24 + self.sh
    }
    fn neg(&self) -> bool {
        (self.cache as i32) < 0
    }
}

pub fn huffman(dst: &mut [f32], bs: &mut Bits, gr: &GrInfo, scf: &[f32], layer3gr_limit: i32) {
    let mut di = 0usize;
    let mut ci = 0usize;
    let mut si = 0usize;
    let mut ireg = 0usize;
    let mut big_val_cnt = gr.big_values as i32;
    let sfb = gr.sfbtab;

    let nb = bs.buf;
    let p0 = bs.pos / 8;
    let s = (bs.pos & 7) as i32;
    let cache_init = (((nb[p0] as u32 * 256 + nb[p0 + 1] as u32) * 256 + nb[p0 + 2] as u32) * 256
        + nb[p0 + 3] as u32)
        << s;
    let mut c = Cache {
        buf: nb,
        next: p0 + 4,
        cache: cache_init,
        sh: s - 8,
    };

    let mut one = 0.0f32;

    while big_val_cnt > 0 {
        let tab_num = gr.table_select[ireg] as usize;
        let mut sfb_cnt = gr.region_count[ireg] as i32;
        ireg += 1;
        let cb = HUFF_TABINDEX[tab_num] as usize;
        let linbits = HUFF_LINBITS[tab_num] as u32;

        loop {
            let np = (sfb[si] / 2) as i32;
            si += 1;
            let mut pairs = big_val_cnt.min(np);
            one = scf[ci];
            ci += 1;
            loop {
                let mut w = 5u32;
                let mut leaf = HUFF_TABS[cb + c.peek(w) as usize] as i32;
                while leaf < 0 {
                    c.flush(w);
                    w = (leaf & 7) as u32;
                    leaf = HUFF_TABS[(cb as i32 + c.peek(w) as i32 - (leaf >> 3)) as usize] as i32;
                }
                c.flush((leaf >> 8) as u32);

                for _ in 0..2 {
                    let lsb = leaf & 0x0F;
                    if linbits != 0 && lsb == 15 {
                        let v = lsb + c.peek(linbits) as i32;
                        c.flush(linbits);
                        c.check();
                        if di < dst.len() {
                            dst[di] = one * pow_43(v) * if c.neg() { -1.0 } else { 1.0 };
                        }
                    } else if di < dst.len() {
                        dst[di] = POW43[(16 + lsb - 16 * (c.cache >> 31) as i32) as usize] * one;
                    }
                    c.flush(if lsb != 0 { 1 } else { 0 });
                    di += 1;
                    leaf >>= 4;
                }
                c.check();
                pairs -= 1;
                if pairs == 0 {
                    break;
                }
            }
            big_val_cnt -= np;
            sfb_cnt -= 1;
            if big_val_cnt <= 0 || sfb_cnt < 0 {
                break;
            }
        }
    }

    let mut np = 1 - big_val_cnt;
    loop {
        let cb1 = if gr.count1_table != 0 {
            &HUFF_TAB33[..]
        } else {
            &HUFF_TAB32[..]
        };
        let mut leaf = cb1[c.peek(4) as usize] as i32;
        if leaf & 8 == 0 {
            leaf = cb1[((leaf >> 3) + ((c.cache << 4) >> (32 - (leaf & 3))) as i32) as usize] as i32;
        }
        c.flush((leaf & 7) as u32);
        if c.bspos() > layer3gr_limit {
            break;
        }

        np -= 1;
        if np == 0 {
            np = (sfb[si] / 2) as i32;
            si += 1;
            if np == 0 {
                break;
            }
            one = scf[ci];
            ci += 1;
        }
        if leaf & (128 >> 0) != 0 {
            if di < dst.len() {
                dst[di] = if c.neg() { -one } else { one };
            }
            c.flush(1);
        }
        if leaf & (128 >> 1) != 0 {
            if di + 1 < dst.len() {
                dst[di + 1] = if c.neg() { -one } else { one };
            }
            c.flush(1);
        }
        np -= 1;
        if np == 0 {
            np = (sfb[si] / 2) as i32;
            si += 1;
            if np == 0 {
                break;
            }
            one = scf[ci];
            ci += 1;
        }
        if leaf & (128 >> 2) != 0 {
            if di + 2 < dst.len() {
                dst[di + 2] = if c.neg() { -one } else { one };
            }
            c.flush(1);
        }
        if leaf & (128 >> 3) != 0 {
            if di + 3 < dst.len() {
                dst[di + 3] = if c.neg() { -one } else { one };
            }
            c.flush(1);
        }
        c.check();
        di += 4;
    }

    bs.pos = layer3gr_limit as usize;
}
