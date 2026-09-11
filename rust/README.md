# RASQ M3 wasm workspace

Cargo workspace for the M3 milestone in [`../README.md`](../README.md): a
wasm plugin sample and the native runtime that hosts it inside Flutter.

- [`misskey-wasm-sample/`](misskey-wasm-sample) — a `wasm32-unknown-unknown`
  cdylib that emits `snapshot`/`patch` and answers `call` as
  `rasq.boundary/1` JSON, per [`../PROTOCOL.md`](../PROTOCOL.md) §3. This is
  the plugin side — what a real SNS plugin author would ship.
- [`mastodon-wasm/`](mastodon-wasm) — the same `sns.*` contract against a
  second SNS (D-135). It has **no screen constructor**: it only answers
  `sns.*`, and the host draws its standard PostTile. Useful as the proof
  that the boundary is SNS-agnostic — the host code is identical for both.
  The interesting differences are all inside the plugin: Mastodon serves
  HTML bodies (flattened to text here, since the host's RSL pass expects
  plain text), pages with `max_id`, has no reactions (favourites are mapped
  onto the reaction surface as ⭐) and no native quotes (`sns.quotes`
  returns C01, so the host hides that column).
- [`bluesky-wasm/`](bluesky-wasm) — the same `sns.*` contract against
  Bluesky / AT Protocol (D-175). Also handler-only (no screen constructor).
  What differs is again all inside the plugin: post ids are `at://` URIs
  (with an in-memory `uri → cid` cache for the strong refs that likes,
  reposts, replies and quotes need), every request goes to the account's
  PDS as XRPC, the session token is a `{did, handle, accessJwt, refreshJwt}`
  JSON produced by `sns.login` (credential login on the anonymous worker)
  and refreshed through `refreshSession` + `storage.safe`, rich text is
  emitted as UTF-8 byte-offset `facets`, posts are capped at 300 graphemes,
  and there are no bookmarks/lists (C01). It also carries the
  `set_host_hook` / `take_host_log` native test hook, so `cargo test`
  exercises real request/response flows against a mocked `net.fetch`.
  See its [README](bluesky-wasm/README.md).
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
cargo build --release --target wasm32-unknown-unknown -p mastodon-wasm
cargo build --release --target wasm32-unknown-unknown -p bluesky-wasm
cargo test                                        # mapping tests for all three plugins (bluesky-wasm mocks net.fetch)
cargo test --release -p rasq_wasm_runtime_core    # loads the sample above and round-trips it through wasmi
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
- [x] Host functions over the `rasq.host_fn` import (PROTOCOL §4.1, sync v1):
      wasmi serves the import, the embedder registers a callback
      (`rasq_wasm_instantiate_with_host`), Dart provides handlers per
      `schema/host-fn.schema.json` — proven wasm → Dart → wasm by the
      `storageRoundTrip` sample fn
- [ ] Run the example on a physical iPhone and measure interpreter throughput
      (D-007: compare `wasmi` vs `wasm3`/WAMR for binary size + perf)
- [x] Real plugin logic (talk to a Misskey instance) instead of the
      hardcoded sample snapshot/patch/call payloads; host functions
      (`net.fetch`, `storage.*`) exposed to the wasm side
- [x] `sns.*` boundary covering the `SnsSession` contract: timelines (all
      `TimelineKind.id`s incl. `list:` / `tag:`), threads, actions, posting,
      users, reactions, emoji — plus notifications, follow/mute/block
      relations, DM rooms, search and trends (D-134). See
      [`../PROTOCOL.md`](../PROTOCOL.md) §4.3 for the wire.
- [x] `uploadMedia` — `net.fetch` now carries binary both ways (`body: {b64}`
      in, `bodyB64` out) and the plugins build the multipart body themselves,
      since the field names and auth placement differ per SNS (D-136)
- [x] `updateProfile`, `users`, `reactedPosts`, lists; clips / Plays on
      Misskey (Mastodon has neither, so those answer C01 and the host hides
      the tabs)
