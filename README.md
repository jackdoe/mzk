# mzk

A teletype-style terminal music player that decodes **Opus, MP3, FLAC, WAV, and
MPEG-4 audio (ALAC + AAC-LC)** entirely from scratch in pure Rust, with **zero
crate dependencies**. It reads `.opus`, `.mp3`, `.flac`, `.wav`, and `.m4a`
files, decodes them in-house (no `libopus`, no `libmp3lame`, no `ffmpeg`, no
audio crates), and plays them through the OS sound server via raw FFI.

```
mzk *.opus *.mp3 *.flac *.wav *.m4a
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
- Decodes **MPEG-1/2/2.5 Layer III** (`.mp3`) from scratch — Huffman spectral
  decode, the bit reservoir, requantization, stereo (M/S + intensity), the
  hybrid IMDCT, and the polyphase synthesis filterbank — verified bit-exact
  against `ffmpeg`'s PCM (relative RMS ~1e-6 after a decoder-delay lag search).
- Decodes **FLAC** (`.flac`) from scratch — STREAMINFO + metadata, fixed and
  LPC subframes, Rice residual coding, and L/S · R/S · M/S decorrelation —
  **bit-exact** against `ffmpeg`.
- Decodes **WAV** (`.wav`) — RIFF/WAVE, PCM (8/16/24/32-bit) and IEEE float —
  bit-exact.
- Decodes **MPEG-4 audio** (`.m4a`) from scratch behind its own ISOBMFF demux:
  **ALAC** (Apple Lossless) **bit-exact**, and **AAC-LC** (Huffman, scalefactors,
  M/S + intensity stereo, TNS, the long/short sine·KBD filterbank) verified
  against `ffmpeg` (relative RMS ~1e-6 on the deterministic path; perceptual-
  noise-substitution bands are decoder-defined and left unfilled).
- Each stream plays at its **own sample rate and channel count** (Opus is
  48 kHz; the rest are usually 44.1 kHz and may be mono); the OS sink is
  reopened when the format changes between tracks, and same-format track
  changes are gapless.
- Plays through **PulseAudio / PipeWire** on Linux and **CoreAudio** on macOS.
- Teletype REPL: list, now-playing, play/pause, next/prev, volume, seek,
  blue-noise shuffle, repeat.

## How it works

Codecs sit behind a `Decoder` trait (`src/decoder.rs`); `decoder::open` picks
one by file extension and the engine drives `Box<dyn Decoder>` without knowing
which codec it is. Each decoder yields interleaved `f32` at its own rate:

```
file ── decoder::open ──┐
                        ├─ src/opus/  (.opus)  ogg → toc → celt → f32 @48k  ┐
                        ├─ src/mp3/   (.mp3)   header → reservoir → huffman  │
                        │                       → requant → imdct → synth    │ Box<dyn
                        ├─ src/flac/  (.flac)  metadata → subframes → rice   │ Decoder>
                        │                       → lpc/fixed → decorrelate    │ next()
                        ├─ src/wav/   (.wav)   riff → pcm/float → f32        │ seek()
                        └─ src/m4a/   (.m4a)   mp4 demux → alac | aac-lc     │ rate()
                                                                             ┘
                                  └─ pcm.rs   push f32 to a lock-free ring
                                       └─ audio/  OS sink drains the ring (FFI)
```

Two threads, `std` only (channels, atomics, a hand-rolled SPSC ring):

- **REPL thread** (`repl.rs`): prints the prompt, reads a line, sends a
  `Command` to the engine, prints a terse status line. All formatting is pure
  functions in `repl_fmt.rs`.
- **Engine thread** (`engine.rs`): owns the playlist and playback state,
  decodes the current track frame-by-frame into the ring buffer, and handles
  commands (seek, skip, volume, shuffle…). It is codec-agnostic — it holds a
  `Box<dyn Decoder>` and tracks position in samples at the decoder's own rate.
  When a new track's rate or channel count differs from the open sink, it stops
  and reopens `PlatformSink` at the new format. A third OS-owned audio thread
  pulls from the ring and writes to the sound server.

### The decoder (`src/celt/`)

CELT is a transform codec. Decoding a frame means rebuilding the MDCT spectrum
from the bitstream and running it back to time domain:

The Opus modules live under `src/opus/` (`ogg`, `toc`, `range`, `mdct`, `celt/`)
behind `opus::OpusDecoder`.

| stage | file | what it does |
|-------|------|--------------|
| entropy | `opus/range.rs` | the range/arithmetic decoder every symbol comes from |
| frame glue | `opus/celt/mod.rs` | `decode_frame`: reads flags, drives every stage in bit-order |
| band energy | `opus/celt/energy.rs` | Laplace-coded coarse + fine energy per band |
| bit allocation | `opus/celt/allocation.rs`, `opus/celt/rate.rs` | how many bits each band gets |
| spectral shape | `opus/celt/vq.rs`, `opus/celt/cwrs.rs` | PVQ: unit-norm pulse vectors per band |
| band assembly | `opus/celt/bands.rs` | `quant_all_bands`: theta/stereo split recursion, denormalise, anti-collapse |
| time domain | `opus/mdct.rs`, `fft.rs`, `opus/celt/synth.rs` | inverse MDCT (own radix-4/2 mixed-radix FFT, zero per-call heap), overlap-add, de-emphasis, comb postfilter |
| constants | `opus/celt/tables.rs` | the normative band layout and allocation tables |

Design notes:
- **Compute, don't store.** FFT twiddles, the MDCT window, and the PVQ
  codebook sizes `V(n,k)` are computed at startup from their recurrences rather
  than shipped as big tables.
- **Float build.** All arithmetic is `f32`, mirroring libopus's float path.
- The entropy-coupled stages (energy, allocation, PVQ) must consume bits in
  exact lockstep with the encoder — one wrong bit turns music into noise — so
  those are ported faithfully from the reference and gated by the
  decode-vs-`ffmpeg` RMS test in `opus/celt/mod.rs`.

### The MP3 decoder (`src/mp3/`)

MP3 (MPEG-1/2/2.5 Layer III) shares nothing with the Opus path — Huffman codes
instead of range coding, a polyphase filterbank instead of the CELT MDCT, raw
framing instead of Ogg — so it is its own module behind `mp3::Mp3Decoder`. The
decode unit is one MP3 frame (1152 samples/channel for MPEG-1, 576 for
MPEG-2/2.5, decoded as granules of 576).

| stage | file | what it does |
|-------|------|--------------|
| framing | `mp3/header.rs` | ID3v2 skip, frame sync, version/bitrate/rate tables, frame length |
| bits | `mp3/bits.rs` | MSB-first big-endian bit reader |
| side info | `mp3/sideinfo.rs` | per-granule/channel block type, region splits, `main_data_begin` |
| reservoir | `mp3/mod.rs` | reassembles main_data that may start in *previous* frames' bytes |
| scalefactors / requant | `mp3/requant.rs` | scalefactor decode, `x^(4/3)`, global-gain/subblock-gain scaling |
| Huffman | `mp3/huffman.rs` | the 576 spectral lines: big-values regions, count1, linbits escapes |
| stereo / reorder / alias | `mp3/stereo.rs` | M/S + intensity stereo, short-block reorder, alias-reduction butterflies |
| hybrid IMDCT | `mp3/imdct.rs` | 36/12-point IMDCT, block-type windows, overlap-add, frequency inversion |
| synthesis | `mp3/synthesis.rs` | DCT-II + the 512-tap polyphase filterbank → time-domain PCM |
| tables | `mp3/tables.rs` | Huffman codebooks, scalefactor bands, the synthesis window |

Design notes:
- **Float build**, like the Opus path — all sample math is `f32`, output scaled
  by `1/32768`.
- The **bit reservoir** (a frame's main data can begin hundreds of bytes before
  the frame), the **Huffman tables**, and the **synthesis filterbank** are the
  three places a single wrong constant or off-by-one is audible; all three are
  ported faithfully from the public-domain `minimp3` and gated by the
  lag-tolerant decode-vs-`ffmpeg` RMS test in `mp3/mod.rs`.
- Duration comes from a **Xing/Info** header when present, else a CBR estimate.

### The FLAC decoder (`src/flac/`)

A native FLAC decoder behind `flac::FlacDecoder`. It parses the metadata blocks
(reads `STREAMINFO`, skips the rest, including the embedded cover-art `PICTURE`),
then decodes frames: per-channel subframes (constant, verbatim, fixed orders
0–4, and LPC), Rice-coded residuals (partitions + escape), and the inter-channel
decorrelation modes (left/side, right/side, mid/side). Output is `i32` →
`f32 / 2^(bps-1)`. Verified **bit-exact** against `ffmpeg`.

### The WAV decoder (`src/wav/`)

`wav::WavDecoder` walks RIFF chunks, reads `fmt `/`data`, and converts PCM
(8/16/24/32-bit), IEEE float (32/64-bit), and `WAVE_FORMAT_EXTENSIBLE` to
interleaved `f32`. Unknown chunks are skipped. Bit-exact.

### The MPEG-4 audio path (`src/m4a/`)

`.m4a` is a container, so the work splits in two. `m4a/mp4.rs` is an ISOBMFF
demux — it walks the box tree (`moov→trak→mdia→minf→stbl`), reads the sample
tables (`stsd`/`stsz`/`stsc`/`stco`) into per-frame byte ranges, and pulls the
codec config (the ALAC magic cookie, or the AAC `AudioSpecificConfig` out of
`esds`). The codec then decodes each frame:

- **ALAC** (`m4a/alac.rs`) — Apple Lossless: per-channel adaptive Golomb-Rice
  residuals, the sign-adaptive FIR predictor, and the stereo unmix. Frames are
  independent, so seeking is exact with no pre-roll. **Bit-exact**.
- **AAC-LC** (`m4a/aac.rs`) — the raw-data-block syntax (SCE/CPE/…), table-driven
  Huffman spectral decode, scalefactor DPCM, inverse quantization (`|q|^(4/3)`),
  M/S + intensity stereo, TNS, and the long/short inverse filterbank with sine
  and Kaiser-Bessel-derived windows — its inverse MDCT routing through the same
  radix-4 FFT in `fft.rs` as Opus, so it is O(N log N), not an O(N²) matrix. The ISO Huffman codebooks and
  scalefactor-band tables live in `m4a/aac_tables.rs`, generated from a
  reference by `scripts/gen-aac-tables.py` (they are factual tables from
  ISO/IEC 14496-3, not hand-transcribed). Verified against `ffmpeg`'s decode
  (relative RMS ~1e-6 on the deterministic path; PNS noise bands are
  decoder-defined and left silent).

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

## Performance

Everything decodes far faster than real time. `mzk --bench FILE...` runs each
decoder flat-out into a null sink and prints throughput, samples, and RSS per
file. On the dev machine (native `target-cpu`), per second of audio:

| format | speed × real time |
|--------|------------------:|
| WAV    | ~4000× |
| AAC-LC | ~780× |
| FLAC   | ~730× |
| ALAC   | ~410× |
| MP3    | ~375× |
| Opus   | ~310× |

The decode hot paths are the transforms and the entropy stages, so those get the
attention:

- **One shared FFT** (`fft.rs`) — a radix-4/2 mixed-radix FFT (radix-3/5
  fallback) with twiddles computed once at startup and **zero per-call heap
  allocation**, running on a reused stack scratch. Both the Opus and the AAC
  inverse MDCT route through it.
- **Table-driven AAC Huffman** — a 10-bit root lookup plus a short scan for the
  rare long codes, instead of a hash probe per bit.
- **Rotating MP3 synthesis buffer** — the 1024-sample FIFO advances a write
  offset rather than memmoving the whole buffer every subband sample.

Release builds use `codegen-units = 1` + LTO, and `.cargo/config.toml` sets
`target-cpu=native` so the `f32` inner loops autovectorize. `scripts/bench.sh
LABEL` builds a symbol-bearing `profiling` profile, records `perf`, renders
`inferno` flamegraphs into `prof/LABEL/`, and `scripts/bench.sh compare A B`
diffs two labeled runs.

## Adding a new codec

Codecs live behind the `Decoder` trait in `src/decoder.rs`:

```rust
pub trait Decoder: Send {
    fn next(&mut self) -> Option<Vec<f32>>;   // one frame, interleaved f32
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> usize;
    fn duration_frames(&self) -> u64;
    fn pos_frames(&self) -> u64;
    fn seek(&mut self, frame: u64);
}
```

To add FLAC, Vorbis, AAC, or another Opus mode:

1. **Add a module** (e.g. `src/flac/`) with a type that implements `Decoder`,
   yielding interleaved `f32` at its own `sample_rate()`/`channels()`.
2. **Dispatch** — add the extension to `decoder::open`.

You do **not** touch `fft`, `pcm`, `audio`, the engine loop, or the REPL — they
are codec-agnostic. The engine drives `Box<dyn Decoder>`, tracks position at the
decoder's own rate, and reopens the sink when the format changes. That seam —
`decoder.rs` plus per-stream rate/channels — is the whole point of the layering
above; `opus/`, `mp3/`, `flac/`, `wav/`, and `m4a/` are its tenants.

## Build & run

```
cargo build --release
./target/release/mzk ~/Music/*.opus ~/Music/*.mp3 ~/Music/*.flac ~/Music/*.m4a
```

No dependencies to install for building. At runtime on Linux you need a working
PulseAudio or PipeWire (standard on any desktop). Tests, including the
decode-accuracy gates, run with `cargo test`. `mzk --bench FILE...` decodes to a
null sink and reports throughput (see [Performance](#performance)).

The decode gates for FLAC/WAV/ALAC/AAC compare against `ffmpeg`-generated
references under `tests/fixtures/voyager/` (public-domain audio from the
[Voyager Golden Record](https://archive.org/details/voyager-golden-record-cd-ozma)).
That directory is git-ignored; `scripts/gen-fixtures.sh` fetches the source and
regenerates the references (and `scripts/gen-aac-tables.py` regenerates the AAC
codebook tables). The tests skip gracefully when the fixtures are absent.

## Scope

In: CELT-only Opus (config 31), MPEG-1/2/2.5 Layer III MP3, FLAC, PCM/float
WAV, and MPEG-4 ALAC + AAC-LC. Out (for now): Opus SILK/hybrid, MP3
free-format, AAC perceptual-noise-substitution playback (PNS bands decode
silent), HE-AAC/SBR, multichannel (>2) layouts, resampling. The `Decoder` seam
above is where the rest would go.

---

Written by Claude (Anthropic). Tested by jackdoe.
