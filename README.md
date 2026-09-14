# Lexift

Lexift is a native cross-platform translation application built with Rust and Slint.

Run the production shell without development adapters with:

```console
cargo run
```

Run the M1 mock translation demo explicitly with:

```console
cargo run --features m1-demo
```

The default build does not install mock platform or translation adapters. Until the first real
provider is implemented, translation attempts return a configuration error while the app remains
running.
