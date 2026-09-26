# {{display}} — 着せ替え(テーマ)パック

wasm を持たない配布物(`kind: "theme"`、スロット契約 v0.16 §2.12)。中身はマニフェストの `theme`(宣言)と画像だけで、
ホストは読むだけ(コードは動かない / 権限も無い)。ホストで同時に有効にできる着せ替えは 1 つ
(設定 → アプリの外観 → フォントやカラースキーム)。

- `manifest.json` の `assets` に画像を 1 枚ずつ書く: `id` / `path`(このディレクトリからの相対パス)/ `use: "theme"` / `label`。
  色だけのテーマなら `assets` は空(`[]`)でもよい
- `theme.brightness` = `light` / `dark` / `both`(既定 `both`)。合わない明るさでは `colors` と `surfaces` を使わない(`icons` は常に効く)
- `theme.colors.light` / `theme.colors.dark` = Material の ColorScheme の上書き(`primary` / `surface` / `onSurface` / `outline` …)。
  色は `#RRGGBB` か `#AARRGGBB`
- `theme.icons` = 名前で持っているアイコンの差し替え。`"favorite_border": "asset:<id>"`(このパックの画像)か
  `"repeat": "material:autorenew"`(別の Material アイコン)
- `theme.surfaces` = 面ごとの見た目。`app`(メインの画面の背景)/ `postTile`(投稿タイル)/ `fow`(下から出るパネル)/
  `actionButton`(アクションバーのボタン)。それぞれ `color` / `radius`(0〜48)/ `layers`(画像、最大 8 枚)
- `layers[]` = `asset`(必須)/ `position`(`below` = 背景、`above` = 中身の上。above は触れない)/ `fit` / `alignment` /
  `repeat` / `opacity`
- 画像は PNG / JPEG / WebP / GIF、1 枚 2 MiB まで、合計 16 MiB・256 枚まで
- `rasq check` がスキーマ / 形式(マジックナンバー)/ 大きさ / 書き忘れ / 参照先(`layers[].asset` と `asset:<id>` が
  `assets` に `use: "theme"` であること)を見る
- `rasq build` で `dist/{{id}}-<version>.rasqpkg`(このパックだけの配布物)、`rasq publish` でストアへ(カテゴリ `theme`)

`slots` / `permissions` / `actions` / `roles` は書けない。土台(SNS プラグイン)にも依らない。
