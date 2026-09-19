# {{display}} — アセットパック

wasm を持たない配布物(`kind: "assets"`、スロット契約 v0.11 §2.10)。中身は画像だけで、ホストは読むだけ(コードは動かない)。
そのぶん審査は軽い(画像の内容と権利の確認だけ)。

- `manifest.json` の `assets` に 1 枚ずつ書く: `id` / `path`(このディレクトリからの相対パス)/ `use`(今は `sticker` だけ)/ `label` / `tags`
- 画像は PNG / JPEG / WebP / GIF、1 枚 2 MiB まで、合計 16 MiB・256 枚まで。ステッカーは透過 PNG の 512px 前後が扱いやすい
- `rasq check` が形式(マジックナンバー)/ 大きさ / 書き忘れ(`stickers/` にあるのに `assets` に無い画像)を見る
- `rasq build` で `dist/{{id}}-<version>.rasqpkg`(このパックだけの配布物)、`rasq publish` でストアへ

`slots` / `permissions` / `actions` は書けない。土台(SNS プラグイン)にも依らない。
