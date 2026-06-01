use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Shared {
    buf: Box<[f32]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

pub struct Ring {
    shared: Arc<Shared>,
}

pub struct Writer {
    shared: Arc<Shared>,
}

pub struct Reader {
    shared: Arc<Shared>,
}

impl Ring {
    pub fn new(capacity_samples: usize) -> Self {
        let cap = capacity_samples.max(1).next_power_of_two();
        let shared = Arc::new(Shared {
            buf: vec![0f32; cap].into_boxed_slice(),
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        });
        Ring { shared }
    }

    pub fn writer(&self) -> Writer {
        Writer {
            shared: self.shared.clone(),
        }
    }

    pub fn reader(&self) -> Reader {
        Reader {
            shared: self.shared.clone(),
        }
    }
}

impl Writer {
    pub fn push(&self, data: &[f32]) -> usize {
        let cap = self.shared.buf.len();
        let head = self.shared.head.load(Ordering::Relaxed);
        let tail = self.shared.tail.load(Ordering::Acquire);
        let used = head.wrapping_sub(tail);
        let free = cap - used;
        let n = data.len().min(free);
        let ptr = self.shared.buf.as_ptr() as *mut f32;
        for i in 0..n {
            let idx = head.wrapping_add(i) & self.shared.mask;
            unsafe {
                *ptr.add(idx) = data[i];
            }
        }
        self.shared
            .head
            .store(head.wrapping_add(n), Ordering::Release);
        n
    }

    pub fn available(&self) -> usize {
        let cap = self.shared.buf.len();
        let head = self.shared.head.load(Ordering::Relaxed);
        let tail = self.shared.tail.load(Ordering::Acquire);
        cap - head.wrapping_sub(tail)
    }

    pub fn clear(&self) {
        let tail = self.shared.tail.load(Ordering::Acquire);
        self.shared.head.store(tail, Ordering::Release);
    }
}

impl Reader {
    pub fn pop(&self, out: &mut [f32]) -> usize {
        let tail = self.shared.tail.load(Ordering::Relaxed);
        let head = self.shared.head.load(Ordering::Acquire);
        let used = head.wrapping_sub(tail);
        let n = out.len().min(used);
        for i in 0..n {
            let idx = tail.wrapping_add(i) & self.shared.mask;
            out[i] = self.shared.buf[idx];
        }
        self.shared
            .tail
            .store(tail.wrapping_add(n), Ordering::Release);
        n
    }
}

pub fn scale(sample: f32, gain: f32, volume: f32) -> f32 {
    (sample * gain * volume).clamp(-1.0, 1.0)
}

pub fn gain_from_q8(output_gain: i16) -> f32 {
    10f32.powf(output_gain as f32 / (20.0 * 256.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_roundtrip() {
        let ring = Ring::new(8);
        let w = ring.writer();
        let r = ring.reader();
        assert_eq!(w.push(&[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0f32; 3];
        assert_eq!(r.pop(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn scale_clamps() {
        assert_eq!(scale(2.0, 1.0, 1.0), 1.0);
        assert_eq!(scale(-2.0, 1.0, 1.0), -1.0);
        assert_eq!(scale(0.5, 1.0, 0.5), 0.25);
    }
}
