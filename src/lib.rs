#![deny(unsafe_op_in_unsafe_fn)]

pub mod audio;
pub mod decoder;
pub mod engine;
pub mod error;
pub mod fft;
pub mod flac;
pub mod m4a;
pub mod mp3;
pub mod opus;
pub mod pcm;
pub mod repl;
pub mod repl_fmt;
pub mod wav;
#[cfg(test)]
mod fuzz;
