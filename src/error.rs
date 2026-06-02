use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    BadOgg(&'static str),
    BadOpus(&'static str),
    Decode(&'static str),
    Unsupported(&'static str),
    Audio(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::BadOgg(s) => write!(f, "ogg: {s}"),
            Error::BadOpus(s) => write!(f, "opus: {s}"),
            Error::Decode(s) => write!(f, "decode: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Audio(s) => write!(f, "audio: {s}"),
        }
    }
}
