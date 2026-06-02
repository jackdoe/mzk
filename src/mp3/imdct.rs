use std::f32::consts::PI;
use std::sync::OnceLock;

struct Imdct {
    cos36: [[f32; 36]; 18],
    cos12: [[f32; 12]; 6],
    win: [[f32; 36]; 4],
}

fn tables() -> &'static Imdct {
    static T: OnceLock<Imdct> = OnceLock::new();
    T.get_or_init(|| {
        let mut cos36 = [[0.0f32; 36]; 18];
        for m in 0..18 {
            for p in 0..36 {
                cos36[m][p] = (PI / 72.0 * (2 * p + 1 + 18) as f32 * (2 * m + 1) as f32).cos();
            }
        }
        let mut cos12 = [[0.0f32; 12]; 6];
        for m in 0..6 {
            for p in 0..12 {
                cos12[m][p] = (PI / 24.0 * (2 * p + 1 + 6) as f32 * (2 * m + 1) as f32).cos();
            }
        }
        let mut win = [[0.0f32; 36]; 4];
        for i in 0..36 {
            win[0][i] = (PI / 36.0 * (i as f32 + 0.5)).sin();
        }
        for i in 0..18 {
            win[1][i] = (PI / 36.0 * (i as f32 + 0.5)).sin();
        }
        for i in 18..24 {
            win[1][i] = 1.0;
        }
        for i in 24..30 {
            win[1][i] = (PI / 12.0 * (i as f32 + 0.5 - 18.0)).sin();
        }
        for i in 0..12 {
            win[2][i] = (PI / 12.0 * (i as f32 + 0.5)).sin();
        }
        for i in 6..12 {
            win[3][i] = (PI / 12.0 * (i as f32 + 0.5 - 6.0)).sin();
        }
        for i in 12..18 {
            win[3][i] = 1.0;
        }
        for i in 18..36 {
            win[3][i] = (PI / 36.0 * (i as f32 + 0.5)).sin();
        }
        Imdct { cos36, cos12, win }
    })
}

fn imdct_win(input: &[f32], out: &mut [f32; 36], block_type: u8) {
    let t = tables();
    *out = [0.0; 36];
    if block_type == 2 {
        for i in 0..3 {
            for p in 0..12 {
                let mut sum = 0.0;
                for m in 0..6 {
                    sum += input[i + 3 * m] * t.cos12[m][p];
                }
                out[6 * i + p + 6] += sum * t.win[2][p];
            }
        }
    } else {
        let w = &t.win[block_type as usize];
        for p in 0..36 {
            let mut sum = 0.0;
            for m in 0..18 {
                sum += input[m] * t.cos36[m][p];
            }
            out[p] = sum * w[p];
        }
    }
}

pub fn hybrid_synthesis(grbuf: &mut [f32], overlap: &mut [f32], block_type: u8, n_long_bands: usize) {
    let mut raw = [0.0f32; 36];
    for sb in 0..32 {
        let bt = if sb < n_long_bands { 0 } else { block_type };
        imdct_win(&grbuf[sb * 18..sb * 18 + 18], &mut raw, bt);
        for i in 0..18 {
            grbuf[sb * 18 + i] = raw[i] + overlap[sb * 18 + i];
            overlap[sb * 18 + i] = raw[i + 18];
        }
    }
}

pub fn frequency_inversion(grbuf: &mut [f32]) {
    let mut sb = 1;
    while sb < 32 {
        let mut i = 1;
        while i < 18 {
            grbuf[sb * 18 + i] = -grbuf[sb * 18 + i];
            i += 2;
        }
        sb += 2;
    }
}
