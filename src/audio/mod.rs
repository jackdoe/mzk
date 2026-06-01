#[cfg(target_os = "linux")]
mod pulse;
#[cfg(target_os = "macos")]
mod coreaudio;

#[cfg(target_os = "linux")]
pub use pulse::PulseSink as PlatformSink;
#[cfg(target_os = "macos")]
pub use coreaudio::CoreAudioSink as PlatformSink;
