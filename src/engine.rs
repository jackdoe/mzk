use crate::audio::PlatformSink;
use crate::decoder::{open, Decoder};
use crate::error::Result;
use crate::pcm::Ring;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RING_CAP: usize = 48000 * 2;

#[derive(Clone, Copy, PartialEq)]
pub enum Repeat {
    Off,
    One,
    All,
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub shuffle: bool,
    pub fav_only: bool,
    pub vol: f32,
    pub repeat: Repeat,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            shuffle: false,
            fav_only: false,
            vol: 1.0,
            repeat: Repeat::All,
        }
    }
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
    ShuffleFavorites,
    ToggleFavorite,
    Repeat(Repeat),
    Quit,
}

#[derive(Clone)]
pub struct Status {
    pub index: usize,
    pub name: String,
    pub ext: String,
    pub rate: u32,
    pub channels: u32,
    pub pos: u64,
    pub total: u64,
    pub vol: f32,
    pub shuffle: bool,
    pub fav_only: bool,
    pub favorite: bool,
    pub repeat: Repeat,
    pub paused: bool,
    pub ended: bool,
}

pub struct Engine {
    tx: Sender<Command>,
    status: Arc<Mutex<Status>>,
    sent: AtomicU64,
    acked: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn names(playlist: &[PathBuf]) -> Vec<String> {
        playlist.iter().map(|p| track_label(p)).collect()
    }

    pub fn spawn(playlist: Vec<PathBuf>, settings: Settings) -> Result<Engine> {
        let first = playlist.first().map(|p| track_name(p)).unwrap_or_default();
        let first_ext = playlist.first().map(|p| track_ext(p)).unwrap_or_default();
        let status = Arc::new(Mutex::new(Status {
            index: 0,
            name: first,
            ext: first_ext,
            rate: 0,
            channels: 0,
            pos: 0,
            total: 0,
            vol: settings.vol,
            shuffle: settings.shuffle,
            fav_only: settings.fav_only,
            favorite: false,
            repeat: settings.repeat,
            paused: false,
            ended: false,
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        let st = status.clone();
        let acked = Arc::new(AtomicU64::new(0));
        let ack = acked.clone();
        let handle = std::thread::spawn(move || run(playlist, settings, rx, st, ack));
        Ok(Engine {
            tx,
            status,
            sent: AtomicU64::new(0),
            acked,
            handle: Some(handle),
        })
    }

    pub fn send(&self, c: Command) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(c);
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn sync(&self) -> Status {
        let want = self.sent.load(Ordering::Relaxed);
        for _ in 0..200 {
            if self.acked.load(Ordering::Acquire) >= want {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.status()
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

fn track_ext(p: &std::path::Path) -> String {
    p.extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn track_label(p: &std::path::Path) -> String {
    let ext = track_ext(p);
    if ext.is_empty() {
        track_name(p)
    } else {
        format!("{}.{}", track_name(p), ext)
    }
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
    placed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    placed.into_iter().map(|(_, i)| i).collect()
}

fn fav_store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("mzk").join("favorites"))
}

fn load_favorites() -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    if let Some(p) = fav_store_path() {
        if let Ok(s) = std::fs::read_to_string(p) {
            for line in s.lines() {
                let l = line.trim();
                if !l.is_empty() {
                    set.insert(PathBuf::from(l));
                }
            }
        }
    }
    set
}

fn save_favorites(favorites: &HashSet<PathBuf>) {
    if let Some(p) = fav_store_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::new();
        for path in favorites {
            out.push_str(&path.to_string_lossy());
            out.push('\n');
        }
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, out).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

fn canon_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn seed_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

fn build_order(
    playlist: &[PathBuf],
    canon: &[PathBuf],
    favorites: &HashSet<PathBuf>,
    shuffle: bool,
    fav_only: bool,
    seed: u64,
) -> Vec<usize> {
    let n = playlist.len();
    let base: Vec<usize> = if shuffle {
        shuffle_order(playlist, seed)
    } else {
        (0..n).collect()
    };
    if fav_only {
        let favs: Vec<usize> = base
            .iter()
            .copied()
            .filter(|&i| favorites.contains(&canon[i]))
            .collect();
        if favs.is_empty() {
            base
        } else {
            favs
        }
    } else {
        base
    }
}

type Track = Box<dyn Decoder>;

fn format_of(track: &Option<Track>) -> (u32, u32) {
    match track {
        Some(t) => (t.sample_rate(), t.channels() as u32),
        None => (48000, 2),
    }
}

fn reopen_sink(
    sink: &mut PlatformSink,
    ring: &Ring,
    cur: &mut (u32, u32),
    track: &Option<Track>,
) -> bool {
    if track.is_none() {
        return false;
    }
    let want = format_of(track);
    if want == *cur {
        return false;
    }
    sink.stop();
    *sink = PlatformSink::new(ring.reader(), want.0, want.1);
    if let Err(e) = sink.start() {
        eprintln!("mzk: audio unavailable: {e}");
    }
    ring.writer().clear();
    *cur = want;
    true
}

fn run(playlist: Vec<PathBuf>, settings: Settings, rx: Receiver<Command>, status: Arc<Mutex<Status>>, acked: Arc<AtomicU64>) {
    let ring = Ring::new(RING_CAP);
    let writer = ring.writer();
    let mut consumed: u64 = 0;

    let n = playlist.len();
    let canon: Vec<PathBuf> = playlist.iter().map(|p| canon_path(p)).collect();
    let mut favorites = load_favorites();
    let mut shuffle = settings.shuffle;
    let mut fav_only = settings.fav_only;
    let mut seed = seed_now();
    let mut repeat = settings.repeat;
    let mut vol = settings.vol;
    let mut paused = false;
    let mut shown_idx = usize::MAX;

    if fav_only && !canon.iter().any(|p| favorites.contains(p)) {
        eprintln!("mzk: no favorites among loaded tracks");
        fav_only = false;
    }
    let mut order = build_order(&playlist, &canon, &favorites, shuffle, fav_only, seed);
    let mut order_pos = 0usize;

    let mut track = open_index(&playlist, order[order_pos], &status);
    update_status(&status, &playlist, &canon, &favorites, &order, order_pos, &track, vol, shuffle, fav_only, repeat, paused, 0, &mut shown_idx);

    let mut cur_format = format_of(&track);
    let mut sink = PlatformSink::new(ring.reader(), cur_format.0, cur_format.1);
    if let Err(e) = sink.start() {
        eprintln!("mzk: audio unavailable: {e}");
    }
    sink.set_volume(vol);

    let mut pending: Vec<f32> = Vec::new();
    let mut pushed: u64 = 0;

    loop {
        let mut quit = false;
        while let Ok(cmd) = rx.try_recv() {
            consumed += 1;
            match cmd {
                Command::Quit => {
                    quit = true;
                }
                Command::Pause => {
                    paused = !paused;
                    sink.set_paused(paused);
                }
                Command::Vol(v) => {
                    vol = v.clamp(0.0, 1.0);
                    sink.set_volume(vol);
                }
                Command::VolDelta(d) => {
                    vol = (vol + d).clamp(0.0, 1.0);
                    sink.set_volume(vol);
                }
                Command::Shuffle(on) => {
                    let cur = order.get(order_pos).copied().unwrap_or(0);
                    shuffle = on;
                    fav_only = false;
                    seed = seed_now();
                    order = build_order(&playlist, &canon, &favorites, shuffle, fav_only, seed);
                    order_pos = order.iter().position(|&x| x == cur).unwrap_or(0);
                }
                Command::ShuffleFavorites => {
                    if !canon.iter().any(|p| favorites.contains(p)) {
                        eprintln!("mzk: no favorites among loaded tracks");
                    } else {
                        let cur = order.get(order_pos).copied().unwrap_or(0);
                        shuffle = true;
                        fav_only = true;
                        seed = seed_now();
                        order = build_order(&playlist, &canon, &favorites, shuffle, fav_only, seed);
                        order_pos = order.iter().position(|&x| x == cur).unwrap_or(0);
                    }
                }
                Command::ToggleFavorite => {
                    let idx = order.get(order_pos).copied().unwrap_or(0);
                    let key = canon[idx].clone();
                    if !favorites.remove(&key) {
                        favorites.insert(key);
                    }
                    save_favorites(&favorites);
                    if fav_only {
                        let cur = order.get(order_pos).copied().unwrap_or(0);
                        order = build_order(&playlist, &canon, &favorites, shuffle, fav_only, seed);
                        order_pos = order.iter().position(|&x| x == cur).unwrap_or(0);
                    }
                }
                Command::Repeat(r) => repeat = r,
                Command::Next => {
                    advance(&mut order_pos, order.len(), repeat, 1);
                    track = open_index(&playlist, order[order_pos], &status);
                    reopen_sink(&mut sink, &ring, &mut cur_format, &track);
                    pending.clear();
                    writer.clear();
                    sink.flush();
                    pushed = 0;
                }
                Command::Prev => {
                    advance(&mut order_pos, order.len(), repeat, -1);
                    track = open_index(&playlist, order[order_pos], &status);
                    reopen_sink(&mut sink, &ring, &mut cur_format, &track);
                    pending.clear();
                    writer.clear();
                    sink.flush();
                    pushed = 0;
                }
                Command::Play(i) => {
                    if i < n {
                        order_pos = order.iter().position(|&x| x == i).unwrap_or(0);
                        track = open_index(&playlist, order[order_pos], &status);
                        reopen_sink(&mut sink, &ring, &mut cur_format, &track);
                        pending.clear();
                        writer.clear();
                        sink.flush();
                        pushed = 0;
                        paused = false;
                    }
                }
                Command::SeekTo(secs) => {
                    if let Some(t) = track.as_mut() {
                        let rate = t.sample_rate() as u64;
                        let ch = t.channels() as u64;
                        t.seek(secs.saturating_mul(rate));
                        pending.clear();
                        writer.clear();
                        sink.flush();
                        pushed = t.pos_frames() * ch;
                    }
                }
                Command::Seek(delta) => {
                    if let Some(t) = track.as_mut() {
                        let rate = t.sample_rate() as u64;
                        let ch = t.channels() as u64;
                        let cur = t.pos_frames() as i64;
                        let tgt = cur.saturating_add(delta.saturating_mul(rate as i64)).max(0) as u64;
                        t.seek(tgt);
                        pending.clear();
                        writer.clear();
                        sink.flush();
                        pushed = t.pos_frames() * ch;
                    }
                }
            }
        }
        if quit {
            break;
        }

        let fill = (RING_CAP - writer.available()) as u64;
        let consumed_samples = pushed.saturating_sub(fill);

        let mut worked = false;
        if !paused {
            if let Some(t) = track.as_mut() {
                if pending.is_empty() {
                    match t.next() {
                        Some(s) => {
                            pending = s;
                            worked = true;
                        }
                        None => {
                            let last = order_pos;
                            advance(&mut order_pos, order.len(), repeat, 1);
                            if repeat == Repeat::Off && order_pos == last {
                                track = None;
                                set_ended(&status);
                            } else {
                                track = open_index(&playlist, order[order_pos], &status);
                            }
                            let changed = reopen_sink(&mut sink, &ring, &mut cur_format, &track);
                            pending.clear();
                            if changed {
                                writer.clear();
                                sink.flush();
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

        update_status(&status, &playlist, &canon, &favorites, &order, order_pos, &track, vol, shuffle, fav_only, repeat, paused, consumed_samples, &mut shown_idx);
        acked.store(consumed, Ordering::Release);

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
    match open(&playlist[idx]) {
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

#[allow(clippy::too_many_arguments)]
fn update_status(
    status: &Arc<Mutex<Status>>,
    playlist: &[PathBuf],
    canon: &[PathBuf],
    favorites: &HashSet<PathBuf>,
    order: &[usize],
    order_pos: usize,
    track: &Option<Track>,
    vol: f32,
    shuffle: bool,
    fav_only: bool,
    repeat: Repeat,
    paused: bool,
    consumed_samples: u64,
    shown_idx: &mut usize,
) {
    let mut s = status.lock().unwrap();
    let idx = order.get(order_pos).copied().unwrap_or(0);
    s.index = idx;
    if idx != *shown_idx {
        let p = &playlist[idx.min(playlist.len().saturating_sub(1))];
        s.name = track_name(p);
        s.ext = track_ext(p);
        *shown_idx = idx;
    }
    s.favorite = canon.get(idx).map(|p| favorites.contains(p)).unwrap_or(false);
    s.vol = vol;
    s.shuffle = shuffle;
    s.fav_only = fav_only;
    s.repeat = repeat;
    s.paused = paused;
    if let Some(t) = track {
        let rate = (t.sample_rate() as u64).max(1);
        let ch = (t.channels() as u64).max(1);
        s.rate = t.sample_rate();
        s.channels = t.channels() as u32;
        s.total = t.duration_frames() / rate;
        s.pos = (consumed_samples / ch / rate).min(s.total.max(1));
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
