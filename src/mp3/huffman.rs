use super::bits::Bits;
use super::sideinfo::GrInfo;
use super::tables::{HUFF_DIR, HUFF_TREE};

fn decode_symbol(bs: &mut Bits, table: usize) -> (i32, i32, i32, i32) {
    let (off, treelen, linbits) = HUFF_DIR[table];
    if treelen == 0 {
        return (0, 0, 0, 0);
    }
    let off = off as usize;
    let mut point = 0usize;
    loop {
        let node = HUFF_TREE[off + point];
        if node & 0xff00 == 0 {
            break;
        }
        if bs.get_bits(1) == 1 {
            while HUFF_TREE[off + point] & 0xff >= 250 {
                point += (HUFF_TREE[off + point] & 0xff) as usize;
            }
            point += (HUFF_TREE[off + point] & 0xff) as usize;
        } else {
            while HUFF_TREE[off + point] >> 8 >= 250 {
                point += (HUFF_TREE[off + point] >> 8) as usize;
            }
            point += (HUFF_TREE[off + point] >> 8) as usize;
        }
    }
    let node = HUFF_TREE[off + point];
    let mut x = ((node >> 4) & 0xf) as i32;
    let mut y = (node & 0xf) as i32;

    if table > 31 {
        let v = (y >> 3) & 1;
        let w = (y >> 2) & 1;
        let xq = (y >> 1) & 1;
        let yq = y & 1;
        let v = sign(bs, v);
        let w = sign(bs, w);
        let xq = sign(bs, xq);
        let yq = sign(bs, yq);
        (xq, yq, v, w)
    } else {
        if linbits != 0 && x == 15 {
            x += bs.get_bits(linbits as u32) as i32;
        }
        x = sign(bs, x);
        if linbits != 0 && y == 15 {
            y += bs.get_bits(linbits as u32) as i32;
        }
        y = sign(bs, y);
        (x, y, 0, 0)
    }
}

fn sign(bs: &mut Bits, v: i32) -> i32 {
    if v > 0 && bs.get_bits(1) == 1 {
        -v
    } else {
        v
    }
}

fn region_bounds(gr: &GrInfo) -> (usize, usize) {
    if gr.block_type == 2 {
        return (36, 576);
    }
    let mut cum = [0usize; 24];
    for k in 0..gr.sfbtab.len().min(23) {
        cum[k + 1] = cum[k] + gr.sfbtab[k] as usize;
    }
    let r0 = gr.region_count[0] as usize;
    let r1 = gr.region_count[1] as usize;
    let i1 = (r0 + 1).min(23);
    let i2 = (r0 + r1 + 2).min(23);
    (cum[i1], cum[i2])
}

pub fn read_huffman(
    bs: &mut Bits,
    gr: &GrInfo,
    part_2_start: usize,
    is: &mut [i32; 576],
) -> usize {
    is.fill(0);
    if gr.part_23_length == 0 {
        return 0;
    }
    let bit_pos_end = part_2_start + gr.part_23_length as usize - 1;
    let (region_1_start, region_2_start) = region_bounds(gr);

    let big = gr.big_values as usize * 2;
    let mut is_pos = 0usize;
    while is_pos < big {
        let table = if is_pos < region_1_start {
            gr.table_select[0]
        } else if is_pos < region_2_start {
            gr.table_select[1]
        } else {
            gr.table_select[2]
        } as usize;
        let (x, y, _, _) = decode_symbol(bs, table);
        is[is_pos] = x;
        is[is_pos + 1] = y;
        is_pos += 2;
    }

    let table = gr.count1_table as usize + 32;
    while is_pos <= 572 && bs.pos <= bit_pos_end {
        let (x, y, v, w) = decode_symbol(bs, table);
        is[is_pos] = v;
        is_pos += 1;
        if is_pos >= 576 {
            break;
        }
        is[is_pos] = w;
        is_pos += 1;
        if is_pos >= 576 {
            break;
        }
        is[is_pos] = x;
        is_pos += 1;
        if is_pos >= 576 {
            break;
        }
        is[is_pos] = y;
        is_pos += 1;
    }
    if bs.pos > bit_pos_end + 1 {
        is_pos = is_pos.saturating_sub(4);
    }
    bs.pos = bit_pos_end + 1;
    is_pos
}
