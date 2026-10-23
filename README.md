# Synthesizer

A small command-line sine-wave tone generator. It plays a note through the
default audio output for two seconds, using A4 = 440 Hz.

```sh
cargo run -- c
cargo run -- 'f#'
cargo run -- bb3
cargo run -- a4
```

Notes without an octave default to octave 4. Note names are case-insensitive;
use `#` for sharps and `b` for flats.

## Development

Format Rust code with `cargo fmt --all`. Check formatting and run tests with:

```sh
cargo fmt --all -- --check
cargo test --locked
```

The formatting check also runs in GitHub Actions for pushes and pull requests.
