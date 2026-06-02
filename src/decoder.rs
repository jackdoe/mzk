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
    if ext == "mp3" {
        Ok(Box::new(crate::mp3::Mp3Decoder::open(path)?))
    } else {
        Ok(Box::new(crate::opus::OpusDecoder::open(path)?))
    }
}
