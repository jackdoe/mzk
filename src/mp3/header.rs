pub const HDR_SIZE: usize = 4;
const MAX_FREE_FORMAT_FRAME_SIZE: usize = 2304;
const MAX_FRAME_SYNC_MATCHES: usize = 10;

pub fn is_mono(h: &[u8]) -> bool {
    h[3] & 0xC0 == 0xC0
}
pub fn is_ms_stereo(h: &[u8]) -> bool {
    h[3] & 0xE0 == 0x60
}
pub fn is_free_format(h: &[u8]) -> bool {
    h[2] & 0xF0 == 0
}
pub fn is_crc(h: &[u8]) -> bool {
    h[1] & 1 == 0
}
fn test_padding(h: &[u8]) -> bool {
    h[2] & 0x2 != 0
}
pub fn test_mpeg1(h: &[u8]) -> bool {
    h[1] & 0x8 != 0
}
fn test_not_mpeg25(h: &[u8]) -> bool {
    h[1] & 0x10 != 0
}
pub fn test_i_stereo(h: &[u8]) -> bool {
    h[3] & 0x10 != 0
}
pub fn test_ms_stereo(h: &[u8]) -> bool {
    h[3] & 0x20 != 0
}
pub fn get_layer(h: &[u8]) -> u32 {
    (h[1] as u32 >> 1) & 3
}
fn get_bitrate(h: &[u8]) -> usize {
    (h[2] as usize) >> 4
}
fn get_sample_rate(h: &[u8]) -> usize {
    (h[2] as usize >> 2) & 3
}
pub fn get_my_sample_rate(h: &[u8]) -> usize {
    get_sample_rate(h) + (((h[1] as usize >> 3) & 1) + ((h[1] as usize >> 4) & 1)) * 3
}
fn is_frame_576(h: &[u8]) -> bool {
    h[1] & 14 == 2
}
fn is_layer_1(h: &[u8]) -> bool {
    h[1] & 6 == 6
}

pub fn valid(h: &[u8]) -> bool {
    h[0] == 0xff
        && ((h[1] & 0xF0) == 0xf0 || (h[1] & 0xFE) == 0xe2)
        && get_layer(h) != 0
        && get_bitrate(h) != 15
        && get_sample_rate(h) != 3
}

pub fn compare(h1: &[u8], h2: &[u8]) -> bool {
    valid(h2)
        && ((h1[1] ^ h2[1]) & 0xFE) == 0
        && ((h1[2] ^ h2[2]) & 0x0C) == 0
        && !(is_free_format(h1) ^ is_free_format(h2))
}

pub fn bitrate_kbps(h: &[u8]) -> usize {
    const HALFRATE: [[[u8; 15]; 3]; 2] = [
        [
            [0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80],
            [0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80],
            [0, 16, 24, 28, 32, 40, 48, 56, 64, 72, 80, 88, 96, 112, 128],
        ],
        [
            [0, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160],
            [0, 16, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192],
            [0, 16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224],
        ],
    ];
    2 * HALFRATE[test_mpeg1(h) as usize][(get_layer(h) - 1) as usize][get_bitrate(h)] as usize
}

pub fn sample_rate_hz(h: &[u8]) -> u32 {
    const HZ: [u32; 3] = [44100, 48000, 32000];
    HZ[get_sample_rate(h)] >> (!test_mpeg1(h)) as u32 >> (!test_not_mpeg25(h)) as u32
}

pub fn frame_samples(h: &[u8]) -> usize {
    if is_layer_1(h) {
        384
    } else {
        1152 >> is_frame_576(h) as usize
    }
}

pub fn frame_bytes(h: &[u8], free_format_size: usize) -> usize {
    let mut fb = frame_samples(h) * bitrate_kbps(h) * 125 / sample_rate_hz(h) as usize;
    if is_layer_1(h) {
        fb &= !3;
    }
    if fb != 0 {
        fb
    } else {
        free_format_size
    }
}

pub fn padding(h: &[u8]) -> usize {
    if test_padding(h) {
        if is_layer_1(h) {
            4
        } else {
            1
        }
    } else {
        0
    }
}

fn match_frame(mp3: &[u8], frame_bytes_in: usize) -> bool {
    let mp3_bytes = mp3.len();
    let mut i = 0usize;
    for nmatch in 0..MAX_FRAME_SYNC_MATCHES {
        i += frame_bytes(&mp3[i..], frame_bytes_in) + padding(&mp3[i..]);
        if i + HDR_SIZE > mp3_bytes {
            return nmatch > 0;
        }
        if !compare(mp3, &mp3[i..]) {
            return false;
        }
    }
    true
}

pub fn find_frame(mp3: &[u8], free_format_bytes: &mut usize) -> (usize, usize) {
    let mp3_bytes = mp3.len();
    if mp3_bytes < HDR_SIZE {
        return (mp3_bytes, 0);
    }
    for i in 0..mp3_bytes - HDR_SIZE {
        let h = &mp3[i..];
        if valid(h) {
            let mut fb = frame_bytes(h, *free_format_bytes);
            let mut frame_and_padding = fb + padding(h);

            let mut k = HDR_SIZE;
            while fb == 0 && k < MAX_FREE_FORMAT_FRAME_SIZE && i + 2 * k < mp3_bytes - HDR_SIZE {
                if compare(h, &mp3[i + k..]) {
                    let cand = k - padding(h);
                    let nextfb = cand + padding(&mp3[i + k..]);
                    if i + k + nextfb + HDR_SIZE > mp3_bytes || !compare(h, &mp3[i + k + nextfb..]) {
                        k += 1;
                        continue;
                    }
                    frame_and_padding = k;
                    fb = cand;
                    *free_format_bytes = cand;
                }
                k += 1;
            }

            if (fb != 0
                && i + frame_and_padding <= mp3_bytes
                && match_frame(&mp3[i..], fb))
                || (i == 0 && frame_and_padding == mp3_bytes)
            {
                return (i, frame_and_padding);
            }
            *free_format_bytes = 0;
        }
    }
    (mp3_bytes, 0)
}

pub fn skip_id3v2(data: &[u8]) -> usize {
    if data.len() > 10 && &data[..3] == b"ID3" {
        let size = ((data[6] as usize & 0x7f) << 21)
            | ((data[7] as usize & 0x7f) << 14)
            | ((data[8] as usize & 0x7f) << 7)
            | (data[9] as usize & 0x7f);
        return 10 + size;
    }
    0
}
