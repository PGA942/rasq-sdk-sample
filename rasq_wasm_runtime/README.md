# rasq_wasm_runtime

Flutter FFI plugin that runs a RASQ SNS plugin (a `wasm32-unknown-unknown`
module speaking the `rasq.boundary/1` protocol — see [`../PROTOCOL.md`](../PROTOCOL.md))
inside the host app, via an embedded [`wasmi`](https://crates.io/crates/wasmi)
interpreter. `wasmi` was picked over a JIT'ing runtime because iOS forbids
JIT (D-007); the same engine runs unchanged on every platform.

The wasm interpreter itself lives in Rust at
[`../rust/rasq_wasm_runtime_core`](../rust/rasq_wasm_runtime_core); this
package only wires it into Flutter's native-assets build (`hook/build.dart`)
and exposes a small Dart API (`RasqWasmPlugin` in `lib/rasq_wasm_runtime.dart`).

## Status

`hook/build.dart` shells out to `cargo build --release --target <triple>` for
whatever Flutter is building (macOS / iOS device + simulator / Android all
ABIs / Linux / Windows hosts) and bundles the resulting dylib/so/dll as this
package's code asset. Flutter then wraps it into a framework (Apple) or
`jniLibs` (Android) like any other native asset — no Podfile / CMake /
xcframework of our own.

Verified here:

- macOS arm64 — end-to-end (wasm plugin → wasmi → Dart FFI → Flutter widget
  tree) by `example`'s `flutter test` and a built `.app`
- iOS simulator arm64 — `flutter build ios --simulator`
- Android arm64 / armv7 / x86_64 — `flutter build apk` (NDK clang is handed to
  the hook by Flutter; the Flutter Gradle plugin auto-installs the NDK)

Linux/Windows are mapped but have not been exercised (no such host here).
Missing rustup targets fail the build with the exact `rustup target add …`
line to run. See `../rust/README.md` for the full target matrix.

## Usage

```dart
import 'package:rasq_wasm_runtime/rasq_wasm_runtime.dart';

final plugin = RasqWasmPlugin.fromBytes(wasmBytes);
final snapshotJson = plugin.snapshot();          // rasq.boundary/1 `snapshot`
final patchJson = plugin.patch();                // rasq.boundary/1 `patch`
final resultJson = plugin.call(                  // answers a host `call`
  fn: 'openThread',
  argsJson: '{"postId":"abc"}',
  callId: 'call-1',
);
plugin.dispose();
```

See `example/lib/main.dart` for a runnable demo that loads
`example/assets/misskey_wasm_sample.wasm` (built from
[`../rust/misskey-wasm-sample`](../rust/misskey-wasm-sample)) and renders the
three messages above.

## Project structure

* `src/rasq_wasm_runtime.h` — the C ABI surface implemented by
  `rasq_wasm_runtime_core` (Rust `extern "C"` functions); `ffigen` generates
  `lib/rasq_wasm_runtime_bindings_generated.dart` from it.
* `hook/build.dart` — builds `rasq_wasm_runtime_core` with `cargo` and
  registers the resulting dylib as this package's code asset.
* `lib/rasq_wasm_runtime.dart` — the public `RasqWasmPlugin` API.

Regenerate bindings after changing the header:

```sh
dart run ffigen --config ffigen.yaml
```

## Flutter help

For help getting started with Flutter, view the
[online documentation](https://docs.flutter.dev).
