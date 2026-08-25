import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show rootBundle;
import 'package:rasq_wasm_runtime/rasq_wasm_runtime.dart';

void main() {
  runApp(const MyApp());
}

/// Demonstrates the RASQ M3 wasm host: loads a wasm plugin (bundled as an
/// asset), runs it inside the embedded `wasmi` interpreter, and displays the
/// `rasq.boundary/1` snapshot/patch/call messages it emits.
class MyApp extends StatefulWidget {
  const MyApp({super.key});

  @override
  State<MyApp> createState() => _MyAppState();
}

class _MyAppState extends State<MyApp> {
  late Future<_WasmDemoResult> _demo;

  @override
  void initState() {
    super.initState();
    _demo = _runDemo();
  }

  Future<_WasmDemoResult> _runDemo() async {
    final bytes = await rootBundle.load('assets/misskey_wasm_sample.wasm');
    // Host functions (schema/host-fn.schema.json): storage.* over an
    // in-memory map — enough to demo the wasm → host → wasm round trip.
    final storage = <String, Object?>{};
    final plugin = RasqWasmPlugin.fromBytes(bytes.buffer.asUint8List(), hostFn: (requestJson) {
      final req = jsonDecode(requestJson) as Map<String, dynamic>;
      final args = (req['args'] as Map?)?.cast<String, Object?>() ?? const {};
      final value = switch (req['fn'] as String?) {
        'storage.set' => (() {
            storage[args['key'] as String] = args['value'];
            return const <String, Object?>{};
          })(),
        'storage.get' => {'value': storage[args['key'] as String]},
        _ => null, // unhandled → runtime answers E00
      };
      if (value == null) return null;
      return jsonEncode({'callId': req['callId'], 'ok': true, 'value': value});
    });
    try {
      final snapshot = plugin.snapshot();
      final patch = plugin.patch();
      final call = plugin.call(
        fn: 'openThread',
        argsJson: '{"postId":"abc"}',
        callId: 'call-1',
      );
      final hostFnDemo = plugin.call(
        fn: 'storageRoundTrip',
        argsJson: '{"key":"greeting","value":"こんにちは wasm"}',
        callId: 'call-2',
      );
      return _WasmDemoResult(snapshot: snapshot, patch: patch, call: call, hostFnDemo: hostFnDemo);
    } finally {
      plugin.dispose();
    }
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      home: Scaffold(
        appBar: AppBar(title: const Text('RASQ wasm runtime demo')),
        body: FutureBuilder<_WasmDemoResult>(
          future: _demo,
          builder: (context, snap) {
            if (snap.hasError) {
              return Center(child: Text('Error: ${snap.error}'));
            }
            if (!snap.hasData) {
              return const Center(child: CircularProgressIndicator());
            }
            final result = snap.data!;
            return SingleChildScrollView(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('snapshot()', style: TextStyle(fontWeight: FontWeight.bold)),
                  SelectableText(result.snapshot),
                  const SizedBox(height: 16),
                  const Text('patch()', style: TextStyle(fontWeight: FontWeight.bold)),
                  SelectableText(result.patch),
                  const SizedBox(height: 16),
                  const Text('call(openThread)', style: TextStyle(fontWeight: FontWeight.bold)),
                  SelectableText(result.call),
                  const SizedBox(height: 16),
                  const Text('call(storageRoundTrip) — wasm → host_fn(storage.*) → wasm', style: TextStyle(fontWeight: FontWeight.bold)),
                  SelectableText(result.hostFnDemo),
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}

class _WasmDemoResult {
  final String snapshot;
  final String patch;
  final String call;
  final String hostFnDemo;

  _WasmDemoResult({required this.snapshot, required this.patch, required this.call, required this.hostFnDemo});
}
