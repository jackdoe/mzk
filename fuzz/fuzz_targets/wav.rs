#![no_main]
use libfuzzer_sys::fuzz_target;
use mzk::decoder::Decoder;

fuzz_target!(|data: &[u8]| {
    if let Ok(mut d) = mzk::wav::WavDecoder::from_bytes(data.to_vec()) {
        let mut frames = 0u64;
        while d.next().is_some() {
            frames += 1;
            if frames > 65536 {
                break;
            }
        }
    }
});
