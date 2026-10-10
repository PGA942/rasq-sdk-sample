import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

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

  test('host functions: wasm → Dart handler → wasm (storage round trip)', () {
    final path = '../rust/target/wasm32-unknown-unknown/release/misskey_wasm_sample.wasm';
    final file = File(path);
    if (!file.existsSync()) {
      markTestSkipped('misskey-wasm-sample not built; see rust/README.md');
      return;
    }
    // Dart-side host functions: storage.* on a plain map (host-fn.schema.json).
    final storage = <String, Object?>{};
    final received = <String>[];
    final plugin = RasqWasmPlugin.fromBytes(file.readAsBytesSync(), hostFn: (requestJson) {
      final req = jsonDecode(requestJson) as Map<String, dynamic>;
      received.add(req['fn'] as String);
      final args = (req['args'] as Map).cast<String, Object?>();
      final value = switch (req['fn'] as String) {
        'storage.set' => (() {
            storage[args['key'] as String] = args['value'];
            return const <String, Object?>{};
          })(),
        'storage.get' => {'value': storage[args['key'] as String]},
        _ => null,
      };
      if (value == null) return null; // unhandled → E00
      return jsonEncode({'callId': req['callId'], 'ok': true, 'value': value});
    });
    addTearDown(plugin.dispose);

    final result = jsonDecode(plugin.call(
      fn: 'storageRoundTrip',
      argsJson: '{"key":"greeting","value":"こんにちは"}',
      callId: 'call-2',
    )) as Map<String, dynamic>;
    expect(result['ok'], true, reason: '$result');
    expect((result['value'] as Map)['stored'], 'こんにちは');
    expect(received, ['storage.set', 'storage.get']);
    expect(storage['greeting'], 'こんにちは');
  });

  test('host functions: no handler → plugin sees E00, no trap', () {
    final path = '../rust/target/wasm32-unknown-unknown/release/misskey_wasm_sample.wasm';
    final file = File(path);
    if (!file.existsSync()) {
      markTestSkipped('misskey-wasm-sample not built');
      return;
    }
    final plugin = RasqWasmPlugin.fromBytes(file.readAsBytesSync());
    addTearDown(plugin.dispose);
    final result = jsonDecode(plugin.call(fn: 'storageRoundTrip', argsJson: '{"key":"k","value":1}', callId: 'call-3')) as Map<String, dynamic>;
    expect(result['ok'], false);
    expect((result['error'] as Map)['code'], 'E00');
  });

  // セキュリティ確認 2026-10-09 の [5]: 1 本ずつは上限内(256 MiB)でも、メモリを 2 本宣言すれば
  // 合計で倍を確保できた。ABI はメモリ 1 本なので、2 本目はインスタンス化(確保)の前に断る。
  group('メモリは 1 本まで(2026-10-09 の [5])', () {
    // (memory (export "memory") <min>) を [count] 本並べただけのモジュール。
    Uint8List memoriesModule(int count, List<int> minLeb) {
      final mem = <int>[count];
      for (var i = 0; i < count; i++) {
        mem..add(0x00)..addAll(minLeb);
      }
      final export = <int>[0x01, 0x06, ...utf8.encode('memory'), 0x02, 0x00];
      return Uint8List.fromList([
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, //
        0x05, mem.length, ...mem,
        0x07, export.length, ...export,
      ]);
    }

    test('2 本 × 256 MiB は hostFn の有無どちらでも読み込まない', () {
      final bytes = memoriesModule(2, [0x80, 0x20]); // 4096 ページ = 256 MiB ずつ
      expect(() => RasqWasmPlugin.fromBytes(bytes), throwsA(isA<RasqWasmInstantiateException>()));
      expect(() => RasqWasmPlugin.fromBytes(bytes, hostFn: (_) => null), throwsA(isA<RasqWasmInstantiateException>()));
    });

    test('1 本なら読み込める(対照)', () {
      final plugin = RasqWasmPlugin.fromBytes(memoriesModule(1, [0x01]), hostFn: (_) => null);
      plugin.dispose();
    });
  });

  group('zstd (.rspkg の展開, D-034)', () {
    final packed = File('test/fixtures/hello.txt.zst').readAsBytesSync();
    final expected = utf8.encode('RASQ zstd fixture ' * 4096);

    test('decompresses a zstd -19 frame', () {
      expect(rasqZstdDecompress(packed), expected);
    });

    test('refuses an output larger than maxOutput (decompression bomb guard)', () {
      expect(() => rasqZstdDecompress(packed, maxOutput: expected.length - 1), throwsA(isA<RasqZstdException>()));
      expect(rasqZstdDecompress(packed, maxOutput: expected.length), expected);
    });

    test('rejects garbage, truncated and empty input', () {
      expect(() => rasqZstdDecompress(Uint8List.fromList(utf8.encode('not zstd'))), throwsA(isA<RasqZstdException>()));
      expect(() => rasqZstdDecompress(Uint8List.sublistView(packed, 0, packed.length ~/ 2)), throwsA(isA<RasqZstdException>()));
      expect(() => rasqZstdDecompress(Uint8List(0)), throwsA(isA<RasqZstdException>()));
    });
  });
}
