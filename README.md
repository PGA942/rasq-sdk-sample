# RASQ SDK sample

RASQ(プラグインベース SNS クライアント)の **プラグイン SDK の仮組み**。
第一期はホスト側(`ORGATE-question/rasq-client` の `packages/rasq_plugin_api`)で Dart の契約を切り、
Misskey アダプタ(`packages/sns_misskey`)で検証したものを、ここで言語非依存の契約として固めていく。

- [`CONTRACT.md`](CONTRACT.md) — Host ⇄ SNS plugin 契約 draft v0.2(`rasq-client/docs/PLUGIN_CONTRACT.md` と同内容)
- [`PROTOCOL.md`](PROTOCOL.md) — 境界プロトコル draft v0.1(Snapshot / Patch / Call / Lifecycle / ホスト関数 / RRL ノードと意味タグ / 検証ゲート)
- [`schema/rrl-node.schema.json`](schema/rrl-node.schema.json) / [`schema/boundary.schema.json`](schema/boundary.schema.json) — RRL ノードと境界メッセージの JSON Schema
- [`examples/post-tile.rrl.json`](examples/post-tile.rrl.json) — 意味タグ付き PostTile の snapshot 例
- [`schema/manifest.schema.json`](schema/manifest.schema.json) — `PluginManifest`(function.json 最小サブセット)の JSON Schema
- [`examples/misskey.function.json`](examples/misskey.function.json) — Misskey 公式プラグインのマニフェスト例

## 次にやること(M3)

1. ~~`misskey-wasm-sample` で `PROTOCOL.md` の `snapshot/patch/call` を WASM 側から出す最小サンプル(まずは JSON v1)~~ → [`rust/misskey-wasm-sample`](rust/misskey-wasm-sample)
2. ~~WASM ランタイム選定(D-007: wasm3 / WAMR / wasmi、iOS インタープリタ基準)の実測~~ → `wasmi` を [`rust/rasq_wasm_runtime_core`](rust/rasq_wasm_runtime_core) に組み込み、Flutter プラグイン [`rasq_wasm_runtime`](rasq_wasm_runtime)(macOS/Linux)から実行を確認。Android/iOS/Windows のクロスコンパイル配線と wasm3/WAMR との比較実測は未着手(`rust/README.md` 参照)
3. スロット宣言(D-005)と `ActionRef`(D-006)の形式確定 → `schema/` に追加
4. 性能が必要なら Snapshot を FlatBuffers/ZeroCopy 化(論理モデルは維持)
