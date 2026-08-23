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

/// A loaded RASQ SNS plugin, running inside an embedded `wasmi` interpreter
/// (D-007: chosen over a JIT'ing runtime so the same engine works on iOS).
///
/// Speaks the `rasq.boundary/1` wire format from `PROTOCOL.md`: [snapshot]
/// and [patch] are emitted by the plugin, [call] delivers a host function
/// call into it. This class only shuttles the JSON strings across the FFI
/// boundary — decoding/validating them is the host renderer's job.
class RasqWasmPlugin {
  final ffi.Pointer<bindings.RasqWasmInstance> _instance;
  bool _destroyed = false;

  RasqWasmPlugin._(this._instance);

  /// Instantiates a wasm module from its raw bytes.
  factory RasqWasmPlugin.fromBytes(Uint8List wasmBytes) {
    final bytesPtr = malloc<ffi.Uint8>(wasmBytes.length);
    try {
      bytesPtr.asTypedList(wasmBytes.length).setAll(0, wasmBytes);
      final instance =
          bindings.rasq_wasm_instantiate(bytesPtr, wasmBytes.length);
      if (instance == ffi.nullptr) {
        throw RasqWasmInstantiateException();
      }
      return RasqWasmPlugin._(instance);
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
