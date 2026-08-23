# RASQ M3 wasm workspace

Cargo workspace for the M3 milestone in [`../README.md`](../README.md): a
wasm plugin sample and the native runtime that hosts it inside Flutter.

- [`misskey-wasm-sample/`](misskey-wasm-sample) — a `wasm32-unknown-unknown`
  cdylib that emits `snapshot`/`patch` and answers `call` as
  `rasq.boundary/1` JSON, per [`../PROTOCOL.md`](../PROTOCOL.md) §3. This is
  the plugin side — what a real SNS plugin author would ship.
- [`rasq_wasm_runtime_core/`](rasq_wasm_runtime_core) — a native cdylib that
  embeds [`wasmi`](https://crates.io/crates/wasmi) (a pure-Rust WASM
  *interpreter*, chosen per D-007 because iOS forbids JIT'ing runtimes) and
  exposes a small C ABI for loading a wasm module and driving its
  snapshot/patch/call exports. This is the host side, consumed by the
  Flutter plugin at [`../rasq_wasm_runtime/`](../rasq_wasm_runtime).

## Building

```sh
rustup target add wasm32-unknown-unknown   # once
cargo build --release --target wasm32-unknown-unknown -p misskey-wasm-sample
cargo test --release -p rasq_wasm_runtime_core   # loads the sample above and round-trips it through wasmi
```

`rasq_wasm_runtime_core`'s own cdylib/staticlib (for the host's native
architecture) is built automatically by the Flutter plugin's build hook
(`../rasq_wasm_runtime/hook/build.dart`) — you don't need to build it by hand
to use the Flutter plugin, only to hack on it directly or run its `cargo
test`.

## Status / follow-ups (M3)

- [x] `snapshot`/`patch` emitted from wasm, `call` answered by wasm (minimal
      `openThread` handler) — `misskey-wasm-sample`
- [x] `wasmi` embedded in a native runtime and driven from Dart via FFI,
      proven end-to-end through an actual Flutter app (macOS) — see
      `../rasq_wasm_runtime/`
- [ ] Android/iOS/Windows cross-compilation wiring in
      `rasq_wasm_runtime/hook/build.dart` (currently macOS/Linux host builds
      only — no NDK/xcframework/MSVC toolchain set up yet)
- [ ] Compare `wasmi` against `wasm3`/WAMR on iOS for the D-007 runtime
      selection (interpreter perf, binary size)
- [ ] Real plugin logic (talk to a Misskey instance) instead of the
      hardcoded sample snapshot/patch/call payloads
