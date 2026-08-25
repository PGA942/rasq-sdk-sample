// Exercises the real app: loads the bundled misskey-wasm-sample.wasm asset,
// runs it inside the embedded wasmi runtime via the actual widget tree, and
// checks the snapshot/patch/call JSON it renders. This is the same code
// path a user would see on screen, just verified headlessly.
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rasq_wasm_runtime_example/main.dart';

void main() {
  testWidgets('runs the wasm plugin and renders snapshot/patch/call',
      (tester) async {
    await tester.pumpWidget(const MyApp());
    await tester.pumpAndSettle();

    expect(find.textContaining('rasq.boundary/1'), findsWidgets);
    expect(find.textContaining('"type":"snapshot"'), findsOneWidget);
    expect(find.textContaining('"type":"patch"'), findsOneWidget);
    expect(find.textContaining('"postId":"abc"'), findsWidgets);
    // Host-function round trip: the value went wasm → Dart storage → wasm.
    expect(find.textContaining('こんにちは wasm'), findsOneWidget);
  });
}
