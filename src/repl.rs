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
        ("?", "help", "show this list"),
        ("l", "ls [from] [count]", "list tracks (windowed around current)"),
        ("c", "np", "now playing: track, format, progress, modes"),
        ("g", "play <n>   (or just <n>)", "play track number n"),
        (".", "pause", "toggle play / pause"),
        ("n", "next", "skip to next track"),
        ("p", "prev", "skip to previous track"),
        ("v", "vol <0-100|+n|-n>", "set or adjust volume percent"),
        ("k", "seek <m:ss|+n|-n>", "jump to a time, or by n seconds"),
        ("s", "shuffle [on|off]", "toggle, or set, shuffle order"),
        ("r", "repeat [off|one|all]", "cycle, or set, repeat mode"),
        ("q", "quit", "quit mzk"),
    ];
    for (key, cmd, desc) in rows {
        println!("  [{}] {:<25} {}", key, cmd, desc);
    }
}

pub fn run(eng: Engine, names: Vec<PathBuf>) {
    let names = Engine::names(&names);
    println!("mzk: {} tracks. type 'help' or '?'.", names.len());
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
            Parsed::ShuffleToggle => {
                let on = !eng.status().shuffle;
                eng.send(Command::Shuffle(on));
                println!("shuffle {}", if on { "on" } else { "off" });
            }
            Parsed::RepeatCycle => {
                let next = match eng.status().repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
                eng.send(Command::Repeat(next));
                println!("repeat {}", repeat_word(next));
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
