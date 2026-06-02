use crate::error::{Error, Result};

const MAX_FILE: u64 = 512 << 20;

pub fn read_file_capped(path: &std::path::Path) -> Result<Vec<u8>> {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_FILE {
        return Err(Error::Unsupported("file too large"));
    }
    Ok(std::fs::read(path)?)
}

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

    fn all_fixtures() -> Vec<Vec<u8>> {
        let mut v = crate::fuzz::read_dir_ext("tests/fixtures", ".mp3");
        v.extend(crate::fuzz::read_dir_ext("tests/fixtures", ".opus"));
        for ext in [".flac", ".wav", ".alac.m4a", ".aac.m4a", ".mp3", ".opus"] {
            v.extend(crate::fuzz::read_dir_ext("tests/fixtures/voyager", ext));
        }
        v
    }

    fn drive(mut d: Box<dyn Decoder>) {
        let _ = (d.sample_rate(), d.channels(), d.duration_frames(), d.pos_frames());
        for _ in 0..8 {
            if d.next().is_none() {
                break;
            }
        }
    }

    #[test]
    fn cross_format_confusion_never_panics() {
        let fixtures = all_fixtures();
        let fixtures = &fixtures;
        crate::fuzz::for_seeds(fixtures.len() as u64, |i| {
            let work: Vec<u8> = fixtures[i as usize].iter().take(64 * 1024).copied().collect();
            if let Ok(d) = crate::mp3::Mp3Decoder::from_bytes(work.clone()) {
                drive(Box::new(d));
            }
            if let Ok(d) = crate::wav::WavDecoder::from_bytes(work.clone()) {
                drive(Box::new(d));
            }
            if let Ok(d) = crate::flac::FlacDecoder::from_bytes(work.clone()) {
                drive(Box::new(d));
            }
            if let Ok(d) = crate::m4a::M4aDecoder::from_bytes(work.clone()) {
                drive(Box::new(d));
            }
            if let Ok(d) = crate::opus::OpusDecoder::from_bytes(&work) {
                drive(Box::new(d));
            }
        });
    }

    #[test]
    fn degenerate_inputs_never_panic() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![0xff],
            vec![0xff; 3],
            vec![0xff; 4],
            b"RIFF".to_vec(),
            b"fLaC".to_vec(),
            b"OggS".to_vec(),
            b"RIFFWAVE".to_vec(),
            vec![0x00; 4096],
            vec![0xff; 4096],
        ];
        for c in cases {
            let _ = crate::mp3::Mp3Decoder::from_bytes(c.clone());
            let _ = crate::wav::WavDecoder::from_bytes(c.clone());
            let _ = crate::flac::FlacDecoder::from_bytes(c.clone());
            let _ = crate::m4a::M4aDecoder::from_bytes(c.clone());
            let _ = crate::opus::OpusDecoder::from_bytes(&c);
        }
    }
}
