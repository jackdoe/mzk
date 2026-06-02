use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Shared {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Sync for Shared {}

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
        let buf = (0..cap)
            .map(|_| UnsafeCell::new(0f32))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let shared = Arc::new(Shared {
            buf,
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
        for i in 0..n {
            let idx = head.wrapping_add(i) & self.shared.mask;
            unsafe {
                *self.shared.buf[idx].get() = data[i];
            }
        }
        self.shared
            .head
            .store(head.wrapping_add(n), Ordering::Release);
        n
    }

    pub fn available(&self) -> usize {
        self.shared.buf.len() - self.len()
    }

    pub fn len(&self) -> usize {
        let head = self.shared.head.load(Ordering::Relaxed);
        let tail = self.shared.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
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
            out[i] = unsafe { *self.shared.buf[idx].get() };
        }
        self.shared
            .tail
            .store(tail.wrapping_add(n), Ordering::Release);
        n
    }
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
    fn ring_concurrent_spsc() {
        const N: usize = 300;
        let ring = Ring::new(64);
        let w = ring.writer();
        let r = ring.reader();
        let producer = std::thread::spawn(move || {
            let mut i = 0usize;
            while i < N {
                if w.push(&[i as f32]) == 1 {
                    i += 1;
                } else {
                    std::thread::yield_now();
                }
            }
        });
        let mut got = Vec::with_capacity(N);
        let mut buf = [0.0f32; 1];
        while got.len() < N {
            if r.pop(&mut buf) == 1 {
                got.push(buf[0]);
            } else {
                std::thread::yield_now();
            }
        }
        producer.join().unwrap();
        for (i, v) in got.iter().enumerate() {
            assert_eq!(*v, i as f32);
        }
    }
}
