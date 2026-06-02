use mzk::{decoder, engine, repl};

fn usage() -> ! {
    eprintln!("usage: mzk [-s off|on|fav] [-v 0-100] [-r off|one|all] [-nd] FILE|DIR...");
    std::process::exit(2);
}

const EXTS: [&str; 5] = ["flac", "wav", "m4a", "opus", "mp3"];
const MAX_SCAN_DEPTH: u32 = 64;

fn rank(p: &std::path::Path) -> usize {
    p.extension()
        .and_then(|e| e.to_str())
        .and_then(|e| EXTS.iter().position(|&x| x == e.to_ascii_lowercase()))
        .unwrap_or(EXTS.len())
}

fn dedup(files: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    let mut seen: std::collections::HashMap<std::path::PathBuf, usize> =
        std::collections::HashMap::new();
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for f in files {
        match seen.entry(f.with_extension("")) {
            std::collections::hash_map::Entry::Occupied(e) => {
                let i = *e.get();
                if rank(&f) < rank(&out[i]) {
                    out[i] = f;
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(out.len());
                out.push(f);
            }
        }
    }
    out
}

fn scan(dir: &std::path::Path, depth: u32, out: &mut Vec<std::path::PathBuf>) {
    if depth >= MAX_SCAN_DEPTH {
        return;
    }
    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => return,
    };
    entries.sort();
    for p in entries {
        if p.is_dir() {
            scan(&p, depth + 1, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

fn add(arg: String, out: &mut Vec<std::path::PathBuf>) {
    let p = std::path::PathBuf::from(arg);
    if p.is_dir() {
        scan(&p, 0, out);
    } else {
        out.push(p);
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let n0 = args.len();
    args.retain(|a| a != "-nd" && a != "--no-dedup");
    let no_dedup = args.len() != n0;
    let finish = |files: Vec<std::path::PathBuf>| if no_dedup { files } else { dedup(files) };
    if args.first().map(String::as_str) == Some("--bench") {
        args.remove(0);
        let mut files = Vec::new();
        for a in args {
            add(a, &mut files);
        }
        bench(finish(files));
        return;
    }

    let mut settings = engine::Settings::default();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-s" | "--shuffle" => match it.next().as_deref() {
                Some("off") => settings.shuffle = false,
                Some("on") => settings.shuffle = true,
                Some("fav") => {
                    settings.shuffle = true;
                    settings.fav_only = true;
                }
                _ => usage(),
            },
            "-v" | "--vol" => match it.next().and_then(|v| v.parse::<f32>().ok()) {
                Some(p) => settings.vol = (p / 100.0).clamp(0.0, 1.0),
                None => usage(),
            },
            "-r" | "--repeat" => match it.next().as_deref() {
                Some("off") => settings.repeat = engine::Repeat::Off,
                Some("one") => settings.repeat = engine::Repeat::One,
                Some("all") => settings.repeat = engine::Repeat::All,
                _ => usage(),
            },
            _ => add(a, &mut files),
        }
    }
    let files = finish(files);
    if files.is_empty() {
        usage();
    }
    match engine::Engine::spawn(files.clone(), settings) {
        Ok(eng) => repl::run(eng, files),
        Err(e) => {
            eprintln!("mzk: {e}");
            std::process::exit(1);
        }
    }
}

fn proc_kib(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn bench(files: Vec<std::path::PathBuf>) {
    println!(
        "{:<6} {:>9} {:>10} {:>9} {:>10} {:>9} {:>9} {:>7}",
        "fmt", "audio", "decode", "speed", "samples", "file", "rss", "peak"
    );
    for path in &files {
        let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let t0 = std::time::Instant::now();
        let mut dec = match decoder::open(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let rate = dec.sample_rate().max(1);
        let ch = dec.channels().max(1);
        let mut samples: u64 = 0;
        let mut peak = 0.0f32;
        while let Some(frame) = dec.next() {
            samples += frame.len() as u64;
            for &s in &frame {
                peak = peak.max(s.abs());
            }
        }
        let el = t0.elapsed().as_secs_f64();
        let secs = samples as f64 / ch as f64 / rate as f64;
        let rss = proc_kib("VmRSS:");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("?");
        println!(
            "{:<6} {:>8.2}s {:>9.1}ms {:>8.0}x {:>10} {:>7}KiB {:>7}KiB {:>7.3}",
            ext,
            secs,
            el * 1000.0,
            secs / el,
            samples,
            bytes / 1024,
            rss,
            peak
        );
    }
    println!("peak RSS: {} KiB", proc_kib("VmHWM:"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn dedup_picks_higher_quality_in_place() {
        let files: Vec<PathBuf> = ["a/x.mp3", "a/x.opus", "a/y.opus", "a/y.flac", "b/x.mp3"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let got = dedup(files);
        let want: Vec<PathBuf> = ["a/x.opus", "a/y.flac", "b/x.mp3"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn dedup_keeps_distinct_and_unknown_extensions() {
        let files: Vec<PathBuf> = ["x.mp3", "y.mp3", "z.ogg"].iter().map(PathBuf::from).collect();
        assert_eq!(dedup(files.clone()), files);
    }

    #[test]
    fn rank_orders_lossless_above_lossy() {
        assert!(rank(Path::new("x.flac")) < rank(Path::new("x.m4a")));
        assert!(rank(Path::new("x.m4a")) < rank(Path::new("x.opus")));
        assert!(rank(Path::new("x.OPUS")) < rank(Path::new("x.mp3")));
        assert_eq!(rank(Path::new("x.ogg")), EXTS.len());
    }
}
