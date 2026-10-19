# RRL — 画面コンストラクタ

> 正典: [`PROTOCOL.md`](../PROTOCOL.md) §2〜3、[`schema/rrl-node.schema.json`](../schema/rrl-node.schema.json)。
> フル例: [`examples/post-tile.rrl.json`](../examples/post-tile.rrl.json)。

## ノードの形

```json
{ "t": "Text", "k": "greeting", "tag": null, "role": null,
  "p": { "text": "こんにちは" }, "on": { "tap": "greet" }, "c": [] }
```

- `t` — ノード型。**L1 Primitive**(`Text` `Icon` `Image` `Row` `Column` `Stack` `Grid` `Scroll` `Canvas` `Spacer`)/
  **L2 Standard Component**(ホスト実装・Props のみ: `PostTile` `ActionBar` `RichText` `MediaGrid` `Avatar` `Button` `PollWidget` `ListTile` …)/
  **L3 Control**(`If` `ForEach` `Call` `Bind` `Slot` `Stream`)
- `k` — 兄弟内で一意な key。**patch の target になるのは明示 key を持つノードだけ**。
  nodeId は `{instanceScope}:{key}`(イベントやジオメトリでもこの形)
- `p` — Props。`"$.path"` はホスト Store への Bind(`$.post.author.displayName` など)
- `on` — イベント → 自分の関数名(`tap` / `longPress` / `pointer*` / `confirm` / `change` / `submit`)。
  発火するとホストから `call(fn, {nodeId, kind, …})` が来る
- `tag` — **意味タグ**(`main_text` / `pic` / `quote` / `user_name` …)。投稿内容を構成する
  ノードにだけ付ける(RAi・関連TL・ソートが読む。PROTOCOL §2.2)
- `role` — 構造ロール(`actionBar` / `contentBody` / `threadNav` / `mediaItem`)。
  検証ゲートに加え、**ジオメトリ登録簿**に載る(= FOW の `anchor` で実物の矩形に張り付ける)

## snapshot と patch

- `snapshot` = `{schema, scope, type: "snapshot", rev, root}` — ツリー全体。
- `patch` = `{schema, scope, type: "patch", rev, baseRev, surface?, ops: [...]}` — 原子バッチ。
  ops は `setProp` / `insert` / `remove` / `replaceSubtree` / `move`。
  `baseRev` 不一致は拒否 → ホストが resync(snapshot 再取得)。
- 引き取らせ方(v1): `call` の戻り値に **`treeUpdated: true`**(→ ホストが snapshot を再取得)
  または **`patch: true`**(→ ホストが `patch()` を 1 回引き取る)。
- `surface: "<fowId>"` を付けると **FOW ドキュメント**への差分になる(→ [fow.md](fow.md))。

最小例(hello プラグインの `greet`):

```rust
*PENDING_PATCH.lock().unwrap() = json!({
    "schema": "rasq.boundary/1", "scope": SCOPE, "type": "patch",
    "rev": base + 1, "baseRev": base,
    "ops": [{ "op": "setProp", "target": format!("{SCOPE}:greeting"),
              "prop": "text", "value": "こんにちは!(1 回目)" }]
});
Ok(json!({ "patch": true }))
```

## 入力欄 `Input`(v0.18、rasq-client D-357)

1 行(または複数行)の文字入力。送信ボタン付きで、ページ型プラグイン([slots.md §2.13](slots.md))の寄せ書きなどに使う。

```json
{ "t": "Input", "k": "gbInput",
  "p": { "hint": "ひとこと(80 文字まで)", "maxLength": 80, "submitLabel": "書く" },
  "on": { "submit": "gb.send" } }
```

| props | 値(既定) | 意味 |
|---|---|---|
| `hint` | 文字列 | 空のときの薄い案内 |
| `value` | 文字列 | 初めに入っている文字(省略 = 空) |
| `multiline` | bool(`false`) | 複数行 |
| `maxLength` | 整数 | 打てる文字数の上限 |
| `submitLabel` | 文字列(送信アイコン) | 送信ボタンの文字 |
| `clearOnSubmit` | bool(`true`) | 送ったら欄を空にする |
| `label` | 文字列 | 欄の名前(枠の上に残る。設定ページの「名前」「自己紹介」など、rasq-client D-482) |
| `allowEmpty` | bool(`false`) | 空のまま送れる(設定ページで項目を消すとき、rasq-client D-482) |
| `pad` | 数 | 左右の余白(上下はその 1/4。設定ページの欄、rasq-client D-482) |

| on | payload | いつ |
|---|---|---|
| `change` | `{text}` | 打つたび(要らなければ書かない。書くと 1 文字ごとに call が来る) |
| `submit` | `{text}` | 送信ボタン / 送信キー |

- 打っている文字は**ホストが持つ**。描き直し(`page.render` / patch)で同じ `k` の `Input` が来ても、打ちかけの文字は消えない
  (`clearOnSubmit` が送ったあとに空にする)。プラグインは `submit` の `text` を使えばよく、下書きを持たなくてよい。
- payload はイベントの args に**平らに**載る(ページ型なら `{token, nodeId, kind: "submit", text}`)。

## 本文(RSL)

`RichText` の `p.source` は生文字列(ホストが MFM/HTML を解析)でも、解析済みスパン配列でもよい。
スパンの形は [`schema/rsl-span.schema.json`](../schema/rsl-span.schema.json)、
MFM 装飾は `transform` スパン(例: [`examples/mfm-body.rsl.json`](../examples/mfm-body.rsl.json))。

## 設定ページ(wasm 不要の静的 RRL)

設定第二階層は `rasq.settings-page/1`([`schema/settings-page.schema.json`](../schema/settings-page.schema.json))の
静的ドキュメント。値は Bind `$.settings.<key>`、書き込みは `on: {"change": "settings.set"}` を
ホストが KV へ写す。SNS プラグインは `settingsPages` として同形式を提供する(PROTOCOL §2.3 の設定ページ注記)。

## SVG(v0.22、rasq-client D-465)

`Image.p.src` に **`data:image/svg+xml;base64,…`**(または `data:image/svg+xml,<URL エンコードした SVG>`)を置ける。
ページ型の落書き([slots.md §2.13](slots.md) の `media.editor.open`)が返す SVG をそのまま描くためのもの。

```json
{ "t": "Image", "k": "paint", "p": { "src": "data:image/svg+xml;base64,PHN2ZyB2aWV3Qm94PSIwIDAgMzAwIDQwMCI…", "fit": "contain" } }
```

- ホストは描く前に**許可リストで消毒**する。残るのは要素 `svg` / `g` / `path` / `polyline` / `polygon` / `line` / `rect` / `circle` / `ellipse` と、
  形と描き方の属性(`viewBox` `width` `height` `d` `points` `x` `y` `cx` `cy` `r` `rx` `ry` `x1` `y1` `x2` `y2` `fill` `stroke` `stroke-width`
  `stroke-linecap` `stroke-linejoin` `opacity` `fill-opacity` `stroke-opacity`)だけ。色は `#RGB` / `#RRGGBB` / `none` / 色の名前。
  `script` / `style` / `foreignObject` / `image` / `use` / `a` / `href` / `on*` / `url(...)` / `transform` などは捨てる(外のものを読まない)。
- `path` の `d` は `M L H V C S Q T Z`(小文字の相対も)を読む。円弧 `A` は描かない。
- 描く量の上限(下の「描く量の上限」)とは別に、SVG 1 枚は **64 KiB・要素 4,000 個・`d` / `points` の数 40,000 個**まで(超えた分は描かない)。
- `p.fit` = `contain`(既定)/ `cover` / `fill`。`p.width` / `p.height` が無ければ親の幅に合わせ、`viewBox` の縦横比で高さを決める。
- (v0.23、rasq-client D-472)**`data:image/svg+xml;zstd;base64,…`** = zstd で縮めた SVG(落書きの `edited` の `svgz` をそのまま、
  `rasq-plugin-kit` の `svgz_data_uri`)。ホストは展開して(展開後も 64 KiB まで)から上と同じく消毒して描く。v0.22 以前のホストは描けない
  (箱になる。`page.open` の `contract` で見分ける、[slots.md §2.13](slots.md))。

## Grid の 1 マスの縦横比(v0.22)

`Grid.p.aspectRatio` = 1 マスの**幅 ÷ 高さ**(既定 `1` = 正方形)。3:4 の縦長のマスを 2 列に並べるなら
`{"t": "Grid", "p": {"columns": 2, "aspectRatio": 0.75}, "c": [...]}`。0.1〜10 の範囲で、外れた値は 1 にする。

## 障害隔離

ノード単位で try/catch され、壊れたノードはインラインの ⚠ ボックスに縮退する(V12)。
未知の `t` も同じ — プラグインが古いホストで動いてもクラッシュはしない。

## 描く量の上限(rasq-client D-454、セキュリティ確認 2026-10-09 [4])

ホストは木を描く前に、描くときと同じ規則で展開後の量を数える(`ForEach` は項目ごと、`Bind` 越しのリストも
数える、偽の `If` の中は数えない)。**1 つの木で展開後のノード 10000 / `ForEach` の繰り返しの合計 10000 /
深さ 64** を超えると、その木は描かずにエラーの箱だけを出す(入れ子の `ForEach` は繰り返しの数の積で数える)。
長い一覧は 1 つの木に全部を入れず、ページを分けるか、patch で必要な分だけ差し替える。

## 画像の読み込み(rasq-client D-407 / D-454、セキュリティ確認 2026-10-09 [11])

プラグインが置いた画像の URL(`Image` / `Avatar` / `MediaGrid` / 絵文字 / アイコンなど)は、`net.fetch` と同じく
マニフェストの `network` で宣言した先だけをホストが読む(制限モード / `network` 無しは `data:` だけ)。
- **リダイレクトの先も確かめる**(端末版): ホストが 1 ホップずつ追い、転送先も宣言した先でなければ送らずに
  代わりの表示にする(最大 5 回、本文 32 MiB まで)。
- **Web 版はリダイレクトを追わない**: ブラウザでは転送先を確かめてから送れないので、3xx の応答は画像にしない。
  また画像は `fetch` で読むので、**接続先に CORS のヘッダ(`Access-Control-Allow-Origin`)が要る**。
- ホストが渡した投稿のメディアの URL は、この制限の外(SNS 側の URL としてそのまま読む)。
