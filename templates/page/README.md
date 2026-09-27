# {{name}} — `{{slot}}` の雛形

`rasq new {{slot}} {{name}}` で作られたページ型プラグイン(カスタムプラグイン)。id は `{{id}}`。

- 契約: rasq-sdk-sample の `docs/slots.md`(§2.13)、機械可読な登録簿は `schema/slots.json`
- `manifest.json` が `plugin.manifest` の答えそのもの(`src/lib.rs` が `include_str!` で返す)。名前 / 説明 / 権限 / `page` はここを直す
- 雛形は **shared**(`page.mode = "shared"`、権限 `shared.session`): みんなで押すカウンター。`+1` は `shared.op incr` で、
  結果は `page.event {type: "var"}` で全員に届く。セッションが無い(1 人で開いた)ときは端末の中だけで数える
- 1 人で完結させるなら `page.mode` を `"solo"` にし、`surfaces` から `astell` を外し、権限の `shared.session` を消す
- 確認: `rasq check`(wasm32 ビルド → export → manifest 往復 → スキーマ → `cargo test`)
- 端末: `rasq dev` が出す絶対パスを rasq-client の「WASMプラグイン試験 (dev)」→「.wasm のパスからインポート」に入れる
- wasm そのものの確認: `rasq call {{name}} page.open --args '{"token":"p1","surface":"page","context":{},"me":{"accountId":"a"},"size":{"width":400,"height":800}}' --then count.local`
