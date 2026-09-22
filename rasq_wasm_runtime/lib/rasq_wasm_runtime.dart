import 'dart:convert';
import 'dart:ffi' as ffi;
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import 'rasq_wasm_runtime_bindings_generated.dart' as bindings;

/// Thrown when a wasm module fails to instantiate (invalid bytes, or it is
/// missing the `memory`/`alloc`/`dealloc`/`snapshot`/`patch`/`call` exports
/// the RASQ boundary protocol requires).
class RasqWasmInstantiateException implements Exception {
  @override
  String toString() => 'RasqWasmInstantiateException: failed to instantiate wasm module';
}

/// Handles one host-function call from the plugin (`rasq.hostfn/1` request
/// JSON in, `{callId, ok, value|error}` response JSON out). Return `null` for
/// "unhandled" — the runtime turns that into an `E00` error for the plugin.
///
/// v1 is **synchronous**: the wasm plugin blocks inside `snapshot`/`patch`/
/// `call` until this returns, so it runs on whatever isolate invoked those.
typedef RasqHostFnHandler = String? Function(String requestJson);

/// A loaded RASQ SNS plugin, running inside an embedded `wasmi` interpreter
/// (D-007: chosen over a JIT'ing runtime so the same engine works on iOS).
///
/// Speaks the `rasq.boundary/1` wire format from `PROTOCOL.md`: [snapshot]
/// and [patch] are emitted by the plugin, [call] delivers a host function
/// call into it, and the plugin's own `rasq.host_fn` calls (net.fetch /
/// storage.* / ml.infer …, see `schema/host-fn.schema.json`) arrive at the
/// [RasqHostFnHandler] given to [RasqWasmPlugin.fromBytes]. This class only
/// shuttles JSON strings across the FFI boundary — decoding/validating them
/// is the host runtime's job.
class RasqWasmPlugin {
  /// Per-call fuel budget sized for bitmap work (the runtime default).
  ///
  /// The heaviest measured legitimate call — editing a 2048x1536 image —
  /// burns ~2.1G, so this leaves room for larger inputs. Cost tracks pixel
  /// count, not the plugin, which is why the axis here is "does it touch
  /// bitmaps", not the manifest's `kind`.
  static int get mediaFuelBudget => bindings.rasq_wasm_fuel_budget_media();

  /// Per-call fuel budget for everything else. Orders of magnitude tighter:
  /// the heaviest non-bitmap workload measured (a full timeline parse plus
  /// RRL rebuild) burns ~2M, so a runaway dies in a fraction of a second.
  static int get standardFuelBudget => bindings.rasq_wasm_fuel_budget_standard();

  final ffi.Pointer<bindings.RasqWasmInstance> _instance;
  final ffi.NativeCallable<bindings.RasqHostFnCallbackFunction>? _callable;
  bool _destroyed = false;

  RasqWasmPlugin._(this._instance, this._callable);

  /// Instantiates a wasm module from its raw bytes. [hostFn] serves the
  /// plugin's host-function calls; without it every `rasq.host_fn` call
  /// answers `E00`.
  ///
  /// [nativeFetch] serves `net.fetch` **inside the runtime** (blocking HTTP,
  /// rustls — works on iOS/Android where the embedder has no synchronous
  /// HTTP). While enabled, `net.fetch` never reaches [hostFn]; the calling
  /// isolate blocks for the duration of each request.
  ///
  /// [fuelPerCall] caps how much a single `call`/`snapshot`/`patch` may
  /// execute; exhausting it fails with `E02` instead of hanging the calling
  /// thread. Leave it null to keep the runtime default ([mediaFuelBudget],
  /// sized for bitmap work); pass [standardFuelBudget] for plugins that do
  /// not touch bitmaps — that is where the tightening actually happens.
  ///
  /// [fetchHosts] is the set of hosts the runtime-native `net.fetch` may
  /// reach (`misskey.io`, `*.example.com`, or `*`). It starts **empty =
  /// closed**; pass the manifest's `network.hosts` plus whatever instance
  /// host the session grants, and call [setFetchHosts] when that changes.
  factory RasqWasmPlugin.fromBytes(Uint8List wasmBytes, {RasqHostFnHandler? hostFn, bool nativeFetch = false, int? fuelPerCall, Set<String> fetchHosts = const {}}) {
    final bytesPtr = malloc<ffi.Uint8>(wasmBytes.length);
    ffi.NativeCallable<bindings.RasqHostFnCallbackFunction>? callable;
    try {
      bytesPtr.asTypedList(wasmBytes.length).setAll(0, wasmBytes);
      final ffi.Pointer<bindings.RasqWasmInstance> instance;
      if (hostFn == null) {
        instance = bindings.rasq_wasm_instantiate(bytesPtr, wasmBytes.length);
      } else {
        ffi.Pointer<ffi.Char> trampoline(ffi.Pointer<ffi.Char> request, ffi.Pointer<ffi.Void> userData) {
          final String? response;
          try {
            response = hostFn(request.cast<Utf8>().toDartString());
          } catch (_) {
            return ffi.nullptr; // runtime reports E00 instead of trapping
          }
          if (response == null) return ffi.nullptr;
          final tmp = response.toNativeUtf8();
          try {
            // Must be duplicated into the runtime's allocator (it frees it).
            return bindings.rasq_wasm_string_dup(tmp.cast());
          } finally {
            malloc.free(tmp);
          }
        }

        // isolateLocal: the callback fires synchronously while we're inside
        // snapshot()/patch()/call() on this isolate's thread.
        callable = ffi.NativeCallable<bindings.RasqHostFnCallbackFunction>.isolateLocal(trampoline);
        instance = bindings.rasq_wasm_instantiate_with_host(bytesPtr, wasmBytes.length, callable.nativeFunction, ffi.nullptr);
      }
      if (instance == ffi.nullptr) {
        callable?.close();
        throw RasqWasmInstantiateException();
      }
      if (nativeFetch) bindings.rasq_wasm_set_native_fetch(instance, true);
      if (fuelPerCall != null) bindings.rasq_wasm_set_fuel_per_call(instance, fuelPerCall);
      final plugin = RasqWasmPlugin._(instance, callable);
      if (fetchHosts.isNotEmpty) plugin.setFetchHosts(fetchHosts);
      return plugin;
    } finally {
      malloc.free(bytesPtr);
    }
  }

  /// Replaces the hosts the runtime-native `net.fetch` may reach. Empty =
  /// closed. Only the runtime-native path looks at this; the embedder's own
  /// `net.fetch` dispatcher must apply the same allowlist itself.
  void setFetchHosts(Set<String> hosts) {
    _checkNotDestroyed();
    final csv = hosts.map((h) => h.trim().toLowerCase()).where((h) => h.isNotEmpty).join(',').toNativeUtf8();
    try {
      bindings.rasq_wasm_set_fetch_hosts(_instance, csv.cast());
    } finally {
      malloc.free(csv);
    }
  }

  /// The `net.fetch` request shapes this instance has seen for the first time
  /// since the previous call. Each entry is `{hash, shape}` — no values, only
  /// host / method / path template / query keys / header names / body key
  /// skeleton — or a single `{overflow: true}` once the per-instance cap is
  /// exceeded. Drain after every call when reporting to the store.
  List<Map<String, Object?>> takeFetchShapes() {
    _checkNotDestroyed();
    final ptr = bindings.rasq_wasm_take_fetch_shapes(_instance);
    if (ptr == ffi.nullptr) return const [];
    try {
      final decoded = jsonDecode(ptr.cast<Utf8>().toDartString());
      return [for (final e in (decoded as List)) (e as Map).cast<String, Object?>()];
    } finally {
      bindings.rasq_wasm_free_string(ptr);
    }
  }

  /// Calls the plugin's `snapshot` export; returns a `rasq.boundary/1`
  /// `snapshot` message as JSON.
  String snapshot() {
    _checkNotDestroyed();
    final ptr = bindings.rasq_wasm_snapshot(_instance);
    return _takeString(ptr);
  }

  /// Calls the plugin's `patch` export; returns a `rasq.boundary/1` `patch`
  /// message as JSON.
  String patch() {
    _checkNotDestroyed();
    final ptr = bindings.rasq_wasm_patch(_instance);
    return _takeString(ptr);
  }

  /// Delivers a host `call { fn, args, callId }` (PROTOCOL.md §3.2) to the
  /// plugin and returns its `result` message as JSON.
  String call({required String fn, required String argsJson, required String callId}) {
    _checkNotDestroyed();
    final fnPtr = fn.toNativeUtf8();
    final argsPtr = argsJson.toNativeUtf8();
    final callIdPtr = callId.toNativeUtf8();
    try {
      final ptr = bindings.rasq_wasm_call(_instance, fnPtr.cast(), argsPtr.cast(), callIdPtr.cast());
      return _takeString(ptr);
    } finally {
      malloc.free(fnPtr);
      malloc.free(argsPtr);
      malloc.free(callIdPtr);
    }
  }

  /// Releases the wasmi instance. The plugin must not be used afterwards.
  void dispose() {
    if (_destroyed) return;
    bindings.rasq_wasm_destroy(_instance);
    _callable?.close();
    _destroyed = true;
  }

  void _checkNotDestroyed() {
    if (_destroyed) {
      throw StateError('RasqWasmPlugin used after dispose()');
    }
  }

  String _takeString(ffi.Pointer<ffi.Char> ptr) {
    final value = ptr.cast<Utf8>().toDartString();
    bindings.rasq_wasm_free_string(ptr);
    return value;
  }
}

/// zstd の展開に失敗した(壊れた入力 / 出力が上限を超えた)。
class RasqZstdException implements Exception {
  const RasqZstdException(this.message);
  final String message;
  @override
  String toString() => 'RasqZstdException: $message';
}

/// `.rasqpkg`(zstd 圧縮 tar、D-034)の展開に使う zstd の復号。ランタイム内蔵の純 Rust 実装(ruzstd)なので
/// 全プラットフォームで同じ。[maxOutput] を超える出力は失敗にする(展開爆弾の対策。既定 256 MiB)。
Uint8List rasqZstdDecompress(Uint8List input, {int maxOutput = 256 * 1024 * 1024}) {
  if (input.isEmpty) throw const RasqZstdException('empty input');
  final inPtr = malloc<ffi.Uint8>(input.length);
  final outLen = malloc<ffi.IntPtr>();
  try {
    inPtr.asTypedList(input.length).setAll(0, input);
    final out = bindings.rasq_zstd_decompress(inPtr, input.length, maxOutput, outLen);
    if (out == ffi.nullptr) {
      throw RasqZstdException('not a valid zstd frame, or the output exceeds $maxOutput bytes');
    }
    try {
      return Uint8List.fromList(out.asTypedList(outLen.value));
    } finally {
      bindings.rasq_bytes_free(out, outLen.value);
    }
  } finally {
    malloc.free(inPtr);
    malloc.free(outLen);
  }
}
