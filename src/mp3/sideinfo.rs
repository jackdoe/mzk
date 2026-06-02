use super::bits::Bits;
use super::header::{get_my_sample_rate, is_mono, test_mpeg1};
use super::tables::{SCF_LONG, SCF_MIXED, SCF_SHORT};

pub const SHORT_BLOCK_TYPE: u8 = 2;
pub const STOP_BLOCK_TYPE: u8 = 3;

#[derive(Clone, Copy, Default)]
pub struct GrInfo {
    pub sfbtab: &'static [u8],
    pub part_23_length: u16,
    pub big_values: u16,
    pub scalefac_compress: u16,
    pub global_gain: u8,
    pub block_type: u8,
    pub mixed_block_flag: u8,
    pub n_long_sfb: u8,
    pub n_short_sfb: u8,
    pub table_select: [u8; 3],
    pub region_count: [u8; 3],
    pub subblock_gain: [u8; 3],
    pub preflag: u8,
    pub scalefac_scale: u8,
    pub count1_table: u8,
    pub scfsi: u8,
}

pub fn read_side_info(bs: &mut Bits, gr: &mut [GrInfo], hdr: &[u8]) -> i32 {
    let mut scfsi: u32 = 0;
    let mut part_23_sum = 0i32;
    let mut sr_idx = get_my_sample_rate(hdr);
    sr_idx -= (sr_idx != 0) as usize;
    let mut gr_count = if is_mono(hdr) { 1 } else { 2 };
    let main_data_begin: i32;

    if test_mpeg1(hdr) {
        gr_count *= 2;
        main_data_begin = bs.get_bits(9) as i32;
        scfsi = bs.get_bits(7 + gr_count as u32);
    } else {
        main_data_begin = (bs.get_bits(8 + gr_count as u32) >> gr_count) as i32;
    }

    for g in gr.iter_mut().take(gr_count) {
        if is_mono(hdr) {
            scfsi <<= 4;
        }
        g.part_23_length = bs.get_bits(12) as u16;
        part_23_sum += g.part_23_length as i32;
        g.big_values = bs.get_bits(9) as u16;
        if g.big_values > 288 {
            return -1;
        }
        g.global_gain = bs.get_bits(8) as u8;
        g.scalefac_compress = bs.get_bits(if test_mpeg1(hdr) { 4 } else { 9 }) as u16;
        g.sfbtab = &SCF_LONG[sr_idx];
        g.n_long_sfb = 22;
        g.n_short_sfb = 0;

        let tables;
        if bs.get_bits(1) != 0 {
            g.block_type = bs.get_bits(2) as u8;
            if g.block_type == 0 {
                return -1;
            }
            g.mixed_block_flag = bs.get_bits(1) as u8;
            g.region_count[0] = 7;
            g.region_count[1] = 255;
            if g.block_type == SHORT_BLOCK_TYPE {
                scfsi &= 0x0F0F;
                if g.mixed_block_flag == 0 {
                    g.region_count[0] = 8;
                    g.sfbtab = &SCF_SHORT[sr_idx];
                    g.n_long_sfb = 0;
                    g.n_short_sfb = 39;
                } else {
                    g.sfbtab = &SCF_MIXED[sr_idx];
                    g.n_long_sfb = if test_mpeg1(hdr) { 8 } else { 6 };
                    g.n_short_sfb = 30;
                }
            }
            tables = bs.get_bits(10) << 5;
            g.subblock_gain[0] = bs.get_bits(3) as u8;
            g.subblock_gain[1] = bs.get_bits(3) as u8;
            g.subblock_gain[2] = bs.get_bits(3) as u8;
        } else {
            g.block_type = 0;
            g.mixed_block_flag = 0;
            tables = bs.get_bits(15);
            g.region_count[0] = bs.get_bits(4) as u8;
            g.region_count[1] = bs.get_bits(3) as u8;
            g.region_count[2] = 255;
        }
        g.table_select[0] = (tables >> 10) as u8;
        g.table_select[1] = ((tables >> 5) & 31) as u8;
        g.table_select[2] = (tables & 31) as u8;
        g.preflag = if test_mpeg1(hdr) {
            bs.get_bits(1) as u8
        } else {
            (g.scalefac_compress >= 500) as u8
        };
        g.scalefac_scale = bs.get_bits(1) as u8;
        g.count1_table = bs.get_bits(1) as u8;
        g.scfsi = ((scfsi >> 12) & 15) as u8;
        scfsi <<= 4;
    }

    if part_23_sum + bs.pos as i32 > bs.limit as i32 + main_data_begin * 8 {
        return -1;
    }
    main_data_begin
}
