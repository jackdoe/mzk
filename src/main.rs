#![deny(unsafe_op_in_unsafe_fn)]

mod audio;
mod decoder;
mod engine;
mod error;
mod fft;
mod flac;
mod m4a;
mod mp3;
mod opus;
mod pcm;
mod repl;
mod repl_fmt;
mod wav;
#[cfg(test)]
mod fuzz;

fn usage() -> ! {
    eprintln!("usage: mzk [-s off|on|fav] [-v 0-100] [-r off|one|all] FILE...");
    std::process::exit(2);
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--bench") {
        args.remove(0);
        bench(args.into_iter().map(Into::into).collect());
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
            _ => files.push(a.into()),
        }
    }
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
