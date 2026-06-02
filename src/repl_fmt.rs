pub fn fmt_time(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{}:{:02}", m, s)
}

pub fn fmt_bar(pos: u64, total: u64, width: usize) -> String {
    let filled = if total == 0 {
        0
    } else {
        let f = (pos * width as u64 / total) as usize;
        if f > width {
            width
        } else {
            f
        }
    };
    let mut s = String::with_capacity(width + 2);
    s.push('[');
    for i in 0..width {
        s.push(if i < filled { '#' } else { '-' });
    }
    s.push(']');
    s
}

fn repeat_flag(repeat: &str) -> char {
    match repeat {
        "all" => 'a',
        "one" => '1',
        _ => '-',
    }
}

pub fn fmt_rate(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{}k", rate / 1000)
    } else {
        format!("{:.1}k", rate as f64 / 1000.0)
    }
}

pub fn fmt_label(name: &str, ext: &str) -> String {
    if ext.is_empty() {
        name.to_string()
    } else {
        format!("{}.{}", name, ext)
    }
}

pub fn fmt_np(
    index: usize,
    name: &str,
    ext: &str,
    rate: u32,
    channels: u32,
    pos: u64,
    total: u64,
    vol: f32,
    shuffle: bool,
    repeat: &str,
) -> String {
    let bar = fmt_bar(pos, total, 10);
    let vol_pct = (vol * 100.0).round() as i64;
    let shuf = if shuffle { "shuf+" } else { "shuf-" };
    let info = if rate > 0 {
        format!("{} {}ch  ", fmt_rate(rate), channels)
    } else {
        String::new()
    };
    let tail = format!(
        "{} {}/{}  {}vol{} {} rep{}",
        bar,
        fmt_time(pos),
        fmt_time(total),
        info,
        vol_pct,
        shuf,
        repeat_flag(repeat)
    );
    let head = format!("{:02}  ", index);
    let fixed = head.chars().count() + 2 + tail.chars().count();
    let budget = if fixed >= 79 { 0 } else { 79 - fixed };
    let truncated: String = fmt_label(name, ext).chars().take(budget).collect();
    format!("{}{}  {}", head, truncated, tail)
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    Ls(Option<usize>, Option<usize>),
    Np,
    Play(Option<usize>),
    Pause,
    Next,
    Prev,
    Vol(f32),
    VolDelta(f32),
    Seek(i64),
    SeekTo(u64),
    Shuffle(bool),
    Repeat(String),
    Help,
    Quit,
}

fn parse_clock(s: &str) -> Option<u64> {
    if let Some((m, sec)) = s.split_once(':') {
        let m: u64 = m.parse().ok()?;
        let sec: u64 = sec.parse().ok()?;
        Some(m * 60 + sec)
    } else {
        s.parse().ok()
    }
}

pub fn parse(line: &str) -> Option<Parsed> {
    let lower = line.trim().to_lowercase();
    let mut it = lower.split_whitespace();
    let cmd = it.next()?;
    let a = it.next();
    let b = it.next();
    match cmd {
        "ls" => Some(Parsed::Ls(
            a.and_then(|x| x.parse().ok()),
            b.and_then(|x| x.parse().ok()),
        )),
        "np" => Some(Parsed::Np),
        "play" => Some(Parsed::Play(a.and_then(|x| x.parse().ok()))),
        "pause" => Some(Parsed::Pause),
        "n" | "next" => Some(Parsed::Next),
        "p" | "prev" => Some(Parsed::Prev),
        "vol" => {
            let v = a?;
            if let Some(rest) = v.strip_prefix('+') {
                Some(Parsed::VolDelta(rest.parse::<f32>().ok()? / 100.0))
            } else if v.starts_with('-') {
                Some(Parsed::VolDelta(v.parse::<f32>().ok()? / 100.0))
            } else {
                Some(Parsed::Vol(v.parse::<f32>().ok()? / 100.0))
            }
        }
        "seek" => {
            let v = a?;
            if v.starts_with('+') {
                Some(Parsed::Seek(v[1..].parse::<i64>().ok()?))
            } else if v.starts_with('-') {
                Some(Parsed::Seek(v.parse::<i64>().ok()?))
            } else {
                Some(Parsed::SeekTo(parse_clock(v)?))
            }
        }
        "shuffle" => match a? {
            "on" => Some(Parsed::Shuffle(true)),
            "off" => Some(Parsed::Shuffle(false)),
            _ => None,
        },
        "repeat" => Some(Parsed::Repeat(a?.to_string())),
        "help" => Some(Parsed::Help),
        "q" | "quit" => Some(Parsed::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time() {
        assert_eq!(fmt_time(151), "2:31");
        assert_eq!(fmt_time(0), "0:00");
        assert_eq!(fmt_time(605), "10:05");
    }

    #[test]
    fn bar() {
        let b = fmt_bar(151, 291, 10);
        assert_eq!(b.len(), 12);
        assert!(b.starts_with('['));
        assert!(b.ends_with(']'));
        assert_eq!(fmt_bar(10, 0, 10), "[----------]");
    }

    #[test]
    fn np() {
        let s = fmt_np(2, "aurora", "flac", 44100, 2, 151, 291, 0.7, true, "off");
        assert!(s.len() <= 79);
        assert!(s.contains("2:31/4:51"));
        assert!(s.contains("aurora.flac"));
        assert!(s.contains("44.1k 2ch"));
        let long = fmt_np(2, &"x".repeat(200), "opus", 48000, 2, 151, 291, 0.7, true, "off");
        assert!(long.len() <= 79);
        assert!(long.contains("48k 2ch"));
        let no_track = fmt_np(1, "song", "mp3", 0, 0, 0, 0, 1.0, false, "all");
        assert!(no_track.contains("song.mp3"));
        assert!(!no_track.contains("0ch"));
    }

    #[test]
    fn rate() {
        assert_eq!(fmt_rate(48000), "48k");
        assert_eq!(fmt_rate(44100), "44.1k");
        assert_eq!(fmt_rate(16000), "16k");
    }

    #[test]
    fn vol_delta() {
        match parse("vol +10").unwrap() {
            Parsed::VolDelta(d) => assert!((d - 0.10).abs() < 1e-6),
            _ => panic!(),
        }
        match parse("vol -10").unwrap() {
            Parsed::VolDelta(d) => assert!((d + 0.10).abs() < 1e-6),
            _ => panic!(),
        }
        match parse("vol 70").unwrap() {
            Parsed::Vol(v) => assert!((v - 0.70).abs() < 1e-6),
            _ => panic!(),
        }
    }

    #[test]
    fn seek() {
        assert_eq!(parse("seek 1:30"), Some(Parsed::SeekTo(90)));
        assert_eq!(parse("seek 90"), Some(Parsed::SeekTo(90)));
        assert_eq!(parse("seek +15"), Some(Parsed::Seek(15)));
        assert_eq!(parse("seek -15"), Some(Parsed::Seek(-15)));
    }

    #[test]
    fn commands() {
        assert_eq!(parse("n"), Some(Parsed::Next));
        assert_eq!(parse("ls 20 5"), Some(Parsed::Ls(Some(20), Some(5))));
        assert_eq!(parse("ls"), Some(Parsed::Ls(None, None)));
        assert_eq!(parse("q"), Some(Parsed::Quit));
        assert_eq!(parse("QUIT"), Some(Parsed::Quit));
        assert_eq!(parse("repeat all"), Some(Parsed::Repeat("all".into())));
        assert_eq!(parse("shuffle on"), Some(Parsed::Shuffle(true)));
        assert_eq!(parse("bogus"), None);
    }
}
