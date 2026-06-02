#![no_main]
use libfuzzer_sys::fuzz_target;
use mzk::decoder::Decoder;

fuzz_target!(|data: &[u8]| {
    if let Ok(mut d) = mzk::opus::OpusDecoder::from_bytes(data) {
        let mut frames = 0u64;
        while let Some(f) = d.next() {
            assert!(f.iter().all(|v| v.is_finite()));
            frames += 1;
            if frames > 4096 {
                break;
            }
        }
    }
});
