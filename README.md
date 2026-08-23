# RASQ SDK sample

RASQ(プラグインベース SNS クライアント)の **プラグイン SDK の仮組み**。
第一期はホスト側(`ORGATE-question/rasq-client` の `packages/rasq_plugin_api`)で Dart の契約を切り、
Misskey アダプタ(`packages/sns_misskey`)で検証したものを、ここで言語非依存の契約として固めていく。

- [`CONTRACT.md`](CONTRACT.md) — Host ⇄ SNS plugin 契約 draft v0.2(`rasq-client/docs/PLUGIN_CONTRACT.md` と同内容)
- [`schema/manifest.schema.json`](schema/manifest.schema.json) — `PluginManifest`(function.json 最小サブセット)の JSON Schema
- [`examples/misskey.function.json`](examples/misskey.function.json) — Misskey 公式プラグインのマニフェスト例

## 次にやること(M3)

1. 境界プロトコル(Snapshot / Patch / Call / Lifecycle, D-001)の IDL 化(FlatBuffers or protobuf、iOS インタープリタ前提)
2. `SnsSession` の各操作を WASM 境界越しの関数呼び出し(`hostFn` / `event`)として写す
3. `misskey-wasm-sample` で同じ契約を WASM 側から満たす
