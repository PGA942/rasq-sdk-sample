import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';
import 'package:logging/logging.dart';

/// Builds `rust/rasq_wasm_runtime_core` (a cdylib embedding the `wasmi` wasm
/// interpreter, see PROTOCOL.md / D-007) with `cargo` for the *target* Flutter
/// is building for, and bundles it as this package's code asset.
///
/// Target matrix (rustup target ↔ Flutter target):
///
/// | OS | arch | rust triple | notes |
/// |---|---|---|---|
/// | macOS | arm64 / x64 | aarch64-apple-darwin / x86_64-apple-darwin | `MACOSX_DEPLOYMENT_TARGET` from config |
/// | iOS device | arm64 | aarch64-apple-ios | `IPHONEOS_DEPLOYMENT_TARGET` from config |
/// | iOS simulator | arm64 / x64 | aarch64-apple-ios-sim / x86_64-apple-ios | |
/// | Android | arm64 / arm / x64 / ia32 | aarch64-linux-android / armv7-linux-androideabi / x86_64-linux-android / i686-linux-android | linker = NDK `<triple><api>-clang` (from `cCompiler`), 16 KiB page size |
/// | Linux | host arch | aarch64/x86_64-unknown-linux-gnu | host build only |
/// | Windows | host arch | aarch64/x86_64-pc-windows-msvc | host build only (untested: no Windows host here) |
///
/// Missing rustup targets fail loudly with the `rustup target add …` line to
/// run — nothing is silently skipped. Apple targets link through Xcode's clang
/// (rustc finds the SDK via `xcrun`), so no extra config is needed there.
void main(List<String> args) async {
  await build(args, (input, output) async {
    final logger = Logger('')
      ..level = Level.ALL
      ..onRecord.listen((record) => print(record.message));

    final code = input.config.code;
    final target = _RustTarget.resolve(code);
    final rustDir = input.packageRoot.resolve('../rust/');
    const crateName = 'rasq_wasm_runtime_core';

    final env = <String, String>{...Platform.environment, ...target.env(code)};
    final cargo = _cargoExecutable();
    logger.info('cargo build --release --target ${target.triple} -p $crateName (for ${code.targetOS}/${code.targetArchitecture})');
    final result = await Process.run(
      cargo,
      ['build', '--release', '--target', target.triple, '-p', crateName],
      workingDirectory: rustDir.toFilePath(),
      environment: env,
    );
    logger.info(result.stdout.toString());
    if (result.exitCode != 0) {
      final err = result.stderr.toString();
      stderr.write(err);
      if (err.contains("target may not be installed") || err.contains('can\'t find crate for `core`')) {
        throw Exception('Rust target ${target.triple} is not installed. Run: rustup target add ${target.triple}');
      }
      throw Exception('cargo build failed with exit code ${result.exitCode}');
    }

    final libFileName = code.targetOS.dylibFileName(crateName);
    final builtLib = rustDir.resolve('target/${target.triple}/release/$libFileName');
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
    output.dependencies.addAll([
      rustDir.resolve('rasq_wasm_runtime_core/src/lib.rs'),
      rustDir.resolve('rasq_wasm_runtime_core/Cargo.toml'),
      rustDir.resolve('Cargo.toml'),
      rustDir.resolve('Cargo.lock'),
    ]);
  });
}

/// `cargo` from PATH, falling back to the rustup default location — Xcode and
/// Gradle invoke the hook with a minimal PATH that usually lacks `~/.cargo/bin`.
String _cargoExecutable() {
  final home = Platform.environment['HOME'] ?? Platform.environment['USERPROFILE'];
  final candidates = [
    if (Platform.environment['CARGO'] != null) Platform.environment['CARGO']!,
    if (home != null) '$home/.cargo/bin/cargo${Platform.isWindows ? '.exe' : ''}',
  ];
  for (final c in candidates) {
    if (File(c).existsSync()) return c;
  }
  return 'cargo';
}

class _RustTarget {
  _RustTarget(this.triple, {this.androidClangTriple});

  final String triple;

  /// NDK clang wrapper prefix (differs from the rust triple for armv7).
  final String? androidClangTriple;

  static _RustTarget resolve(CodeConfig code) {
    final os = code.targetOS;
    final arch = code.targetArchitecture;
    Never unsupported() => throw UnsupportedError('rasq_wasm_runtime: no Rust target mapping for $os/$arch');

    if (os == OS.macOS) {
      return switch (arch) {
        Architecture.arm64 => _RustTarget('aarch64-apple-darwin'),
        Architecture.x64 => _RustTarget('x86_64-apple-darwin'),
        _ => unsupported(),
      };
    }
    if (os == OS.iOS) {
      final sim = code.iOS.targetSdk == IOSSdk.iPhoneSimulator;
      return switch ((arch, sim)) {
        (Architecture.arm64, false) => _RustTarget('aarch64-apple-ios'),
        (Architecture.arm64, true) => _RustTarget('aarch64-apple-ios-sim'),
        (Architecture.x64, true) => _RustTarget('x86_64-apple-ios'),
        _ => unsupported(),
      };
    }
    if (os == OS.android) {
      return switch (arch) {
        Architecture.arm64 => _RustTarget('aarch64-linux-android', androidClangTriple: 'aarch64-linux-android'),
        Architecture.arm => _RustTarget('armv7-linux-androideabi', androidClangTriple: 'armv7a-linux-androideabi'),
        Architecture.x64 => _RustTarget('x86_64-linux-android', androidClangTriple: 'x86_64-linux-android'),
        Architecture.ia32 => _RustTarget('i686-linux-android', androidClangTriple: 'i686-linux-android'),
        _ => unsupported(),
      };
    }
    if (os == OS.linux) {
      return switch (arch) {
        Architecture.arm64 => _RustTarget('aarch64-unknown-linux-gnu'),
        Architecture.x64 => _RustTarget('x86_64-unknown-linux-gnu'),
        _ => unsupported(),
      };
    }
    if (os == OS.windows) {
      return switch (arch) {
        Architecture.arm64 => _RustTarget('aarch64-pc-windows-msvc'),
        Architecture.x64 => _RustTarget('x86_64-pc-windows-msvc'),
        _ => unsupported(),
      };
    }
    return unsupported();
  }

  /// Extra environment for cargo: per-target linker (Android NDK), deployment
  /// targets (Apple) and link args (Android 16 KiB pages).
  Map<String, String> env(CodeConfig code) {
    final key = triple.toUpperCase().replaceAll('-', '_');
    final os = code.targetOS;
    if (os == OS.android) {
      final cc = code.cCompiler?.compiler;
      if (cc == null) {
        throw StateError('Flutter did not provide an NDK C compiler for Android; is the Android NDK installed?');
      }
      final binDir = File.fromUri(cc).parent.path;
      final api = code.android.targetNdkApi;
      final ext = Platform.isWindows ? '.cmd' : '';
      final linker = '$binDir/$androidClangTriple$api-clang$ext';
      if (!File(linker).existsSync()) {
        throw StateError('NDK clang wrapper not found: $linker');
      }
      return {
        'CARGO_TARGET_${key}_LINKER': linker,
        'CARGO_TARGET_${key}_AR': '$binDir/llvm-ar${Platform.isWindows ? '.exe' : ''}',
        // Android 15+ requires 16 KiB page-aligned .so files (NDK r28 default, be explicit).
        'CARGO_TARGET_${key}_RUSTFLAGS': '-C link-arg=-Wl,-z,max-page-size=16384',
      };
    }
    if (os == OS.iOS) {
      return {'IPHONEOS_DEPLOYMENT_TARGET': '${code.iOS.targetVersion}.0'};
    }
    if (os == OS.macOS) {
      return {'MACOSX_DEPLOYMENT_TARGET': '${code.macOS.targetVersion}.0'};
    }
    return const {};
  }
}
