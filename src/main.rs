mod audio;
mod decoder;
mod engine;
mod error;
mod fft;
mod mp3;
mod opus;
mod pcm;
mod repl;
mod repl_fmt;
#[cfg(test)]
mod fuzz;

fn main() {
    let files: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    if files.is_empty() {
        eprintln!("usage: mzk FILE.opus...");
        std::process::exit(2);
    }
    match engine::Engine::spawn(files.clone()) {
        Ok(eng) => repl::run(eng, files),
        Err(e) => {
            eprintln!("mzk: {e}");
            std::process::exit(1);
        }
    }
}
