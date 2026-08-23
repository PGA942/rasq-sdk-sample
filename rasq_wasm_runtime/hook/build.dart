import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';
import 'package:logging/logging.dart';

/// Builds `rust/rasq_wasm_runtime_core` (a cdylib embedding the `wasmi` wasm
/// interpreter, see PROTOCOL.md/D-007) via `cargo` and bundles it as this
/// package's code asset.
///
/// Only macOS and Linux hosts are wired up so far — Android/iOS/Windows need
/// cross-compilation toolchains (Android NDK, an iOS xcframework build, MSVC)
/// that haven't been set up yet. That is tracked as SDK-sample M3 follow-up
/// work, not silently faked here.
void main(List<String> args) async {
  await build(args, (input, output) async {
    final logger = Logger('')
      ..level = Level.ALL
      ..onRecord.listen((record) => print(record.message));

    final targetOS = input.config.code.targetOS;
    if (targetOS != OS.macOS && targetOS != OS.linux) {
      throw UnsupportedError(
        'rasq_wasm_runtime has no cargo cross-compilation wiring for '
        '$targetOS yet (only macOS/Linux hosts are supported today). '
        'See rust/README.md.',
      );
    }

    final rustDir = input.packageRoot.resolve('../rust/');
    final crateName = 'rasq_wasm_runtime_core';

    logger.info('Building $crateName with cargo (release, host target)...');
    final result = await Process.run(
      'cargo',
      ['build', '--release', '-p', crateName],
      workingDirectory: rustDir.toFilePath(),
    );
    logger.info(result.stdout.toString());
    if (result.exitCode != 0) {
      stderr.write(result.stderr);
      throw Exception('cargo build failed with exit code ${result.exitCode}');
    }

    final libFileName = targetOS.dylibFileName(crateName);
    final builtLib = rustDir.resolve('target/release/$libFileName');
    if (!File.fromUri(builtLib).existsSync()) {
      throw Exception('Expected cargo output not found at $builtLib');
    }

    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: '${input.packageName}_bindings_generated.dart',
        linkMode: DynamicLoadingBundled(),
        file: builtLib,
      ),
    );
    output.addDependencies([
      rustDir.resolve('rasq_wasm_runtime_core/src/lib.rs'),
      rustDir.resolve('rasq_wasm_runtime_core/Cargo.toml'),
    ]);
  });
}
