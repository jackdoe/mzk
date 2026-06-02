use crate::error::Result;

pub trait Decoder: Send {
    fn next(&mut self) -> Option<Vec<f32>>;
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> usize;
    fn duration_frames(&self) -> u64;
    fn pos_frames(&self) -> u64;
    fn seek(&mut self, frame: u64);
}

pub fn open(path: &std::path::Path) -> Result<Box<dyn Decoder>> {
    let ext = path
        .extension()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.to_str() {
        Some("mp3") => Ok(Box::new(crate::mp3::Mp3Decoder::open(path)?)),
        Some("wav") => Ok(Box::new(crate::wav::WavDecoder::open(path)?)),
        Some("flac") => Ok(Box::new(crate::flac::FlacDecoder::open(path)?)),
        Some("m4a") => Ok(Box::new(crate::m4a::M4aDecoder::open(path)?)),
        _ => Ok(Box::new(crate::opus::OpusDecoder::open(path)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_prefix(path: &std::path::Path, max: usize) -> Vec<f32> {
        let mut d = open(path).unwrap();
        let mut v = Vec::new();
        while v.len() < max {
            match d.next() {
                Some(f) => v.extend_from_slice(&f),
                None => break,
            }
        }
        v.truncate(max);
        v
    }

    #[test]
    fn flac_and_wav_decode_identically() {
        let dir = std::path::Path::new("tests/fixtures/voyager");
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut checked = 0;
        for e in entries.flatten() {
            let flac = e.path();
            if flac.extension().and_then(|s| s.to_str()) != Some("flac") {
                continue;
            }
            let wav = flac.with_extension("wav");
            if !wav.exists() {
                continue;
            }
            let max = 44100 * 2 * 5;
            let a = decode_prefix(&flac, max);
            let b = decode_prefix(&wav, max);
            let n = a.len().min(b.len());
            assert!(n > 44100, "{:?} too short", flac);
            let maxd = a[..n]
                .iter()
                .zip(&b[..n])
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(maxd < 1e-6, "{:?} flac vs wav diff {maxd}", flac);
            checked += 1;
        }
        assert!(checked > 0, "no voyager fixtures");
    }
}
