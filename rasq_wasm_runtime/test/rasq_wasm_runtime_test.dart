import 'dart:convert';
import 'dart:io';

import 'package:rasq_wasm_runtime/rasq_wasm_runtime.dart';
import 'package:test/test.dart';

/// Loads the misskey-wasm-sample module built by
/// `cargo build --release --target wasm32-unknown-unknown -p misskey-wasm-sample`
/// (run from `rust/`). Skips instead of failing if it hasn't been built yet,
/// since this test package doesn't drive cargo itself.
RasqWasmPlugin? loadSamplePlugin() {
  final path =
      '../rust/target/wasm32-unknown-unknown/release/misskey_wasm_sample.wasm';
  final file = File(path);
  if (!file.existsSync()) {
    return null;
  }
  return RasqWasmPlugin.fromBytes(file.readAsBytesSync());
}

void main() {
  test('snapshot/patch/call round-trip through the embedded wasmi runtime',
      () {
    final plugin = loadSamplePlugin();
    if (plugin == null) {
      markTestSkipped(
          'misskey-wasm-sample not built; see rust/README.md');
      return;
    }
    addTearDown(plugin.dispose);

    final snapshot = jsonDecode(plugin.snapshot()) as Map<String, dynamic>;
    expect(snapshot['schema'], 'rasq.boundary/1');
    expect(snapshot['type'], 'snapshot');
    expect(snapshot['rev'], 1);

    final patch = jsonDecode(plugin.patch()) as Map<String, dynamic>;
    expect(patch['type'], 'patch');
    expect(patch['baseRev'], 1);

    final result = jsonDecode(plugin.call(
      fn: 'openThread',
      argsJson: '{"postId":"abc"}',
      callId: 'call-1',
    )) as Map<String, dynamic>;
    expect(result['type'], 'result');
    expect(result['ok'], true);
    expect(result['callId'], 'call-1');
  });
}
