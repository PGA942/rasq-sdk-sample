# {{name}} — `{{slot}}` の雛形

`rasq new {{slot}} {{name}}` で作られたプラグイン。id は `{{id}}`。

- 契約: rasq-sdk-sample の `docs/slots.md`(§2.7)、機械可読な登録簿は `schema/slots.json`
- `manifest.json` が `plugin.manifest` の答えそのもの(`src/lib.rs` が `include_str!` で返す)。名前 / 説明 / 権限はここを直す
- 確認: `rasq check`(wasm32 ビルド → export → manifest 往復 → スキーマ → `cargo test`)
- 端末: `rasq dev` が出す絶対パスを rasq-client の「WASMプラグイン試験 (dev)」→「.wasm のパスからインポート」に入れる
