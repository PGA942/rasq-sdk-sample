# {{name}} — `{{slot}}` の雛形

`rasq new {{slot}} {{name}}` で作られたプラグイン。id は `{{id}}`。

- 契約: rasq-sdk-sample の `docs/slots.md`(§2.9)、機械可読な登録簿は `schema/slots.json`
- `manifest.json` が `plugin.manifest` の答えそのもの(`src/lib.rs` が `include_str!` で返す)。名前 / 説明 / 権限はここを直す
- 確認: `rasq check`(wasm32 ビルド → export → manifest 往復 → スキーマ → `cargo test`)
- 端末: `rasq dev` が出す絶対パスを rasq-client の「WASMプラグイン試験 (dev)」→「.wasm のパスからインポート」に入れる
- wasm そのものの確認: `rasq call {{name}} <fn> --args '<JSON>'`(wasmi で動かす。`cargo test` は native なので、時刻 / 乱数 / スレッドを呼ぶ依存が wasm でだけ panic するのはここで見つかる — rasq-sdk-sample `docs/cli.md` §5 の落とし穴)
