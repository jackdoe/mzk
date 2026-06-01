# mzk

A teletype-style terminal music player that decodes Opus entirely from scratch
in pure Rust, with **zero crate dependencies**. It reads `.opus` files, decodes
them in-house (no `libopus`, no `ffmpeg`, no audio crates), and plays them
through the OS sound server via raw FFI.

```
mzk *.opus
```

You get a 1979-style command prompt — append-only, no curses, no escape codes,
usable on a real teletype:

```
mzk> np
02  aurora  [#######---] 2:31/4:51  vol70 shuf+ repa
mzk> seek 1:30
mzk> n
```

Type `help` for the command list.

## What it does

- Decodes **CELT-mode Opus** (the mode every music `.opus` from `opusenc` /
  `ffmpeg` / YouTube uses: 48 kHz, stereo, 20 ms frames) bit-accurately —
  verified to match `ffmpeg`'s PCM within a relative RMS of ~0.001.
- Plays through **PulseAudio / PipeWire** on Linux and **CoreAudio** on macOS.
- Teletype REPL: list, now-playing, play/pause, next/prev, volume, seek,
  blue-noise shuffle, repeat.

## How it works

The pipeline, per packet:

```
.opus file
  └─ ogg.rs        demux Ogg pages → Opus packets, read OpusHead/granule
       └─ toc.rs   read the TOC byte → which codec config this packet is
            └─ celt/  decode one frame to 48 kHz stereo f32 PCM
                 └─ pcm.rs     apply gain + volume, push f32 to a lock-free ring
                      └─ audio/  OS sound server drains the ring as float (FFI)
```

Two threads, `std` only (channels, atomics, a hand-rolled SPSC ring):

- **REPL thread** (`repl.rs`): prints the prompt, reads a line, sends a
  `Command` to the engine, prints a terse status line. All formatting is pure
  functions in `repl_fmt.rs`.
- **Engine thread** (`engine.rs`): owns the playlist and playback state,
  decodes the current track frame-by-frame into the ring buffer, and handles
  commands (seek, skip, volume, shuffle…). A third OS-owned audio thread pulls
  from the ring and writes to the sound server.

### The decoder (`src/celt/`)

CELT is a transform codec. Decoding a frame means rebuilding the MDCT spectrum
from the bitstream and running it back to time domain:

| stage | file | what it does |
|-------|------|--------------|
| entropy | `range.rs` | the range/arithmetic decoder every symbol comes from |
| frame glue | `celt/mod.rs` | `decode_frame`: reads flags, drives every stage in bit-order |
| band energy | `celt/energy.rs` | Laplace-coded coarse + fine energy per band |
| bit allocation | `celt/allocation.rs`, `celt/rate.rs` | how many bits each band gets |
| spectral shape | `celt/vq.rs`, `celt/cwrs.rs` | PVQ: unit-norm pulse vectors per band |
| band assembly | `celt/bands.rs` | `quant_all_bands`: theta/stereo split recursion, denormalise, anti-collapse |
| time domain | `celt/mdct.rs`, `celt/fft.rs`, `celt/synth.rs` | inverse MDCT (own mixed-radix FFT), overlap-add, de-emphasis, comb postfilter |
| constants | `celt/tables.rs` | the normative band layout and allocation tables |

Design notes:
- **Compute, don't store.** FFT twiddles, the MDCT window, and the PVQ
  codebook sizes `V(n,k)` are computed at startup from their recurrences rather
  than shipped as big tables.
- **Float build.** All arithmetic is `f32`, mirroring libopus's float path.
- The entropy-coupled stages (energy, allocation, PVQ) must consume bits in
  exact lockstep with the encoder — one wrong bit turns music into noise — so
  those are ported faithfully from the reference and gated by the
  decode-vs-`ffmpeg` RMS test in `celt/mod.rs`.

### Audio (`src/audio/`)

`PlatformSink` is selected at compile time. On Linux it's `pulse.rs`, which
`dlopen`s `libpulse-simple.so.0` and uses the PulseAudio "simple" API — this
works on both PulseAudio and PipeWire (via `pipewire-pulse`), and needs no
`-dev` packages. A small `pa_buffer_attr` keeps latency ~80 ms; seek flushes
both the ring and the server buffer so it lands instantly. macOS uses
`coreaudio.rs` (AudioQueue).

### Shuffle is blue noise

`shuffle on` does **not** use a uniform random permutation (that clumps the
same artist together — "white noise"). Instead each artist's tracks are spread
evenly across the playlist with a random phase, so artists are interleaved and
never cluster — a blue-noise distribution. See `shuffle_order` in `engine.rs`.

## Adding a new codec or Opus configuration

Today the decoder is wired for one Opus configuration (CELT, fullband, 20 ms,
the one all music `.opus` use); `toc::Config::parse` hard-errors on anything
else, on purpose. Everything *except* the CELT math is already codec-agnostic.
To add SILK, hybrid, or other CELT frame sizes/bandwidths:

1. **`src/toc.rs`** — extend `Config::parse` to recognise the new TOC configs
   instead of erroring, returning the mode, bandwidth, frame size, and
   frame-count for the packet.
2. **A decoder for it** — for other CELT sizes, just build a different
   `celt::Mode` (it is already parameterised by sample rate, frame size, and
   band layout); the energy/allocation/PVQ/MDCT code is generic over `Mode`.
   For SILK, add a `src/silk/` module with its own `decode_frame`.
3. **Dispatch** — `Track::next` in `engine.rs` calls `celt::decode_frame`
   unconditionally; change it to pick the decoder based on the `Config`.

You do **not** touch `ogg`, `range`, `fft`, `mdct`, `pcm`, `audio`, the engine
loop, or the REPL — they are independent of the codec. That separation is the
whole point of the layering above.

## Build & run

```
cargo build --release
./target/release/mzk ~/Music/*.opus
```

No dependencies to install for building. At runtime on Linux you need a working
PulseAudio or PipeWire (standard on any desktop). Tests, including the
decode-accuracy gate, run with `cargo test`.

## Scope

In: CELT-only Opus (config 31). Out (for now): SILK, hybrid, other frame sizes,
resampling, non-Opus formats. The seam above is where they'd go.

---

Written by Claude (Anthropic). Tested by jackdoe.
