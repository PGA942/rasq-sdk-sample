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

`rasq_wasm_runtime_core`'s own cdylib (for whatever Flutter is targeting) is
built automatically by the Flutter plugin's build hook
(`../rasq_wasm_runtime/hook/build.dart`) — you don't need to build it by hand
to use the Flutter plugin, only to hack on it directly or run its `cargo test`.

### Cross-compilation targets

The hook runs `cargo build --release --target <triple>` per Flutter target.
Install the rustup targets you need once:

```sh
# Apple
rustup target add aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
# Android (linker = NDK clang, handed to the hook by Flutter; NDK is auto-installed by the Flutter Gradle plugin)
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
```

| Flutter target | rust triple | extra config from the hook | status |
|---|---|---|---|
| macOS arm64 / x64 | `aarch64-apple-darwin` / `x86_64-apple-darwin` | `MACOSX_DEPLOYMENT_TARGET` | ✅ built + tested (arm64) |
| iOS device arm64 | `aarch64-apple-ios` | `IPHONEOS_DEPLOYMENT_TARGET` | wired (device build not run here — needs a signing identity) |
| iOS simulator arm64 / x64 | `aarch64-apple-ios-sim` / `x86_64-apple-ios` | same | ✅ built (universal) + run on iPhone 17 simulator |
| Android arm64 / arm / x64 / ia32 | `aarch64-linux-android` / `armv7-linux-androideabi` / `x86_64-linux-android` / `i686-linux-android` | `CARGO_TARGET_*_LINKER` = NDK `<triple><api>-clang`, `-z max-page-size=16384` | ✅ built (arm64/arm/x64 via `flutter build apk`) |
| Linux host | `*-unknown-linux-gnu` | — | wired, not run here |
| Windows host | `*-pc-windows-msvc` | — | wired, not run here (no Windows box) |

## Status / follow-ups (M3)

- [x] `snapshot`/`patch` emitted from wasm, `call` answered by wasm (minimal
      `openThread` handler) — `misskey-wasm-sample`
- [x] `wasmi` embedded in a native runtime and driven from Dart via FFI,
      proven end-to-end through an actual Flutter app (macOS) — see
      `../rasq_wasm_runtime/`
- [x] Cross-compilation wiring in `rasq_wasm_runtime/hook/build.dart` for
      macOS / iOS (device + simulator) / Android (all ABIs) / Linux / Windows
- [ ] Run the example on a physical iPhone and measure interpreter throughput
      (D-007: compare `wasmi` vs `wasm3`/WAMR for binary size + perf)
- [ ] Real plugin logic (talk to a Misskey instance) instead of the
      hardcoded sample snapshot/patch/call payloads; host functions
      (`net.fetch`, `storage.*`) exposed to the wasm side
