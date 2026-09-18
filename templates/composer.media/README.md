# {{name}} — `{{slot}}` の雛形

`rasq new {{slot}} {{name}}` で作られたプラグイン。id は `{{id}}`。

- 契約: rasq-sdk-sample の `docs/slots.md`(§2.2)、機械可読な登録簿は `schema/slots.json`
- `manifest.json` が `plugin.manifest` の答えそのもの(`src/lib.rs` が `include_str!` で返す)。名前 / 説明 / 権限はここを直す
- 確認: `rasq check`(wasm32 ビルド → export → manifest 往復 → スキーマ → `cargo test`)
- 端末: `rasq dev` が出す絶対パスを rasq-client の「WASMプラグイン試験 (dev)」→「.wasm のパスからインポート」に入れる
- wasm そのものの確認: `rasq call {{name}} <fn> --args '<JSON>'`(wasmi で動かす。`cargo test` は native なので、時刻 / 乱数 / スレッドを呼ぶ依存が wasm でだけ panic するのはここで見つかる — rasq-sdk-sample `docs/cli.md` §5 の落とし穴)

## 下段バーのボタン(契約 v0.10)

`manifest.json` の `actions` の `slot: "composer.media"` の項目が、加工画面の下段バーのボタンになる
(`label` / `icon` / `tags` = 束ね先の希望 / `accepts` = 出る添付の種類、省略は `image/*`)。
宣言の `id` が `media.edit` に `actionId` で届き、`genres` に同じ id の編集があればホストがすぐ `media.action` を呼ぶ
(この雛形は `sample`)。動画 / 音声も受けるなら `accepts` に `"video/*"` / `"audio/*"` を足し、`media.edit` の
mimeType の判定も合わせる。
