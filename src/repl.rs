use crate::engine::{Command, Engine, Repeat};
use crate::repl_fmt::{fmt_label, fmt_np, fmt_rate, fmt_time, parse, Parsed};
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn repeat_word(r: Repeat) -> &'static str {
    match r {
        Repeat::Off => "off",
        Repeat::One => "one",
        Repeat::All => "all",
    }
}

fn print_np(eng: &Engine) {
    let s = eng.status();
    println!(
        "{}",
        fmt_np(
            s.index + 1,
            &s.name,
            &s.ext,
            s.rate,
            s.channels,
            s.pos,
            s.total,
            s.vol,
            s.shuffle,
            repeat_word(s.repeat)
        )
    );
}

fn print_ls(eng: &Engine, names: &[String], from: Option<usize>, count: Option<usize>) {
    let s = eng.status();
    let n = names.len();
    let (start, end) = match (from, count) {
        (Some(f), Some(c)) => (f.saturating_sub(1), f.saturating_sub(1) + c),
        (Some(f), None) => (f.saturating_sub(1), f.saturating_sub(1) + 10),
        _ => {
            let lo = s.index.saturating_sub(4);
            (lo, lo + 10)
        }
    };
    let end = end.min(n);
    for i in start..end {
        let mark = if i == s.index { '*' } else { ' ' };
        let name: String = names[i].chars().take(56).collect();
        println!("{:>3}{} {}", i + 1, mark, name);
    }
}

fn help() {
    let rows = [
        ("ls [from] [count]", "list tracks (windowed around current)"),
        ("np", "now playing: track, progress, volume, modes"),
        ("play <n>", "play track number n"),
        ("pause", "toggle play / pause"),
        ("n  / next", "skip to next track"),
        ("p  / prev", "skip to previous track"),
        ("vol <0-100>", "set volume percent (e.g. vol 70)"),
        ("vol <+n|-n>", "raise / lower volume by n percent"),
        ("seek <m:ss>", "jump to a time (e.g. seek 1:30)"),
        ("seek <+n|-n>", "jump forward / back n seconds"),
        ("shuffle <on|off>", "shuffle the play order"),
        ("repeat <off|one|all>", "repeat nothing / this track / all"),
        ("help", "show this list"),
        ("q  / quit", "quit mzk"),
    ];
    for (cmd, desc) in rows {
        println!("  {:<21} {}", cmd, desc);
    }
}

pub fn run(eng: Engine, names: Vec<PathBuf>) {
    let names = Engine::names(&names);
    println!("mzk: {} tracks. type 'help'.", names.len());
    print_np(&eng);
    let stdin = std::io::stdin();
    loop {
        print!("mzk> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let parsed = match parse(&line) {
            Some(p) => p,
            None => {
                if line.trim().is_empty() {
                    continue;
                }
                println!("?");
                continue;
            }
        };
        match parsed {
            Parsed::Quit => {
                eng.send(Command::Quit);
                break;
            }
            Parsed::Help => help(),
            Parsed::Ls(f, c) => print_ls(&eng, &names, f, c),
            Parsed::Np => print_np(&eng),
            Parsed::Play(n) => {
                if let Some(n) = n {
                    eng.send(Command::Play(n.saturating_sub(1)));
                } else {
                    eng.send(Command::Pause);
                }
                print_np(&eng);
            }
            Parsed::Pause => {
                eng.send(Command::Pause);
                let s = eng.sync();
                println!("{}", if s.paused { "pause" } else { "play" });
            }
            Parsed::Next => {
                eng.send(Command::Next);
                announce(&eng);
            }
            Parsed::Prev => {
                eng.send(Command::Prev);
                announce(&eng);
            }
            Parsed::Vol(v) => {
                eng.send(Command::Vol(v));
                println!("vol {}", (v * 100.0).round() as i32);
            }
            Parsed::VolDelta(d) => {
                eng.send(Command::VolDelta(d));
                let s = eng.sync();
                println!("vol {}", (s.vol * 100.0).round() as i32);
            }
            Parsed::Seek(d) => {
                eng.send(Command::Seek(d));
                println!("seek {:+}s", d);
            }
            Parsed::SeekTo(secs) => {
                eng.send(Command::SeekTo(secs));
                println!("seek {}", fmt_time(secs));
            }
            Parsed::Shuffle(on) => {
                eng.send(Command::Shuffle(on));
                println!("shuffle {}", if on { "on" } else { "off" });
            }
            Parsed::Repeat(r) => {
                let rep = match r.as_str() {
                    "one" => Repeat::One,
                    "all" => Repeat::All,
                    _ => Repeat::Off,
                };
                eng.send(Command::Repeat(rep));
                println!("repeat {}", repeat_word(rep));
            }
        }
    }
    eng.join();
}

fn announce(eng: &Engine) {
    let s = eng.sync();
    let label = fmt_label(&truncate(&s.name, 40), &s.ext);
    let info = if s.rate > 0 {
        format!("  {} {}ch", fmt_rate(s.rate), s.channels)
    } else {
        String::new()
    };
    println!(">> now {:>2}  {}  {}{}", s.index + 1, label, fmt_time(s.total), info);
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
