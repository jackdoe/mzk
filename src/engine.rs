use crate::audio::PlatformSink;
use crate::celt::{decode_frame, DecoderState, Mode};
use crate::error::Result;
use crate::ogg::OpusStream;
use crate::pcm::{gain_from_q8, scale, Ring};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RATE: u64 = 48000;
const FRAME: usize = 960;
const RING_CAP: usize = 48000 * 2;

#[derive(Clone, Copy, PartialEq)]
pub enum Repeat {
    Off,
    One,
    All,
}

pub enum Command {
    Play(usize),
    Pause,
    Next,
    Prev,
    Vol(f32),
    VolDelta(f32),
    Seek(i64),
    SeekTo(u64),
    Shuffle(bool),
    Repeat(Repeat),
    Quit,
}

#[derive(Clone)]
pub struct Status {
    pub index: usize,
    pub name: String,
    pub pos: u64,
    pub total: u64,
    pub vol: f32,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub paused: bool,
    pub ended: bool,
}

pub struct Engine {
    tx: Sender<Command>,
    status: Arc<Mutex<Status>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn names(playlist: &[PathBuf]) -> Vec<String> {
        playlist.iter().map(|p| track_name(p)).collect()
    }

    pub fn spawn(playlist: Vec<PathBuf>) -> Result<Engine> {
        let first = playlist.first().map(|p| track_name(p)).unwrap_or_default();
        let status = Arc::new(Mutex::new(Status {
            index: 0,
            name: first,
            pos: 0,
            total: 0,
            vol: 1.0,
            shuffle: false,
            repeat: Repeat::All,
            paused: false,
            ended: false,
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        let st = status.clone();
        let handle = std::thread::spawn(move || run(playlist, rx, st));
        Ok(Engine {
            tx,
            status,
            handle: Some(handle),
        })
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn track_name(p: &std::path::Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn next_rand(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 11) as f64) / ((1u64 << 53) as f64)
}

fn artist(p: &std::path::Path) -> String {
    let stem = track_name(p);
    match stem.split_once(" - ") {
        Some((a, _)) => a.trim().to_ascii_lowercase(),
        None => stem.to_ascii_lowercase(),
    }
}

fn shuffle_order(playlist: &[PathBuf], seed: u64) -> Vec<usize> {
    let n = playlist.len();
    if n <= 1 {
        return (0..n).collect();
    }
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for i in 0..n {
        let a = artist(&playlist[i]);
        match groups.iter_mut().find(|(name, _)| *name == a) {
            Some(g) => g.1.push(i),
            None => groups.push((a, vec![i])),
        }
    }
    let mut state = seed | 1;
    let mut placed: Vec<(f64, usize)> = Vec::with_capacity(n);
    for (_, members) in &groups {
        let mut mem = members.clone();
        for i in (1..mem.len()).rev() {
            let j = (next_rand(&mut state) * (i as f64 + 1.0)) as usize;
            mem.swap(i, j.min(i));
        }
        let c = mem.len() as f64;
        let base = next_rand(&mut state) / c;
        for (k, &idx) in mem.iter().enumerate() {
            let jitter = (next_rand(&mut state) - 0.5) / c * 0.5;
            placed.push((base + k as f64 / c + jitter, idx));
        }
    }
    placed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    placed.into_iter().map(|(_, i)| i).collect()
}

struct Track {
    stream: OpusStream,
    dec: DecoderState,
    idx: usize,
    pre_skip: u64,
    gain: f32,
    emitted: u64,
}

impl Track {
    fn open(path: &std::path::Path) -> Result<Track> {
        let data = std::fs::read(path)?;
        let stream = OpusStream::parse(&data)?;
        let gain = gain_from_q8(stream.head.output_gain);
        let pre_skip = stream.head.pre_skip as u64;
        let channels = stream.head.channels.max(1) as usize;
        Ok(Track {
            stream,
            dec: DecoderState::new(channels),
            idx: 0,
            pre_skip,
            gain,
            emitted: 0,
        })
    }

    fn total_frames(&self) -> u64 {
        self.stream.total_samples
    }

    fn pos_frames(&self) -> u64 {
        self.emitted.saturating_sub(self.stream.head.pre_skip as u64)
    }

    fn seek(&mut self, target_frames: u64) {
        let pkt = (target_frames / FRAME as u64) as usize;
        self.idx = pkt.min(self.stream.packets.len());
        self.dec.reset();
        self.pre_skip = 0;
        self.emitted = self.idx as u64 * FRAME as u64;
    }

    fn next(&mut self, mode: &Mode, vol: f32) -> Option<Vec<f32>> {
        if self.idx >= self.stream.packets.len() {
            return None;
        }
        let pkt = self.stream.packets[self.idx].clone();
        self.idx += 1;
        let cfg = match crate::toc::Config::parse(&pkt) {
            Ok(c) => c,
            Err(_) => return Some(Vec::new()),
        };
        let frame = decode_frame(&mut self.dec, mode, cfg.frame, cfg.stereo);
        self.emitted += FRAME as u64;
        let mut drop = 0usize;
        if self.pre_skip > 0 {
            let d = (self.pre_skip as usize).min(FRAME);
            self.pre_skip -= d as u64;
            drop = d * 2;
        }
        let out: Vec<f32> = frame[drop..]
            .iter()
            .map(|&s| scale(s, self.gain, vol))
            .collect();
        Some(out)
    }
}

fn run(playlist: Vec<PathBuf>, rx: Receiver<Command>, status: Arc<Mutex<Status>>) {
    let mode = Mode::new();
    let ring = Ring::new(RING_CAP);
    let writer = ring.writer();

    let n = playlist.len();
    let mut order: Vec<usize> = (0..n).collect();
    let mut order_pos = 0usize;
    let mut shuffle = false;
    let mut repeat = Repeat::All;
    let mut vol = 1.0f32;
    let mut paused = false;

    let mut track = open_index(&playlist, order[order_pos], &status);
    update_status(&status, &playlist, &order, order_pos, &track, vol, shuffle, repeat, paused, 0);

    let mut sink = PlatformSink::new(ring.reader());
    if let Err(e) = sink.start() {
        eprintln!("mzk: audio unavailable: {e}");
    }

    let mut pending: Vec<f32> = Vec::new();
    let mut pushed: u64 = 0;

    loop {
        let mut quit = false;
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Command::Quit => {
                    quit = true;
                }
                Command::Pause => paused = !paused,
                Command::Vol(v) => vol = v.clamp(0.0, 1.0),
                Command::VolDelta(d) => vol = (vol + d).clamp(0.0, 1.0),
                Command::Shuffle(on) => {
                    let cur = order.get(order_pos).copied().unwrap_or(0);
                    shuffle = on;
                    order = if on {
                        let seed = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(1);
                        shuffle_order(&playlist, seed)
                    } else {
                        (0..n).collect()
                    };
                    order_pos = order.iter().position(|&x| x == cur).unwrap_or(0);
                }
                Command::Repeat(r) => repeat = r,
                Command::Next => {
                    advance(&mut order_pos, n, repeat, 1);
                    track = open_index(&playlist, order[order_pos], &status);
                    pending.clear();
                    writer.clear();
                    sink.flush();
                    pushed = 0;
                }
                Command::Prev => {
                    advance(&mut order_pos, n, repeat, -1);
                    track = open_index(&playlist, order[order_pos], &status);
                    pending.clear();
                    writer.clear();
                    sink.flush();
                    pushed = 0;
                }
                Command::Play(i) => {
                    if i < n {
                        order_pos = order.iter().position(|&x| x == i).unwrap_or(0);
                        track = open_index(&playlist, order[order_pos], &status);
                        pending.clear();
                        writer.clear();
                        sink.flush();
                        pushed = 0;
                        paused = false;
                    }
                }
                Command::SeekTo(secs) => {
                    if let Some(t) = track.as_mut() {
                        t.seek(secs * RATE);
                        pending.clear();
                        writer.clear();
                        sink.flush();
                        pushed = t.pos_frames() * 2;
                    }
                }
                Command::Seek(delta) => {
                    if let Some(t) = track.as_mut() {
                        let cur = t.pos_frames() as i64;
                        let tgt = (cur + delta * RATE as i64).max(0) as u64;
                        t.seek(tgt);
                        pending.clear();
                        writer.clear();
                        sink.flush();
                        pushed = t.pos_frames() * 2;
                    }
                }
            }
        }
        if quit {
            break;
        }

        let fill = (RING_CAP - writer.available()) as u64;
        let consumed = pushed.saturating_sub(fill);

        let mut worked = false;
        if !paused {
            if let Some(t) = track.as_mut() {
                if pending.is_empty() {
                    match t.next(&mode, vol) {
                        Some(s) => {
                            pending = s;
                            worked = true;
                        }
                        None => {
                            let last = order_pos;
                            advance(&mut order_pos, n, repeat, 1);
                            if repeat == Repeat::Off && order_pos == last {
                                track = None;
                                set_ended(&status);
                            } else {
                                track = open_index(&playlist, order[order_pos], &status);
                            }
                            pushed = 0;
                        }
                    }
                }
                if !pending.is_empty() {
                    let did = writer.push(&pending);
                    if did > 0 {
                        pending.drain(0..did);
                        pushed += did as u64;
                        worked = true;
                    }
                }
            }
        }

        update_status(&status, &playlist, &order, order_pos, &track, vol, shuffle, repeat, paused, consumed);

        if !worked {
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    sink.stop();
}

fn open_index(
    playlist: &[PathBuf],
    idx: usize,
    status: &Arc<Mutex<Status>>,
) -> Option<Track> {
    match Track::open(&playlist[idx]) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("mzk: {}: {e}", playlist[idx].display());
            let mut s = status.lock().unwrap();
            s.ended = true;
            None
        }
    }
}

fn advance(order_pos: &mut usize, n: usize, repeat: Repeat, dir: i64) {
    if n == 0 {
        return;
    }
    match repeat {
        Repeat::One => {}
        _ => {
            let p = *order_pos as i64 + dir;
            *order_pos = p.rem_euclid(n as i64) as usize;
        }
    }
}

fn set_ended(status: &Arc<Mutex<Status>>) {
    status.lock().unwrap().ended = true;
}

fn update_status(
    status: &Arc<Mutex<Status>>,
    playlist: &[PathBuf],
    order: &[usize],
    order_pos: usize,
    track: &Option<Track>,
    vol: f32,
    shuffle: bool,
    repeat: Repeat,
    paused: bool,
    consumed_samples: u64,
) {
    let mut s = status.lock().unwrap();
    let idx = order.get(order_pos).copied().unwrap_or(0);
    s.index = idx;
    s.name = track_name(&playlist[idx.min(playlist.len().saturating_sub(1))]);
    s.vol = vol;
    s.shuffle = shuffle;
    s.repeat = repeat;
    s.paused = paused;
    if let Some(t) = track {
        s.total = t.total_frames() / (RATE / 1000) / 1000;
        s.pos = (consumed_samples / 2 / RATE).min(s.total.max(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blue_noise_shuffle_is_permutation_and_spreads_artists() {
        let mut pl: Vec<PathBuf> = Vec::new();
        for a in ["Slayer", "Dio", "Ghost"] {
            for t in 0..5 {
                pl.push(PathBuf::from(format!("{a} - song {t}.opus")));
            }
        }
        let order = shuffle_order(&pl, 0x1234_5678);
        let mut seen = order.clone();
        seen.sort();
        assert_eq!(seen, (0..15).collect::<Vec<_>>());

        let arts: Vec<String> = order.iter().map(|&i| artist(&pl[i])).collect();
        let mut adjacent = 0;
        for w in arts.windows(2) {
            if w[0] == w[1] {
                adjacent += 1;
            }
        }
        assert_eq!(adjacent, 0, "same-artist tracks clustered: {arts:?}");
    }
}
