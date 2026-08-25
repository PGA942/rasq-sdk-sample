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
  final ffi.Pointer<bindings.RasqWasmInstance> _instance;
  final ffi.NativeCallable<bindings.RasqHostFnCallbackFunction>? _callable;
  bool _destroyed = false;

  RasqWasmPlugin._(this._instance, this._callable);

  /// Instantiates a wasm module from its raw bytes. [hostFn] serves the
  /// plugin's host-function calls; without it every `rasq.host_fn` call
  /// answers `E00`.
  factory RasqWasmPlugin.fromBytes(Uint8List wasmBytes, {RasqHostFnHandler? hostFn}) {
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
      return RasqWasmPlugin._(instance, callable);
    } finally {
      malloc.free(bytesPtr);
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
