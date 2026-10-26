# Synthesizer

A small command-line sine-wave tone generator. It plays a note through the
default audio output for two seconds, using A4 = 440 Hz.

```sh
cargo run -- c
cargo run -- 'f#'
cargo run -- bb3
cargo run -- a4
cargo run -- midi
```

Notes without an octave default to octave 4. Note names are case-insensitive;
use `#` for sharps and `b` for flats.

MIDI mode automatically connects to the primary musical input on an AKAI MPK
when available (or to the only MIDI input device), plays notes while their
keys are held, and opens a window with buttons to select sine, sawtooth, or
pulse oscillators and a volume slider. Close the window to exit. Notes use
short attack and release envelopes to avoid clicks. If there are multiple MIDI
inputs and no AKAI input is detected, the available device names are shown in
the error message.

## Development

Format Rust code with `cargo fmt --all`. Check formatting and run tests with:

```sh
cargo fmt --all -- --check
cargo test --locked
```

The formatting check also runs in GitHub Actions for pushes and pull requests.
