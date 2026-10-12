# RASQ スロットプラグイン契約 v0.22(正典)

> **ここが正典。** 以前は `orgate-question/plugin-sample/*/CONTRACT.md` に同じ文書が複製されていたが、v0.8 から
> 各リポジトリの `CONTRACT.md` はこのファイルへのポインタだけになった。節番号(§2.1〜§2.9 / §3 / §4 / §5)は複製時代のまま。
> スロットの一覧と機械可読な契約(ティア / call / ホスト関数 / 権限 / ライフサイクル)は [`../schema/slots.json`](../schema/slots.json)
> (D-005)、マニフェストは [`../schema/manifest.schema.json`](../schema/manifest.schema.json) v0.5、ActionRef / ActionKey は
> [`../schema/action-ref.schema.json`](../schema/action-ref.schema.json)(D-006)。文章とスキーマがずれたら **スキーマを直してから文章を直す**。

[`../PROTOCOL.md`](../PROTOCOL.md)(wasm ABI / RRL / FOW / ホスト関数)の上に、**アプリの各所へ
差し込む「スロット」**を定義する。プラグインは wasm32 cdylib(`rasq-plugin-kit` の
`rasq_plugin!` で export)で、ホスト(rasq-client D-167〜)は「WASMプラグイン試験 (dev)」の
**スロットに登録**で読み込み、宣言されたスロットの場所から `call` する。

原則:

- **データはすべて `call` の args で渡す**(ホスト関数で状態を問い合わせない)。
- プラグインからホストへの働きかけ(共有 / 置換 / コピー / 通知)は**ホスト関数**で、
  **fire-and-forget**(戻りは受理 `{}` だけ。表示や反映は非同期)。
- 画像などのバイナリは JSON 内の **base64**(`bytesB64` + `mimeType`)。
- UI はすべて **FOW**(`fow.open`、`fow.widget` 権限)か、スロットが受け取る RRL。
- 未対応の関数は `C01` を返す(ホストは該当 UI を隠す)。

## 1. マニフェスト — `call("plugin.manifest", {})`

```json
{ "schema": "rasq.plugin-manifest/1",
  "id": "sample.composer-media", "name": "メディア加工(サンプル)", "version": "0.1.0",
  "kind": "action", "description": "ガンマ / 回転 / 色相 / クロップ",
  "slots": ["composer.media"],
  "permissions": ["fow.widget", "storage.safe"] }
```

- 形は [`../schema/manifest.schema.json`](../schema/manifest.schema.json)(v0.5)。`kind` はその enum(`sns` / `action` / `view` / `trend` / `mediaViewer` / `postProcessor`。wasm を持たない配布物の `assets`(§2.10)/ `theme`(§2.12)も)。
  ページ型(`slots: ["page"]`、§2.13)は `kind: "view"` で、`page`(`mode` / `surfaces` …)を書く(v0.18)。
- `slots` = [`../schema/slots.json`](../schema/slots.json) にある id(下表は `stable` のもの。`planned` の面は id だけ予約)。複数可。
  どの `kind` がどのスロットを宣言できるか、必要な権限は登録簿が持ち、`python3 schema/validate.py` が突き合わせる。
- `permissions` = ホスト関数の権限(`fow.widget` / `storage.safe` / `network` / `mute.words` …)。
  ホストはスロット登録時にユーザーへ表示し、許可した分だけ通す。

| slot | 差し込み先 | ホスト → プラグイン(`call`) | プラグイン → ホスト(ホスト関数) |
|---|---|---|---|
| `astell.action` | AsTell ルーム FOW 下段の **プラグインアクション** ボタン | `astell.open` / `astell.event` | `astell.share` / `sound.play` / `toast` |
| `composer.media` | コンポーザー添付タイルの **プラグインで加工**(全画面エディター) | `media.catalog` / `media.edit` / `media.action` / `media.commit` / `media.cancel` | `composer.media.preview` / `composer.media.replace` / `toast` |
| `composer.text` | コンポーザー本文の変化(デバウンス 200ms) | `text.changed` | `composer.text.replace` / `toast` |
| `viewer.overlay` | メディア全画面ビューワー(画像の上に重ねる) | `viewer.open` / `viewer.close` | `viewer.overlay.set` / `clipboard.set` / `url.open` / `mute.add` / `toast` |
| `post.transform` | 全 TL の表示直前(投稿加工) | `post.transform` | — |
| `tl` | ホーム TL のカスタムタブ | `tl.timelines` / `tl.sort` | — |
| `trend.suggest` | 検索ホームのトレンド節(サジェスト行) | `trend.suggest` | — |
| `post.share` | 投稿の **共有 FOW** の「プラグイン」節(v0.3、宣言だけで自動登録) | `post.share` / `post.share.close` | `composer.open` / `clipboard.set` / `url.open` / `astell.share` / `toast` |
| `post.action` | 投稿の **アクションバー**(v0.7、手動の投稿加工ボタン。タグで希望スロットを宣言 → 既存ボタンのバンドルへ) | `post.action` | `composer.open` / `clipboard.set` / `url.open` / `astell.share` / `toast` |
| `page` | **ページ型(カスタムプラグイン)**: 単独のページ / AsTell のルーム / プロフィールのタイル(v0.18、§2.13。`manifest.page` で `solo` / `shared`) | `page.open` / `page.event` / `page.close` | `page.render` / `shared.set` / `shared.op` / `shared.call` / `fow.open` / `storage.*` / `toast` / `url.open` |

共通: `lifecycle {kind, surface?}` / FOW の `onClosed` は PROTOCOL どおり。

## 2. スロット別の詳細

### 2.1 `astell.action` — AsTell プラグイン

**ボタンの宣言(v0.15、rasq-client D-328)**: ほかのアクションバーと同じく、マニフェストの `actions` に `slot: "astell.action"` の
項目を書くと、**その 1 つが ルーム FOW 下段のボタン 1 つ**になる(label / icon / tags / `placement` は §2.11 どおり。
`placement` の既定 `auto` = 宣言だけで、並びの最後の 1 つ(入力欄の隣、既定では字幕)の手前に入る。`tags` が内蔵のボタン
(Speaker / Mic / Screen / Fullscreen / Captions)に似ていればそのバンドルへ。`manual` = 利用者が (+) で足すまで出ない)。
宣言が**無い**プラグインは、プラグイン名 + パズルのボタン 1 つ(手動配置)。v0.14 までの「プラグインアクション」ボタン
(登録済みのプラグインを選ぶシート)は**廃止**。

```json
"actions": [
  { "id": "panel",  "slot": "astell.action", "label": "効果音・効果映像", "icon": "celebration", "tags": ["Effect"] },
  { "id": "ping",   "slot": "astell.action", "label": "ピンポン",         "icon": "notifications_active" }
]
```

- `astell.open {token, action?, room, me, peers, shares}` — ユーザーがこのプラグインのボタンを押した。`action` = 押された
  ボタンの宣言 `id`(v0.15。宣言の無い既定ボタンでは載らない)。
  `room` = `AstellRoom` ワイヤ(`id, title, ownerId, genres, …`)、`me` = `{accountId, peerId, handle, displayName}`、
  `peers` = `[{peerId, accountId, handle, displayName, micOn}]`(自分は含まない)、
  `shares` = 直近 10 件の共有(**古い → 新しい**、`[{id, kind, payload, displayName, handle, createdAt}]`)。
  `kind: "post"` の payload はアプリの投稿共有(D-166)と同じ `{url, sns, instance, postId, author(文字列), handle, avatarUrl?, body, thumbnailUrl?, media[], createdAt?}`。
  プラグインはここで `fow.open`(操作 UI)を出す。戻り `{}`。
- `astell.event {token, type, ...}` — FOW を開いている間の通知。`type` = `share`(`share`)/ `transcript`(`transcript`)/
  `peer-joined` / `peer-left`(`peer`)/ `closed`。
- `astell.share {kind: "text"|"link"|"media"|"post", payload}` — ルームへ共有(ホストが WS の `share` にする)。
  `media` は `{name, mimeType, bytesB64}` でも `{url, mimeType, title}` でも可 — `bytesB64` はホストがアセットにアップロードして URL にする。
  効果映像 = 生成した PNG/GIF を `media` で流す。
- `sound.play {kind: "click"|"alert"}` — 端末のシステム音(効果音)。ホストは `SystemSound` を鳴らす。
- 投稿タイルデータの受け渡し(投稿の … → 「AsTell で共有」経由)は `astell.event {type:"share", share:{kind:"post", payload}}` で届く。

### 2.2 `composer.media` — メディア加工(v0.10: 加工画面の下段バーのボタン宣言 + 重ねる編集パネル)

ホストの**加工画面**(添付タイルの「加工」)は 1 つ: 基本の加工(クロップ / 回転 / ペイント / テキスト / 絵文字 / フィルター /
ぼかし / 調整)はネイティブの編集、その**下段バー**に内蔵の編集と**プラグインが宣言したボタン**が並ぶ。プラグインのボタンを
押すと、加工画面の上に**編集パネル**が重なる: 中央にプレビュー(下付き FOW が開くと小さくなる)、**下付きの横方向リスト
(端はフェードアウト)= 第 2 階層**(そのプラグインの全編集ボタン)、ヘッダーに キャンセル / 適用。パラメーターが要る編集は
第 3 階層 = プラグインの下付き FOW(スライダー等)。v0.4〜v0.9 の専用画面(右の列で「プラグイン」/「ジャンル」を切り替える)は
**廃止**: プラグインの選び分けと束ねは下段バー(並べ替え / バンドル)が担う。

**下段バーのボタン宣言(v0.10)**: マニフェストの `actions` に `slot: "composer.media"` の項目を書く。

```json
"actions": [
  { "id": "glaze", "slot": "composer.media", "label": "Glaze", "icon": "auto_awesome", "tags": ["Filter"] },
  { "id": "wave",  "slot": "composer.media", "label": "波形",  "icon": "graphic_eq",  "tags": ["Audio"], "accepts": ["audio/*", "video/*"] }
]
```

- `label` / `icon`(Material Icon の名前)= ボタンの見た目。`tags` = 希望の束ね先(§2.9 と同じ規則): 内蔵の編集のタグ
  (`Transform` / `Draw` / `Text` / `Sticker` / `Filter` / `Blur` / `Tune`)と類似度 ≥ 閾値なら、その編集とのバンドルに入る。
  届かなければ単体で末尾。ユーザーは「バーの編集」で並べ替え / バンドル / 削除ができる(投稿 / 全画面のアクションバーと同じ仕組み。
  ボタンの id = `slot:<pluginId>:<actionId>`、構成は画像用と動画用で別)。
- **`accepts`** = このボタンが出る添付の種類: MIME の型(`image/*` / `video/mp4` / `audio/*`)か拡張子(`.gif`)。
  **省略 = `["image/*"]`**。音声だけのファイルは動画の加工画面で扱うので、`audio/*` のボタンは動画用のバーに出る。
  当てはまる添付が無いときボタンは出ない(要らないボタンでバーを埋めない)。
- 押すと `media.edit` に **`actionId`**(宣言の `id`)が付いて届く。戻りの `genres` に**同じ id の編集**があれば、ホストは
  セッションを開いた直後にその `media.action` を呼ぶ(ボタン = その編集への近道)。無ければパネルを開くだけ。
- 画像: ホストは今の見た目を 1 枚(PNG)に焼き込んで渡し、`media.commit` の結果を新しい背景にする。
  動画 / 音声: **元のファイルをそのまま**渡し、`media.commit` の結果で添付を差し替えて加工画面を閉じる。
- **宣言が 1 つも無い**プラグイン(v0.9 まで)は、プラグイン名 + 既定アイコンのボタン 1 つが**画像にだけ**出る
  (`actionId` は付かない)。`rasq check` は警告を出す。
- **`output`(v0.11)** = `media.commit` の結果の置き方。`"replace"`(既定)= 背景を差し替える。`"sticker"` = 結果(透過つきの画像が
  望ましい)を**ステッカーのレイヤー**として今の絵の上に置く(背景は変えない。ユーザーは動かす / 拡縮 / 回転 / 削除ができる)。
  ジェネレーター(QR コード、吹き出し、枠、AI 生成の切り抜きなど)向け。入力は従来どおり今の見た目の 1 枚(無視してよい)。
  加工画面のステッカー一覧にも「プラグインで作る」として並ぶ。動画 / 音声のバーでは `replace` と同じ扱い。
- **`output: "filter"`(v0.12)= カスタムフィルター**。ボタンは ON / OFF(もう一度押すと外れる)。編集パネルは開かない。
  - 押すと **`filter.preview {actionId, width, height, seed}`**(`width` / `height` = 今の絵の画素数、`seed` = そのフィルターを
    掛けている間変わらない乱数の種 = ランダム配置でも絵が毎回変わらない)。戻り:
    ```json
    { "mode": "overlay", "ops": [
        { "op": "rect",   "x": 0, "y": 0, "w": 1, "h": 1, "color": "#FF8800", "opacity": 0.18, "blend": "overlay" },
        { "op": "circle", "x": 0.7, "y": 0.3, "r": 0.2, "color": "#FFFFFF", "opacity": 0.5, "blend": "screen", "blur": 0.05 },
        { "op": "ring",   "x": 0.5, "y": 0.5, "r": 0.35, "width": 0.02, "color": "#FFE9A8", "opacity": 0.8, "blend": "plus", "blur": 0.01 }
    ], "matrix": [1,0,0,0,0, 0,1,0,0,0, 0,0,1,0,0, 0,0,0,1,0] }
    ```
    - `ops` = **描画リスト**(ホストが CustomPaint で描く。座標と長さは**絵の幅・高さに対する 0..1**、`r` / `width` / `blur` は短辺に対する割合)。
      `op`: `rect`(`x,y,w,h`、`radius?`)/ `circle`(`x,y,r`)/ `ring`(`x,y,r,width` = 光輪)/ `line`(`x,y,x2,y2,width`)。共通: `color`(`#RRGGBB` /
      `#AARRGGBB`)、`opacity`(0..1)、`blend`(`normal` / `multiply` / `screen` / `overlay` / `plus` / `softLight` / `color`)、`blur`。
      **マトリックス状 / ランダム配置**はプラグインが op を並べて作る(上限 **2000 個**。超えたぶんは捨てる)。
    - `matrix` = 5×4 の色行列(省略可)。**プレビューでは効かない**ことがある(ホストは絵そのものを持っていない)ので、色調を変えるなら
      `mode: "pixels"` にして `filter.apply` でやるか、`blend` 付きの `rect` で近づける。
    - `mode`: **`"overlay"`** = レイヤー重ね。完了のとき**ホストがこの描画リストをそのまま焼き込む**(プラグインは呼ばれない)。
      **`"pixels"`** = ピクセルの書き換え。描画リストは**プレビューだけ**で、完了のときホストが(ほかの編集と overlay のフィルターを
      焼き込んだ)1 枚を **`filter.apply {actionId, seed, width, height, mimeType, bytesB64}`** に渡し、戻り `{bytesB64, mimeType}` を結果にする。
      複数の pixels フィルターは押した順に直列で通す。`filter.apply` が失敗したフィルターは**プレビューの見た目を焼き込まずに**飛ばし、
      ホストがその旨を出す(半端な結果を黙って出さない)。
  - **パラメーター(v0.13)**: `filter.preview` の戻りに `params` を書くと、ホストが**調整の FOW**(スライダー)を出す。
    ```json
    "params": [
      { "id": "count",    "label": "個数", "min": 4,   "max": 80, "step": 1,    "value": 36 },
      { "id": "strength", "label": "強さ", "min": 0.0, "max": 1,  "step": 0.05, "value": 0.5 }
    ]
    ```
    値が変わるたびにホストは `filter.preview {actionId, width, height, seed, params: {count: 50, strength: 0.5}}` を呼び直して
    描画リストを差し替える(`seed` は同じ = 配置は変わらず、個数や強さだけが変わるように作るのが望ましい)。戻りの `params` の
    `value` は**いまの値**を返す(ホストは戻りの値でスライダーを合わせ直す = プラグイン側で丸めてよい)。`filter.apply` にも
    同じ `params` が付く。ボタンは、パラメーターのあるフィルターでは「もう一度押す = FOW を開き直す」(外すのは FOW の「外す」)、
    無いフィルターでは従来どおり ON / OFF。
  - **オブジェクト(v0.13)**: 戻りに `"object": {"x": 0.5, "y": 0.3, "size": 0.7}` を書くと、描画リストは**絵いっぱいではなく、動かせる
    オブジェクト**になる(光輪など)。`x` / `y` = 中心(絵に対する 0..1)、`size` = 一辺(**短辺**に対する割合)の正方形で、`ops` の座標は
    **その正方形に対する 0..1**。ユーザーは移動 / 拡縮 / 回転ができ、完了のときその位置で焼き込まれる。`object` は最初に置くときだけ
    使う(パラメーターを変えて呼び直しても、ユーザーが動かした位置はそのまま)。`mode: "pixels"` とは併用できない(ホストは `overlay` として扱う)。
  - ホストはフィルターごとに **掛かっているか / 完了時に apply が要るか / いまのパラメーター** を持つ(フラグ管理)。プレビューのレイヤーは動かせず、
    書き出しのとき pixels のぶんは外してから焼き込む。動画 / 音声のバーには出ない(v0.12 は画像だけ)。
- `accepts` / `output` は `slot: "composer.media"` の宣言にだけ書ける。`actions[].slot` は `manifest.slots` にあるスロットだけ。
- **初期状態(v0.14, §2.11 temp)**: `output: "filter"` のボタンは `auto`(省略 = `false`)で、加工画面を開いた瞬間にオンで
  始めるか決められる。`replace` / `sticker` は編集セッションを開くので `auto` は書けない(`rasq check` が止める)。

**ジャンル**は既定 5 種 + カスタム。既定の id は固定(ホストがラベル / アイコンを持つ):

| id | ラベル | 例 |
|---|---|---|
| `tone` | 色調 | ガンマ / 色相 / 彩度 / リセット |
| `process` | 加工 | ぼかし / シャープ / モザイク / フィルター |
| `transform` | 変形 | 回転 / 反転 / クロップ / リサイズ |
| `add` | 追加 | テキスト / スタンプ / 枠 |
| `draw` | 描画 | ペン / 塗り |

カスタムジャンルは任意の id(既定と被らないもの)で `label` / `icon` を付けて宣言する。

- `media.catalog {}` — セッション無しの静的な一覧 `{genres: [{id, label?, icon?, actions: [{id, label, icon?}]}]}`。
  既定ジャンルは `label` / `icon` を省略してよい。**v0.10 のホストは呼ばない**(「ジャンル」の見方が無くなったため)。
  ツール(`rasq call`)や将来の検索用に残してよい。
- `media.edit {token, index, name, mimeType, actionId?, width?, height?, bytesB64}` — 添付 1 枚。`actionId` = 押された
  ボタンの宣言 id(v0.10)。戻り:
  - **エディター形**: `{genres: [...]}`(`media.catalog` と同じ形)。
  - すぐ返す形(UI 無し): `{bytesB64, mimeType, name?}` → その場で差し替え。
  - 旧形(`{}` を返して自分で `fow.open`)も動く(FOW はエディターの下付きになる)。
- `media.action {token, genre, action}` — 第 2 階層のボタン。即時の操作(回転など)ならその場で処理して
  `composer.media.preview` を呼ぶ。パラメーターが要るなら `fow.open`(第 3 階層 = **エディターの下付き FOW**、
  スライダー等)。スライダーの `change` ごとにプレビューを更新してよい(長辺 ≤ 360 の小さい画像で)。戻り `{}`。
- `composer.media.preview {token, bytesB64, mimeType, width?, height?}`(ホスト関数)— 中央のプレビューを差し替える(何度でも)。
  `width` / `height` = プレビュー画像の画素数(重ねの座標系。省略時はホストが復号して測る)。
- `composer.media.overlay {token, root}`(ホスト関数、v0.5)— **プレビューの上に重ねる RRL**(回転 / クロップのハンドル、
  貼り付けたオブジェクトの枠、ペンの受け皿など)。`root` = `Stack`(`p.width` / `p.height` = プレビューの画素数。ホストが表示倍率に
  合わせて拡縮する)。子は `Box` / `Image` / `Icon` / `Text` を `left/top(/width/height)` で置き、`on: {tap, drag}` で受ける
  (§3)。`root: null` で消す。プレビューを差し替えても重ねは残る(座標系が変わるなら重ねも出し直す)。
- `media.commit {token}` — 適用。**戻りに本サイズの結果** `{bytesB64, mimeType, name?}` を返す
  (ホストがパネルを閉じて結果を受け取る)。`composer.media.replace` でもよい。
  **プラグインをまたぐ編集**(v0.10): 適用して加工画面に戻り、別のプラグインのボタンを押す(結果は焼き込まれて次の
  `media.edit` の入力になる = 直列合成)。パネルの中でのプラグイン切り替えは無い。
- `media.cancel {token}` — キャンセル(FOW を閉じ、状態を捨てる)。戻り `{}`。
- 対象の `mimeType` は宣言の `accepts` の範囲(省略なら画像。ホストの焼き込みは `image/png`)。受けないものは `C01` でよい。`name` は差し替え後のファイル名(拡張子を変えてよい)。
- **ホスト側の分担(rasq-client D-173 → D-249)**: 基本の加工はネイティブの pro_image_editor が担う。プラグインは wasm / ONNX /
  アセットに無いフィルターなど、ネイティブに無い処理に向く。
- **合わせる先(バウンディングボックス、v0.6)**: 重ねの `Box` はホスト側で**ダブルタップ → ドロップダウン**
  「この上にドラッグ(合わせる先にする)」/「合わせる先を選ぶ…」(FOW: 重ねの Box 一覧から複数選択)/「解除」が付く。
  合わせる先に指定された Box はホストが点線で強調する。以降、`drag` を持つノードの **`end` イベント**で、指を離した位置
  (ノード左上 + `x,y`)が合わせる先の中にあれば payload に **`snap: {key, label?, left, top, width, height, angle}`**
  (プレビュー画素、`angle` は度)が付く。プラグインはそれを受けて自分のオブジェクトの矩形 / 角度を合わせる
  (スタンプは比率を保って内側に収める、図形は矩形そのまま、など。合わせ方はプラグインの裁量)。
  ホストが Box を一覧に出せるように、重ねの Box には **`p.label`**(表示名: 「スタンプ 1」「顔 2」「クロップ範囲」…)を、
  回転しているものには **`p.angle`**(度)を付ける。`label` の無い Box は一覧に出ない(ハンドルなど)。
- サンプル(v0.5): `media-transform-plugin`(変形: 回転 / クロップのハンドル)、`media-process-plugin`(加工: 顔検知ぼかし、
  スタンプのオブジェクト = 移動 / 拡縮 / 回転 / 削除 / 複製)、`media-draw-plugin`(描画: ペンの軌跡をスプラインで描く、四角 / 丸のオブジェクト)。
- `SliderListTile` はホスト側で**楽観更新**する(ドラッグ中はつまみと数値が動く)。プラグインが `setProp` で `value` を
  書き戻せばそれが優先される。

### 2.3 `composer.text` — テキスト加工 / 補完

- `text.changed {token, text, selectionStart, selectionEnd, trigger?}` — 本文が変わった(デバウンス)。
  `trigger` = カーソル直前のトークン(`#タグ` / `@handle` / `:emoji`)があればホストが**文字列**で渡す
  (範囲は `[selectionStart − trigger.length, selectionStart)`、UTF-16 code unit)。空白/改行/全角空白で区切る。
  プラグインは条件(例: `#` で始まるトークン)を見て `fow.open`(候補一覧)し、そうでなければ `fow.close`。戻り `{}`。
- `composer.text.replace {token, start, end, text}` — 本文の `[start, end)` を `text` に置き換える(UTF-16 code unit オフセット、Dart の `String` と同じ)。
- ホストは FOW をコンポーザーの子(`parentId` 無しでも同じカラム)として開くので入力は隠れない。
- **ON / OFF(v0.14, §2.11)**: ホストは入力欄の上に、`composer.text` のプラグインごとの ON / OFF チップを出す(キーは
  `slot:<pluginId>:composer.text`)。見た目は `actions` に `slot: "composer.text"` の項目があればそれ(`label` / `icon`)、無ければ
  `name`。初期状態は `auto`(省略 = `true`)。オフのあいだ `text.changed` は呼ばれない。

### 2.4 `viewer.overlay` — 全画面ビューワーの重ね

- `viewer.open {token, url, mimeType, width, height, bytesB64?}` — 全画面表示が開いた。`bytesB64` は画像本体
  (ホストが取れた場合のみ。大きい画像はホストが長辺 1024 に縮小してから渡し、`width/height` はその縮小後の画素数)。
  戻り `{root?: RRL}` — 画像の上に重ねる RRL(座標は `width/height` の画素系。ホストが表示倍率に合わせて拡縮)。
  後から差し替えるときは `viewer.overlay.set {token, root}`。
- `viewer.close {token}` — 閉じた(FOW も閉じること)。
- `viewer.open` の失敗(壊れた入力 = `D01`)はホストが黙って無視する(重ねない)。「何も無い」は `{}`。
- ホストは重ねる `Stack` を画像の画素数(`width/height`)のサイズで置き、表示倍率に合わせて拡縮する。
- 重ねる RRL は `Stack` の子に **`Box`**(ホスト拡張ノード、§3)を `left/top/width/height` で置く。`on: {tap}` でプラグインへ戻る。
- リンク FOW からは `clipboard.set {text}` / `url.open {url}` / `mute.add {text}`(権限 `mute.words`。ホストは確認してからミュート語に追加)。
- **全画面アクションバーのボタン宣言(v0.9)**: `viewer.overlay` を宣言したプラグインは、全画面(投稿のメディア / プロフィールの
  アイコン・バナー)のヘッダーの**全画面アクションバー**に ON / OFF ボタンとして並ぶ(rasq-client D-214 / D-246)。その見た目を
  マニフェストの `actions` に **`slot: "viewer.overlay"`** の項目で宣言する:

  ```json
  "actions": [
    { "id": "toggle", "slot": "viewer.overlay", "label": "QR コード検出", "icon": "qr_code_scanner", "tags": ["全画面表示", "QR"] }
  ]
  ```

  `label` はツールチップと「アクションリストの編集」の検索に、`icon`(Material のアイコン名)はボタンに、`tags` は編集画面の
  検索に使う。押したときの `call` は無い(ホストが有効 / 無効を切り替え、有効なら画像を渡す)。
  **初期状態(v0.14)**: 全画面が開いた瞬間のオン / オフは `actions[].auto`(省略 = `true`)。面の中で切り替えた値は
  その全画面限りで、次に開いたときは既定に戻る(§2.11 temp)。v0.13 までの「切り替えを `slots.viewer` に保存して次も引き継ぐ」は
  やめた。利用者が「次からも自動でオン / オフ」を選べば、ホストの設定(`slots.global.auto`)が宣言の既定に勝つ。
  宣言が無ければホストが `name` と既定のアイコンで作る(v0.8 までのプラグインもそのまま並ぶ)。保存用のキーは
  ActionKey と同じ形の `slot:<pluginId>:viewer.overlay`(`id` には依らない)。`actions[].slot` を省略した項目は従来どおり
  `post.action` のボタン。
- ホストはボタンに**状態**を出す(D-246): 処理中 / 検出あり / 検出なし / 失敗(理由)。`viewer.open` がエラー(`D01` など)を
  返したときは重ねないのは同じだが、理由はユーザーに見える。

### 2.5 `post.transform` — 投稿加工([PROTOCOL §4.4](../PROTOCOL.md) と同じ)

- `post.transform {posts: [Post ワイヤ]}` → `{posts: {postId: {body?, cw?}}}`。載っていない投稿は無加工。

### 2.6 `tl` — タイムライン = **アルゴリズムノード**(v0.2)

TL プラグインはタブではなく、簡略カスタマイズ UI / ノードグラフの**アルゴリズム部品(ソートノード)**として並ぶ。
パラメーターは「ノードを設定」FOW にプラグインの RRL 設定ページが差し込まれる。

- `tl.timelines {}` → `{timelines: [{id, label, base}]}`(`label` = 部品名「〇〇でソート」の元。`base` は互換用)。
- `tl.params {id}` → `{root: RRL, defaults: {key: value}}` — 設定 DSL のサブセット(`SectionHeader` / `RadioListTile` /
  `SwitchListTile` / `SliderListTile` / `ValueListTile` / `Text`)。値は Bind `$.settings.<key>`、書き込みは
  `on: {"change": "settings.set"}`(ホストが部品のパラメーターに保存する。ノード key = パラメーター名)。無ければ `C01`。
- `tl.sort {id, posts, params}` → `{order: [postId…]}` — `params` は保存されたパラメーター(無ければ `defaults`)。

### 2.7 `trend.suggest` — トレンドのサジェスト

- `trend.suggest {recent: [string], query?: string, limit?: number}` → `{suggestions: [{text, reason?}]}`。
  `recent` = 検索ホームが今持っているトレンド語(タグ)。サーバー無しでも動くよう、ローカルの規則(共起 / 時間帯 / 固定辞書)で返す。

### 2.8 `post.share` — 投稿共有(v0.3: 共有 FOW からインラインで編集 & 共有)

投稿のアクションバー右端(共有)で**共有 FOW** が開く: ヘッダー左にアカウント選択(非公開投稿は受け取った
アカウントだけ)、本体に 内容をコピー / リンクをコピー / 他のアカウントで共有(選んだアカウントのコンポーザーに
リンクを初期入力)、**プラグイン**節、DM へ共有、ブラウザで開く。`post.share` を `slots` に宣言したプラグインは
**権限(マニフェスト)から自動で**プラグイン節に 1 行ずつ並ぶ(`name` / `description`)。

- `post.share {token, post, account, link}` — ユーザーがこのプラグインの行を選んだ。
  `post` = §2.1 と同じ投稿 payload(`{url, sns, instance, postId, author, handle, avatarUrl?, body, thumbnailUrl?, media[], createdAt?}`)、
  `account` = ヘッダーで選んだアカウント `{sns, instance, handle}`、`link` = 投稿の Web URL。
  プラグインはここで `fow.open`(**共有 FOW の子 = インライン**。共有 FOW が閉じれば連動して閉じる)を出し、
  文面の編集(テンプレート / 引用の付け方 …)と共有先のボタンを描く。戻り `{}`。
- `post.share.close {token}` — 共有 FOW が閉じた(プラグインは FOW を閉じて状態を捨てる)。戻り `{}`。
- `composer.open {token, text, mediaB64?, mimeType?, name?}`(ホスト関数)— **選んだアカウントのコンポーザー**を
  `text` を初期入力して開く(画像を添えるときは `mediaB64` + `mimeType`)。ホストは共有 FOW とプラグイン FOW を閉じてから開く。
- ほかに `clipboard.set` / `url.open` / `astell.share`(参加中の AsTell があれば)/ `toast`。

### 2.9 `post.action` — 投稿のアクションボタン(v0.7: 手動系の投稿加工、希望スロット宣言)

`post.transform`(自動、表示前に本文を書き換える)と違い、**ユーザーが押して動く**ボタンを投稿のアクションバーに足す。
マニフェストに **`actions`** を宣言する(`slots` に `post.action` を含める):

```json
"actions": [
  { "id": "later", "label": "後で読む", "icon": "bookmark_add", "tags": ["Favorite", "Bookmark", "Star"] },
  { "id": "copy",  "label": "本文をコピー", "icon": "content_copy", "tags": ["Copy", "Share"] }
]
```

- **希望スロット = `tags`**(複数)。ホストはアカウントごとに、SNS の既存アクションボタンのタグ(`genreTag`: `Reaction` / `Repost` /
  `Reply` / `Quote` / `Favorite` …と、ホストが持つ別名)と比べ、類似度が**閾値(既定 0.85)以上**なら、その既存ボタンの
  **バンドルの最下層**に足す(既存ボタンがユーザーのアクションリストに入っていればそのリストの末尾、入っていなければ
  既存ボタン + プラグインボタンの新しいバンドルを作る)。どれにも届かなければ単体ボタンとして末尾に並ぶ。
  類似度 = タグ同士の最大値(大文字小文字を無視した完全一致 = 1.0、それ以外は文字 2-gram の Dice 係数)。
- **配置の宣言(v0.14, §2.11 placement)**: 上の自動配置は `placement: "auto"`(既定)。`placement: "manual"`(または `tags` を
  省いた項目)は**自動では置かれず**、利用者が「プラグインボタンを検索」FOW で足すまで出ない。宣言しないことで自動配置を
  防ぐ、という使い方(利用者が選んだときだけ出るボタン)。
- **初期状態(v0.14, §2.11 temp)**: `auto: true` のボタンは、投稿タイルが作られたときに**そのタイルで 1 回**、
  `post.action {…, "auto": true}` として自動で呼ばれる(FOW は開かないこと。`message` はトーストに出ない)。書けるのは
  `permissions` に **`auto_hook`** を持つ(= 審査で自動実行を認められた)プラグインの、`destructive` でも `autoHookForbidden`
  でもないボタンだけ。省略 = `false` = 利用者が押すまで動かない。v0.13 までの「auto_hook = フックの仕組み」はこれに置き換わり、
  権限の名前だけ残る。
- アカウントごとの **スロット設定**(「アクションリストの編集」の「プラグインボタン」節)で、ボタンごとの有効 / 無効と閾値を変えられる。
  「プラグインボタンを検索」FOW で登録済みプラグインの `actions` から探して足せる。既存ボタンの … → 「置き換え」で
  既存ボタンの位置にプラグインボタンを入れ替えられる。
- `post.action {token, actionId, post, account}` — ユーザーが押した。`post` は §2.1 の投稿 payload、`account` = `{sns, instance, handle}`。
  戻り `{result?: {done?, message?}}`(`message` はホストがトーストに出す)。UI が要るなら `fow.open`(投稿の面と同じカラムに
  インラインで出る)。ホスト関数は `composer.open`(引用 / 返信の文面をコンポーザーへ)/ `clipboard.set` / `url.open` /
  `astell.share` / `toast`。
- ボタンの id はホスト側で `slot:<pluginId>:<actionId>` になる(非表示 / 並び順の保存に使う)= **ActionKey**
  ([`../schema/action-ref.schema.json`](../schema/action-ref.schema.json)、D-006)。版を含めないので、プラグインを更新しても設定が残る。
  実行時にホストが使う完全修飾の `ActionRef`(`pluginId:accountKey:pluginVersion:fn`)とは別物。

### 2.10 アセットパック — `kind: "assets"`(v0.11、スロットではない)

**wasm を持たない配布物**。中身は画像だけで、ホストは読むだけ(コードは動かない / 権限も無い)。そのぶん審査は軽い
(画像の内容と権利の確認だけ)。今の使い道は加工画面の**ステッカー**(`use: "sticker"`)。

```json
{
  "schema": "rasq.plugin-manifest/1",
  "id": "net.example.stickers", "name": "ねこステッカー", "version": "0.1.0", "kind": "assets",
  "description": "ねこのステッカー 12 枚",
  "assets": [
    { "id": "cat-1", "path": "stickers/cat-1.png", "use": "sticker", "label": "ねこ 1", "tags": ["cat"] }
  ]
}
```

- `assets[]`: `id`(パック内で一意)/ `path`(パックのディレクトリからの相対パス = 配布物の中のパス)/ `use` / `label` / `tags`。
- 画像は **PNG / JPEG / WebP / GIF**(中身のマジックナンバーで判定、拡張子と一致すること)、**1 枚 2 MiB・合計 16 MiB・256 枚**まで。
- `slots` / `permissions` / `actions` / `roles` は書けない。土台(SNS プラグイン)にも依らない(`requires` 無し)。
  ほかの `kind` のマニフェストは `assets` を書けない(プラグインに同梱するファイルは rasq.toml の `[[assets]]`)。
  例外は着せ替え(`kind: "theme"`、§2.12、v0.16)で、同じ `assets[]` に `use: "theme"` で画像を載せる(形式と上限はここと同じ)。
  アセットパックの `use` は `sticker`、着せ替えの `use` は `theme` だけ(混ぜられない)。
- 配布物(`.rspkg`): `bundle.json` の `root` は `{id, version, kind: "assets", manifest}` で **`wasm` を持たない**。`children` / `libs` /
  `models` は空、`assets[]` は全部 `kind: "image"` で `sha512` / `sizeBytes` 必須([`bundle.schema.json`](../schema/bundle.schema.json))。
  署名 / ハッシュの扱いはほかの配布物と同じ。
- CLI: `rasq new assets <name>` → `packs/<name>/`(`manifest.json` + `stickers/`)、rasq.toml に `[[packs]]`。`rasq check` は
  スキーマ / 形式 / 大きさ / 書き忘れ(ディレクトリにあるのに `assets` に無い画像 = 警告)を見る(wasm のビルドは要らない)。
  `rasq build` はパックごとに配布物 1 つ、`rasq publish` は掲載のカテゴリを `assets` にする。コードのプラグインと同じ
  ワークスペースに置ける。
- ホスト(rasq-client): インストールすると画像を端末に保存し、加工画面の「ステッカー」に**パックごとの節**で並ぶ。
  ホストは配布物に wasm が 1 つでも入っていたらアセットパックとして扱わない(入れ物に紛れ込ませない)。

### 2.11 ボタンの共通属性(v0.14): switch / temp(`auto`)/ placement

投稿タイル(`post.action`)・全画面(`viewer.overlay`)・加工画面(`composer.media` の filter)・コンポーザー本文(`composer.text`)の
プラグインボタンは、**ボタン(ActionKey = `slot:<pluginId>:<actionId>`)単位**で次の 3 つを持つ。どれも直交する。

| 概念 | 意味 | 宣言(プラグインの既定) | 利用者の設定(上書き) |
|---|---|---|---|
| **switch** | 端末全体の主電源。オフなら全アカウント・全面で出ず、自動でも動かない | 無し(常に既定オン) | 設定画面の「プラグインボタン」でボタンごとに |
| **temp**(`auto`) | 面が**作られた瞬間**の初期状態。オンで始めるか、オフ(手動)で始めるか。面の中で切り替えた値はその面限りで、永続しない | `actions[].auto`(省略 = 面ごとのホスト既定: viewer.overlay `true`、filter `false`、composer.text `true`、post.action `false`) | 設定画面と、面の中の長押し「次からも自動でオン / オフ」 |
| **placement** | 宣言だけで自動配置されるか、利用者が (+) で足すまで出ないか | `actions[].placement: "auto" \| "manual"`(`tags` 無しの post.action は `manual`) | 無し(配置そのものは従来のアクションリストの編集) |

- switch がオフなら temp は評価しない(オフが最優先)。switch がオンで temp がオフ = 面の中で手で押せば動く。
- `post.action` の temp オン = そのタイルで 1 回自動で実行(§2.9)。これは v0.13 までの `auto_hook`(フック)の置き換えで、
  権限 `auto_hook` は「`auto: true` を書ける」の意味に変わる。宣言に `auto: true` の post.action が 1 つも無いのに
  `auto_hook` を持つのは過剰宣言(審査の許容の枠内で数える)。
- 制限モード(§5.1)のサイドロードでは `auto` を読まず、ホスト既定だけを使う(`auto_hook` は `action` ティア = 審査済みのみ)。
- 宣言に `auto` / `placement` の無い既存のプラグインは、見た目・挙動が変わらない(全画面のオン / オフだけが「永続」から
  「面ローカル + 既定」になる。ホストは移行時に、保存されていた値を既定に写す)。
- `rasq check`: `auto: true` × (`destructive` | `autoHookForbidden`)は error、post.action の `auto: true` に `auto_hook` が無ければ
  error、composer.media の `auto` は `output: "filter"` だけ(replace / sticker は true でも false でも error)、`placement: "manual"` で `tags` が無ければ「検索でタグ一致しない」warning。
  スキーマ(`manifest.schema.json` v0.5)も同じ規則を持つ。

### 2.12 着せ替え(テーマ)パック — `kind: "theme"`(v0.16、スロットではない。v0.17 でグラデーション / 帯 / onColor)

アセットパック(§2.10)と同じ **wasm を持たない配布物**(rasq-client D-335)。中身はマニフェストの `theme`(宣言)と
画像(`assets[]`、`use: "theme"`)だけで、ホストは宣言を読むだけ(コードは動かない / 権限も無い)。
ホストで**同時に有効にできる着せ替えは 1 つ**(設定 → アプリの外観 → フォントやカラースキーム)。

```json
{
  "schema": "rasq.plugin-manifest/1",
  "id": "sample.theme-starry", "name": "星空", "version": "0.1.0", "kind": "theme",
  "description": "夜空の背景と星のボタン",
  "assets": [
    { "id": "sky",    "path": "theme/sky.png",    "use": "theme", "label": "夜空" },
    { "id": "stars",  "path": "theme/stars.png",  "use": "theme", "label": "星の粒(上付き)" },
    { "id": "button", "path": "theme/button.png", "use": "theme", "label": "ボタンの背景" },
    { "id": "heart",  "path": "theme/heart.png",  "use": "theme", "label": "いいねのアイコン" }
  ],
  "theme": {
    "brightness": "dark",
    "colors": {
      "dark":  { "primary": "#8AB4FF", "surface": "#0B1026", "onSurface": "#E8ECFF", "outline": "#3A4470" },
      "light": { "primary": "#3355CC" }
    },
    "icons": {
      "favorite_border": "asset:heart",
      "repeat": "material:autorenew"
    },
    "surfaces": {
      "app":          { "color": "#060914", "layers": [ { "asset": "sky", "position": "below", "fit": "cover", "opacity": 0.9 } ] },
      "postTile":     { "color": "#CC0B1026", "radius": 12, "layers": [ { "asset": "stars", "position": "above", "fit": "none", "repeat": "repeat", "opacity": 0.35 } ] },
      "fow":          { "color": "#0B1026", "layers": [ { "asset": "sky", "position": "below", "fit": "cover", "alignment": "bottomCenter", "opacity": 0.5 } ] },
      "actionButton": { "radius": 18, "layers": [ { "asset": "button", "position": "below", "fit": "fill" } ] }
    }
  }
}
```

(同じものが [`../examples/manifests/sample.theme-starry.json`](../examples/manifests/sample.theme-starry.json)、画像つきの一式は
plugin-sample の `theme-plugin`。)

**`theme` のキー**([`manifest.schema.json`](../schema/manifest.schema.json) の `$defs.theme` / `themeColors` / `themeSurface` / `themeLayer` / `themeGradient`):

| キー | 形 | 意味 |
|---|---|---|
| `brightness` | `"light"` \| `"dark"` \| `"both"`(既定 `both`) | どちらの明るさのときに効くか。アプリの明暗設定と合わない側では `colors` と `surfaces` を使わない(`icons` の差し替えは常に効く) |
| `colors` | `{ "light"?: Colors, "dark"?: Colors }` | Material の ColorScheme の上書き。Colors に書けるのは `primary` / `onPrimary` / `secondary` / `onSecondary` / `tertiary` / `error` / `surface` / `onSurface` / `onSurfaceVariant` / `surfaceContainer` / `surfaceContainerHigh` / `surfaceContainerHighest` / `outline` / `outlineVariant` だけ(ほかは不可) |
| `icons` | `{ "<Material のアイコン名>": "asset:<assets[].id>" \| "material:<Material のアイコン名>" }` | 名前で持っているアイコン(アクションバー / バンドル / 全画面バー / AsTell の下段など、`icon: "favorite_border"` のように名前で持つもの)を差し替える。キーと名前は `^[a-z0-9_]+$` |
| `surfaces` | `{ "app"?, "postTile"?, "fow"?, "actionButton"?: Surface }` | 面ごとの見た目(ほかのキーは不可。今の面はこの 4 つ) |

- 色は `#RRGGBB` か `#AARRGGBB`(8 桁は**先頭**が不透明度。Flutter の `Color(0xAARRGGBB)` / media filter の色と同じ並びで、
  例の `#CC0B1026` = `#0B1026` の 80%。§3 の RRL の色 `#RRGGBBAA`(末尾が不透明度)とは逆なので注意)。
- **面(surfaces)**: `app` = メインの画面の背景、`postTile` = 投稿タイル、`fow` = FOW(下から出るパネル)の背景、
  `actionButton` = アクションバーのボタン。
- **Surface** = `{ "color"?: 色, "onColor"?: 色, "radius"?: 0〜48 の数, "layers"?: Layer[](最大 8)}`。`color` は画像の下に敷く塗り、
  `radius` は角丸。**`onColor`(v0.17)** = その面の上の**文字とアイコンの色**(濃い背景を敷いた面で、文字を読めるようにする)。
- **Layer** = 面に重ねるもの 1 枚。書いた順に下から描く。**`asset`(画像)か `gradient`(グラデーション、v0.17)のどちらか 1 つ**
  (両方 / どちらも無しは不可)。

| キー | 値(既定) | 意味 |
|---|---|---|
| `asset` | `assets[].id` | `use: "theme"` の画像。`gradient` とはどちらか 1 つ |
| `gradient` | Gradient(v0.17) | 線形グラデーション(下記)。`asset` とはどちらか 1 つ |
| `height` | 1〜400 の数(無し = 面いっぱい) | v0.17。書くと面いっぱいではなく **`alignment` の辺に寄せた高さ `height`(論理 px)の帯**になる(`topLeft` / `topCenter` / `topRight` → 上辺、`bottom*` → 下辺、`center*` → 上下の真ん中)。画像にもグラデーションにも使える |
| `position` | `below` \| `above`(`below`) | `below` = 中身の下(背景)、`above` = 中身の上に重ねる。**`above` の層はタッチを受けない**(押せる部分を邪魔しない) |
| `fit` | `cover` \| `contain` \| `fill` \| `fitWidth` \| `fitHeight` \| `none` \| `scaleDown`(`cover`) | 面への合わせ方(Flutter の `BoxFit`)。画像のときだけ |
| `alignment` | `topLeft` \| `topCenter` \| `topRight` \| `centerLeft` \| `center` \| `centerRight` \| `bottomLeft` \| `bottomCenter` \| `bottomRight`(`center`) | 寄せ方(`Alignment`) |
| `repeat` | `none` \| `repeat` \| `repeatX` \| `repeatY`(`none`) | 敷き詰め(`ImageRepeat`)。`fit: "none"` と組み合わせてタイル状に。画像のときだけ |
| `opacity` | 0〜1(1) | 不透明度。画像にもグラデーションにも効く |

- **Gradient**(v0.17、Flutter の `LinearGradient`)= `{ "colors": [色, …](2〜8 個), "stops"?: [0〜1 の数, …], "begin"?: alignment(既定 `topCenter`), "end"?: alignment(既定 `bottomCenter`) }`。
  色は Surface と同じ `#RRGGBB` / `#AARRGGBB`。`stops` は `colors` と**同じ数で昇順**(省略 = 等間隔)。`fit` / `repeat` はグラデーションでは
  無視される(`rasq check` は書いてあれば警告)。

例(v0.17、サンプル「RASQ」= [`../examples/manifests/sample.theme-rasq.json`](../examples/manifests/sample.theme-rasq.json)): 投稿タイルの上辺に
高さ 28 の帯(不透明の `#1F1E33` → 同じ色の不透明度 0 `#001F1E33` = 投稿の背景色に溶ける)と右寄せの薄いマーク、FOW は緑のグラデーションに白い文字。

```json
{
  "schema": "rasq.plugin-manifest/1",
  "id": "sample.theme-rasq", "name": "RASQ", "version": "0.1.0", "kind": "theme",
  "description": "投稿の上のグラデーションの帯と薄い RASQ のマーク、緑のグラデーションの FOW",
  "assets": [
    { "id": "mark", "path": "theme/rasq_mark_faded.png", "use": "theme", "label": "薄い RASQ のマーク" }
  ],
  "theme": {
    "brightness": "both",
    "surfaces": {
      "postTile": {
        "layers": [
          { "asset": "mark", "position": "below", "fit": "contain", "alignment": "centerRight" },
          { "gradient": { "colors": ["#1F1E33", "#001F1E33"] }, "position": "below", "alignment": "topCenter", "height": 28 }
        ]
      },
      "fow": {
        "color": "#02775C",
        "onColor": "#FFFFFF",
        "layers": [
          { "gradient": { "colors": ["#02775C", "#178178"], "begin": "topCenter", "end": "bottomCenter" }, "position": "below" }
        ]
      }
    }
  }
}
```

v0.17 の追加は後方互換(v0.16 の宣言はそのまま有効)。

- **整合**(`rasq check` / `python3 schema/validate.py` / ホストが同じ規則で見る): `layers[].asset` と `icons` の `asset:<id>` は
  `assets[]` に **`use: "theme"`** で載っている id であること(グラデーションのレイヤーは参照先を持たないので見ない)。
  `gradient.stops` は `colors` と同じ数で昇順であること(スキーマでは書けないので `rasq check` / validate.py が見る)。`theme` は `kind: "theme"` のマニフェストにだけ書ける。
  `kind: "theme"` は `theme` と `assets` が必須。ただし `assets` は **0 枚でもよい**(色だけのテーマ)。
- 画像の形式と上限はアセットパックと同じ(**PNG / JPEG / WebP / GIF**、1 枚 2 MiB・合計 16 MiB・256 枚)。
- `slots` / `permissions` / `actions` / `roles` は書けない。土台(SNS プラグイン)にも依らない(`requires` 無し)。
- 配布物(`.rspkg`): `bundle.json` の `root` は `{id, version, kind: "theme", manifest}` で **`wasm` を持たない**。`children` / `libs` /
  `models` は空、`assets[]` は全部 `kind: "image"` で `sha512` / `sizeBytes` 必須(色だけのテーマなら空)。アセットパックと同じ扱い
  ([`bundle.schema.json`](../schema/bundle.schema.json))。
- CLI: `rasq new theme <name>` → `packs/<name>/`(`manifest.json`(kind = theme、上の例に近い雛形)+ `theme/` の画像 1 枚)、
  rasq.toml に `[[packs]]`。`rasq check` はアセットパックと同じ検査(スキーマ / 形式 / 大きさ / 書き忘れ)に加えて上の整合
  (asset の参照、色の形、面のキー、asset と gradient のどちらか 1 つ、stops の数と順)を見る。どこからも指されていない `use: "theme"` の画像は警告。`rasq build` はパックごとに
  配布物 1 つ(kind theme)、`rasq publish` は掲載のカテゴリを `theme` にする。
- ホスト(rasq-client): インストールすると画像を端末に保存し、設定 → アプリの外観 → フォントやカラースキーム に並ぶ。
  有効にできるのは 1 つだけ(別のものを選ぶと前のものは外れる)。`above` の層は描くだけでタッチを通す。
  ホストは配布物に wasm が 1 つでも入っていたら着せ替えとして扱わない(アセットパックと同じ)。

### 2.13 `page` — ページ型プラグイン(カスタムプラグイン、v0.18)

rasq-client D-357 / rasq-central S-048。**アプリに新しい画面(ページ)を足す**プラグイン。単独のページ、AsTell のルーム、
RASQ アカウントのプロフィールページから開ける。**1 人で完結するもの(`solo`)**と、**複数人で共有するもの(`shared`、
サーバー変数とサーバー関数を使う)**がある。wasm は**各端末で動き**、shared の状態は **Central が中継・保持**する
(プラグインはサーバーのコードを持たない。サーバーがするのは変数の保持と下の決まった演算、呼び出しの中継だけ)。

tier `page`(審査済みのみ)、kinds `["view"]`、権限は `render` が必須、`shared.session` / `storage.safe` / `network` が任意。

```json
{ "schema": "rasq.plugin-manifest/1", "id": "sample.board", "name": "みんなのボード", "version": "0.1.0",
  "kind": "view", "slots": ["page"],
  "permissions": ["render", "shared.session", "storage.safe"],
  "page": { "title": "みんなのボード", "icon": "dashboard", "mode": "shared",
            "surfaces": ["page", "astell", "profile"], "maxParticipants": 50 } }
```

(同じものが [`../examples/manifests/sample.custom-page.json`](../examples/manifests/sample.custom-page.json)、solo の例は
[`../examples/manifests/sample.custom-page-solo.json`](../examples/manifests/sample.custom-page-solo.json)、実装は plugin-sample の `custom-page-plugin`。v0.22 の TTL と落書き(`media.editor.open`)の例は
[`../examples/manifests/sample.custom-paint.json`](../examples/manifests/sample.custom-paint.json)、実装は plugin-sample の `custom-plugin-paint`。)

**`page` のキー**([`manifest.schema.json`](../schema/manifest.schema.json) の `$defs.page`。`slots` に `page` があれば必須、無ければ書けない):

| キー | 値(既定) | 意味 |
|---|---|---|
| `mode` | `solo` \| `shared`(必須) | `solo` = 1 人で完結(サーバー変数なし)。`shared` = 複数人でセッションを共有(権限 **`shared.session` が必須**) |
| `surfaces` | `page` / `astell` / `profile` / `profile.host` / `swing` の配列(`["page"]`) | どこから開けるか(下。v0.20 から Swing は書かなくても配れる。v0.21 で `profile.host`) |
| `title` | 文字列(`name`) | ページのヘッダーとタイルの名前。`page.open` / `page.render` の `title` で上書きできる |
| `icon` | Material Icon の名前 | タイル / サイドメニュー / AsTell のカード |
| `maxParticipants` | 1〜200(50) | shared の 1 セッションに入れる人数。超えると入れない(WS が 409) |
| `sharedLimits` | `{valueBytes?, totalBytes?}`(無し) | (v0.22)サーバー変数の上限を広げる(shared だけ)。下の「キーと上限」。**宣言した版は強化審査**、リリース版からは審査料が割り増し |

- **`page`** = 単独のページ(サイドメニューの「プラグイン」やストアから)。shared なら招待リンク(`link:` のセッション)で人を呼べる。
- **`astell`** = AsTell のルームに**デプロイ**する(**`shared` のみ**。スキーマが見る)。参加者は共有に `kind: "plugin"` の提案を出し、
  ホスト(ルームのオーナー / モデレーター)が承認するとルームのセッション(`room:<roomId>`)ができ、全員のルームにカードが出る。
- **`profile`** = RASQ アカウントのページにタイルとして置く(`PUT /me/pages`、8 個まで)。**solo** は訪問者それぞれの端末で完結、
  **shared** は持ち主のプロフィールごとの 1 セッション(`profile:<accountId>`)を訪問者で共有する。
  - サイドロード(未審査)のプラグインを RASQ のページに置くと**制限モード**(§5.1)で動き、置けるのは **`solo` だけ**
    (共有セッションはストアの審査済みの版にしか作れない)。
- **`profile.host`**(v0.21、rasq-client D-446)= `profile` で RASQ アカウントのページに置いた**持ち主(ホスト)だけ**が開ける**ホスト用ページ**。
  - **`profile` と一緒に書く**(`profile.host` だけはスキーマのエラー)。置いたタイルとは別に、持ち主にだけ「〜のホスト用ページ」の入口が出て、
    同じプラグインが `surface: "profile.host"` で `page.open` される。`context` は `profile` と同じ(`profileHandle` / `profileAccountId`)。
  - **訪問者には出ない**(入口もなく、`profile.host` では開かれない)。持ち主だけが変えられる設定・管理(パラメーターなど)に使う。
  - 決めた値はプラグインがふだんの保存先に持つ: 持ち主の端末だけで使うなら `storage.*`、訪問者のページに効かせるなら shared のサーバー変数
    (`profile:<accountId>` のセッション。持ち主の `role` は `owner`。持ち主だけに書かせたいキーは `@<accountId>.` で始める)。
- **`swing`**(v0.19、rasq-central S-056 / rasq-client D-377)= RASQ の **Swing**(フォロワーと RASQ リストの人へ流れて消える配信)として配る。
  - **v0.20(rasq-central S-079 / rasq-client D-424)から、親子関係の無いページ型プラグインは `swing` を書かなくても Swing で配れ、
    `shared` も配れる**(v0.19 の「solo のみ」は外した)。`swing` を書くのは「Swing 向けに作った」(全画面・`page.end`・`paused` を扱う)という宣言で、
    ホストは Swing を作る画面でそれを先に並べる。書いていないプラグインにも、Swing では `surface: "swing"` で `page.open` が来る
    (知らない surface は `page` と同じに描けばよい)。
  - **親子関係**(配れないもの): 入れ物(`.rspkg`)の中に**ほかのプラグインと一緒に入っている**もの(SNS プラグインの `children` の 1 つ =
    ホストの登録元が `…#<childId>`、または子を持つ入れ物の root)と、**ほかのプラグインを土台にする**もの(`bundle.json` の `root.requires`)。
    ページ型が自分の FOW を `fow.open` で子として開くのは親子関係に数えない。
  - **`shared` を Swing で配る** = 見る人は**作った人のセッションに入る**(Swing を見られる人 = セッションに入れる人)。作るときに
    新しいセッション(scope **`swing:<swingId>`**。Swing が消えると閉じる)か、作った人がいま入っている `link:` / `profile:` のセッション
    (カスタムプラグインのページの「招待」、rasq-client D-424)を Swing に付ける。`room:` のセッションは付けられない(ルームに入っている人だけのもの)。
    見る人がプラグインを入れていなければ、ほかの共有セッションと同じく同意のうえ一時的に動かす(`?session=`、有料でも)。
  - 見る人の端末で**全画面**に開く。見る人がプラグインを入れていなければ、ストアへ案内する。
  - 開いている間は、スワイプで次の Swing へは移れない。見る人が「全画面から縮小」を押すと、ホストはページをぼかして止め(`page.event {type: "paused"}`)、スワイプで移れるようにする。
  - 縮小したページを押すと全画面に戻る(`{type: "resumed"}`)。
  - プラグインが **`page.end`** を呼ぶか、見る人が次へ進む / 閉じると、その Swing は**見終えた**ことになり、その人には二度と出ない。
  - `page.open` の `context` に `swingId` と、作った人が入れた `data`(JSON 16 KiB まで)と、`author: {accountId, handle, displayName}` が載る。
  - **未審査のまま配れる**(rasq-central S-057 / rasq-client D-379): 作った人が `.rspkg` / `.wasm` を上げる。
    - 見る人は同意してから、**制限モード + GPU** で一時的に動かす(`network` / `storage.unsafe` / interstitial・system の FOW / `url.open` などは閉、`gpu.compute` は可、fuel は半分)。
    - 入れ物は**子プラグインを持たない 1 つだけ**(`requires` も 2 つめのプラグインも不可 = 上の親子関係が無い)。スロットは `page` だけの solo
      (v0.20: surface `swing` の宣言は要らない。共有セッションはストアの審査済みの版にしか作れないので、未審査の shared は配れない)。
- 有料のプラグインでも、有効なセッションに入れる人は**購入せずにそのセッションの中でだけ**使える(ホストは `?session=<id>` 付きで
  マニフェストと配布物を取る。Central の S-048)。

**ホスト → プラグイン(`call`)** — args はすべて**平ら**(`page.event` の中身も `{token, type, ...}` の一段):

- `page.open {token, surface: "page"|"astell"|"profile"|"profile.host"|"swing", context: {roomId?, profileHandle?, profileAccountId?, swingId?, data?, author?}, me: {accountId, handle, displayName}, session?: Session, size: {width, height}, now}`
  (v0.20: surface `swing` でも shared なら `session` が載る。scope は `swing:<swingId>` か、作った人が付けた `link:` / `profile:`)
  (v0.21: surface `profile.host` は持ち主だけ。`context` は `profile` と同じ `profileHandle` / `profileAccountId`、shared なら `session` は `profile:<accountId>`)
  (v0.22: **`now`** = いまの時刻(エポックミリ秒)。wasm32 では `SystemTime::now` がトラップするので([cli.md](cli.md) の落とし穴)、時刻はホストからもらう。
  ホストは Central の時計に合わせた値を渡す(welcome の `now` との差で端末の時計のずれを直す)。`page.event` と RRL のイベントにも毎回載る)
  → 戻り **`{root: Node, title?: string}`**(ページの RRL。表示は全画面のページで、**ヘッダーと戻るはホスト**)。
  `session` は shared で、セッションに入っているときだけ載る(無い = 1 人で開いた。solo と同じに動けばよい):
  ```json
  { "id": "cs_…", "scope": "room:<roomId>", "role": "owner", "ownerAccountId": "<accountId>",
    "participants": [ { "accountId": "…", "handle": "…", "displayName": "…" } ],
    "vars": { "counter": { "value": 3, "rev": 7, "by": "<accountId>" },
              "@<accountId>.paint": { "value": { "svg": "<svg …>" }, "rev": 2, "by": "<accountId>", "expiresAt": 1791936000000 } } }
  ```
  `role` = `owner`(セッションを作った人 / プロフィールの持ち主 / ルームのオーナー・モデレーター)か `member`。
  `ownerAccountId` = (v0.22)セッションの持ち主(作った人 / プロフィールの持ち主)。持ち主だけが書ける設定を `@<ownerAccountId>.…` に置けば、
  参加者はそのキーで読める(ルームのモデレーターも `role` は `owner` だが、`ownerAccountId` は持ち主 1 人)。`vars` = 今のサーバー変数
  (`rev` = そのキーの版、`by` = 最後に書いた人、`expiresAt` = (v0.22)期限のエポックミリ秒。期限の無いキーには無い)。
- `page.event {token, type, now, ...}` — ページを開いている間の通知(v0.22: どの `type` にも `now` = いまの時刻、エポックミリ秒):

  | `type` | 残りのフィールド | いつ |
  |---|---|---|
  | `var` | `key, value, rev, by, expiresAt?, reason?` | サーバー変数が変わった(**自分の書き込みも届く**。`value: null` = 消えた)。`key` は書き換え後(`me.vote` → `@<accountId>.vote`)。(v0.22)`expiresAt` = 期限(あれば)。`reason` = 書いた人が消したのでないときの消えた理由: `expired`(期限が来た)/ `evicted`(ホストの総量を超えたので期限の早い順に前倒しで消した)/ `removedByOwner`(セッションの owner が消した)。`expired` / `evicted` の `by` は `null` |
  | `call` | `fn, args, from: {accountId, handle, displayName}` | 誰かの `shared.call`(**自分のも届く**) |
  | `joined` | `participant: {accountId, handle, displayName}` | 参加者が入った |
  | `left` | `accountId` | 参加者が出た |
  | `conflict` | `key, value, rev` | `shared.set` の `ifRev` が合わなかった(書いていない。`value` / `rev` は今の値) |
  | `closed` | `reason` | セッションが閉じられた(以後 `shared.*` は `E01`)。ページは開いたまま |
  | `resized` | `size: {width, height}` | ページの大きさが変わった |
  | `paused` | — | (v0.19、surface `swing`)見る人が「全画面から縮小」を押した。ホストはページをぼかして入力を止めている。動き(音・時間・アニメーション)を止める |
  | `resumed` | — | (v0.19、surface `swing`)全画面に戻った |
  | `edited` | `svg, width, height` | (v0.22)`media.editor.open` で開いた編集画面で保存した。`svg` は許可リストで消毒した SVG の文字列(`width` / `height` はその `viewBox` の大きさ) |
  | `editCancelled` | — | (v0.22)`media.editor.open` で開いた編集画面を保存せずに閉じた |

- RRL のイベント: `on: {"tap": "<fn>"}` などは FOW と同じく `<fn>` を call する。args は **`{token, nodeId, kind, now, ...payload}`**
  (payload のフィールドは**平らに展開**。例: `RadioListTile` の `change` = `{token, nodeId, kind: "change", now, value}`。`now` は v0.22)。
- 文字の入力は **`Input`**([`rrl.md`](rrl.md) の「入力欄 `Input`」、v0.18): `p: {hint?, value?, multiline?, maxLength?, submitLabel?, clearOnSubmit?}`、
  `on: {submit}` → `{token, nodeId, kind: "submit", text}`(`change` → `{…, text}` は打つたび。要らなければ書かない)。打ちかけの文字はホストが持つので、
  `page.render` で描き直しても消えない。
- `page.close {token, reason}` — ページを離れた(戻る / 閉じる / ルームを出た)。戻り `{}`。

**プラグイン → ホスト(ホスト関数)** — すべて fire-and-forget(戻り `{}`)で、`token` 必須(古いページからの反映は捨てる):

| fn | args | 権限 |
|---|---|---|
| `page.render` | `{token, root, title?}` — ページの RRL を丸ごと差し替える | `render` |
| `page.end` | `{token}` — (v0.19)プラグインが自分で終わる。surface `swing` では、その Swing を見終えたことにして次へ進む(ほかの surface では何もしない) | `render` |
| `shared.set` | `{token, key, value, ifRev?, ttl?}` — サーバー変数を書く(`value: null` で消す)。`ttl` = 残す秒数(v0.22、下) | `shared.session` |
| `shared.op` | `{token, key, op: "incr"\|"append"\|"remove"\|"merge", value, ttl?}` — サーバー側で原子的に | `shared.session` |
| `shared.call` | `{token, fn, args}` — 参加者全員(自分も)へ `page.event {type: "call"}` を配る(サーバー関数 = 中継) | `shared.session` |
| `media.editor.open` | `{token, format: "svg", aspect?, svg?, title?}` — (v0.22)メディア編集画面を **SVG モード**で開く(下) | `render` |

- `shared.op`: `incr` = 数値に足す(無ければ 0 から)、`append` = 配列の末尾に足す(**上限 500 要素**、超えたら先頭から捨てる)、
  `remove` = 配列から等しい要素を 1 つ消す、`merge` = オブジェクトに浅くマージ。
- `shared.*` は**セッションが無ければ `E01`**(solo / 1 人で開いたとき / `closed` のあと)。反映は必ず `page.event {type: "var"}` で届くので、
  プラグインは**手元で先に値を変えず**、`var` を受けて描き直す(全員が同じ順で同じ値を見る)。
- ほかに使えるホスト関数: `fow.open` / `fow.close`(ページの子として開く)/ `storage.*`(`storage.safe`)/ `toast` / `url.open`。

**落書き(SVG の編集、v0.22、rasq-client D-465)** — `media.editor.open {token, format: "svg", aspect?: "3:4", svg?, title?}`:

- ホストのメディア編集画面を **SVG モード**で開く。**広い画面は第 3 カラム**(ページの横)、**狭い画面は全画面**。道具はホストのもの
  (ペン / 消しゴム / 色 / 太さ / 取り消し)。縦横比は `aspect`(`"<w>:<h>"`、1〜16 の整数、既定 `"3:4"`)で固定。
- `svg` を渡すと**続きから描ける**(64 KiB まで。ホストが下の許可リストで消毒してから読む。読めない部分は捨てる)。
- 保存すると `page.event {type: "edited", svg, width, height}`、保存せずに閉じると `{type: "editCancelled"}`。
  開いている間にもう一度呼ぶと、前のものを閉じて(`editCancelled`)開き直す。ページを閉じると編集画面も閉じる。
- 出力の `svg` は `<svg viewBox="0 0 W H">` に、なぞった線を簡略化した `<path>` を並べたもの(`fill="none"`、`stroke` / `stroke-width` /
  `stroke-linecap="round"` / `stroke-linejoin="round"`。消しゴムは描いた線を消す)。大きさは線の数で決まるので、サーバー変数に置くなら
  値の上限(16 KiB、`sharedLimits` で広げられる)に入るかを確かめる。
- 描くとき(RRL の `Image` の `data:image/svg+xml`、§3)も読むとき(`svg`)も、ホストは**許可リスト**で消毒する:
  要素は `svg` / `g` / `path` / `polyline` / `polygon` / `line` / `rect` / `circle` / `ellipse`、属性は形と描き方(`viewBox` `d` `points`
  `x` `y` `width` `height` `cx` `cy` `r` `rx` `ry` `x1` `y1` `x2` `y2` `fill` `stroke` `stroke-width` `stroke-linecap` `stroke-linejoin` `opacity`
  `fill-opacity` `stroke-opacity`)だけ。`script` / `style` / `foreignObject` / `image` / `use` / `href` / `on*` / 外部参照(`url(...)`)は捨てる。

**キーと上限**(サーバーが守る。外れたら書かずに `error`):

- キーは `^[A-Za-z0-9_.:@-]{1,64}$`。**`me.` で始まるキーはサーバーが `@<accountId>.` に書き換える**(例: `me.vote` → `@<自分>.vote`)。
  **`@<accountId>.` で始まるキーはその本人だけが書ける**(読むのは全員)= 1 人 1 票の投票などに使う。
  - (v0.22)**セッションの owner(`role: "owner"`)は、ほかの人の `@<accountId>.` のキーを消せる**(モデレーション)。
    `shared.set {key: "@<その人>.x", value: null}` だけで、書き換え(`null` 以外)と `shared.op` はできない。
    全員へ `var {value: null, by: <owner>, reason: "removedByOwner"}` が届く。
- 値は JSON で **16 KiB まで**、1 セッション **256 キー・合計 1 MiB まで**。`call` の `args` は 16 KiB まで。
  - (v0.22)マニフェストの **`page.sharedLimits: {valueBytes?, totalBytes?}`** で広げられる(天井: 値 **256 KiB** / セッション **8 MiB**)。
    宣言した版は**強化審査**の対象で、Central は**審査を通った版**のセッションでだけ効かせる(共有セッションはもともと審査済みの版にしか作れない)。
    **リリース版からは審査料が割り増し**になる(β では審査だけ、[REVIEW_FEE.md](REVIEW_FEE.md) §5.1)。`rasq check` は宣言に警告を出す。
- (v0.22)**TTL**: `shared.set` / `shared.op` の **`ttl`**(秒、1〜2,592,000 = 30 日)で、そのキーに期限を付ける。
  - 期限が来ると **Central が消し**、全員へ `var {value: null, by: null, reason: "expired"}` を配る(数秒の遅れはある)。
    期限を過ぎたキーは、消す前でも welcome / `GET /custom/sessions/{id}` に出さない。
  - `shared.set` で `ttl` を省くと**期限なし**(前の期限も外す)。`shared.op` で省くと**今の期限のまま**(`ttl` を渡せば付け直す)。
  - 期限は `vars` / `var` の **`expiresAt`**(エポックミリ秒)で見える。残りを出すなら `expiresAt - now`。
- (v0.22)**ホストの総量**: 無料版のホスト(β ではこれだけ)は、**1 ホスト 16 MiB**(ホスト = セッションの持ち主のアカウント。
  そのアカウントが持つ**全部の共有プラグイン・全部のセッション**の値の合計)。超える書き込みが来ると、Central はそのホストの全セッションから
  **期限の早い順に、時刻を待たずに消して**空ける(期限の無いキーは最後、その中は古い書き込みから。いま書くキーは消さない)。
  消したキーは、そのセッションの全員へ `var {value: null, by: null, reason: "evicted"}`。空けても入らなければ書かずに `error`。
- 1 接続あたり **20 通/秒まで**(超えたぶんは `error`)。

**セッション**(Central、rasq-central S-048 / [`../openapi/store.yaml`](../openapi/store.yaml) の `/custom/*`): scope は `room:<roomId>` /
`profile:<accountId>` / `link:<sessionId>` / `swing:<swingId>`(v0.20、S-079: Swing を作るときに Central が作り、その Swing を見られる人が入れる。
Swing が消える / 取り下げられると閉じる)。作るのはホスト(`POST /custom/sessions`。`swing:` は `POST /swings` の `plugin.session`)、中身の同期は WS `/custom/sessions/{id}/ws`
(`welcome` / `var` / `call` / `joined` / `left` / `conflict` / `error` / `closed`)。ホストは WS の通知を上の `page.event` に写す
(v0.22: `welcome` に Central の時刻 `now`、`var` / `vars` に `expiresAt` と `reason`)。
プラグインは WS も Central も直接は見ない。

- CLI: `rasq new page <name>`(雛形 = shared のカウンター。セッションが無ければ端末だけで数える)。`rasq check` は上のスキーマの規則
  (`page` が要る / shared なら `shared.session` / `astell` は shared だけ / `profile.host` は `profile` と一緒 / `sharedLimits` は shared だけ)と、
  solo なのに `shared.session` / `maxParticipants` がある警告、`sharedLimits` の宣言(強化審査・リリース版の審査料の割り増し)の警告を見る。
- ストアのカードの `pluginType` は、現在版のマニフェストの `slots` に `page` があれば **`custom`**(ほかより優先)。

## 3. RRL のホスト拡張(rasq-client D-167 以降。古いホストは無視/⚠ボックス)

- `Image.p.src` に **`data:image/png;base64,…`** を受ける。`p.width` / `p.height` で大きさ指定。
  (v0.22)**`data:image/svg+xml;base64,…`**(または `data:image/svg+xml,<URL エンコードした SVG>`)も受け、ホストが許可リストで消毒してから描く
  (§2.13 の「落書き」、[rrl.md](rrl.md) の「SVG」)。`p.fit`(`contain` 既定 / `cover` / `fill`)。
- (v0.22)`Grid.p.aspectRatio` = 1 マスの**幅 ÷ 高さ**(既定 1 = 正方形。3:4 の縦長なら `0.75`)。`Grid.p.columns` と一緒に使う。
- `Stack` の子で `p.left` / `p.top`(+ `width` / `height`)があるものは絶対配置(`Positioned`)。
- 新ノード **`Box`**: `p: {width, height, color, borderColor, borderWidth, radius, angle?(度、v0.6: 中心で回転して描く), label?(v0.6: 合わせる先の一覧に出す名前)}`、子を持てる。`on: {tap}` 可。
- `Text.p` に `size` / `bold` / `color`。色は `#RRGGBB` か `#RRGGBBAA`(8 桁は末尾が不透明度)。`Icon.p.color` も同じ。
- L1 ノード(`Text` / `Box` / `Image` …)の `on: {tap, longPress}` はホストが GestureDetector で拾う。
- **`on: {drag}`**(v0.5): 指でなぞる。イベントの payload は `{phase: "start"|"update"|"end", x, y, dx, dy}`(v0.6: `end` に `snap` が付くことがある、§2.2)。
  `x` / `y` はそのノードの左上を原点にした位置、`dx` / `dy` は前回からの差分。単位は親 `Stack` の単位
  (メディアエディターの重ねなら**プレビューの画素**。ホストが表示倍率を戻してから渡す)。`end` の `dx/dy` は 0。
  ハンドル(小さな `Box`)にも、画像全体を覆う透明な `Box`(ペンの受け皿)にも付けられる。

## 4. ホスト関数の追加(すべて fire-and-forget、戻り `{}`)

| fn | args | 権限 |
|---|---|---|
| `toast` | `{message}` | — |
| `sound.play` | `{kind: click\|alert}` | — |
| `clipboard.set` | `{text}` | — |
| `url.open` | `{url}`(http/https のみ) | — |
| `mute.add` | `{text}` | `mute.words`(ユーザー確認つき) |
| `composer.open` | `{token, text, mediaB64?, mimeType?, name?}`(§2.8) | — |
| `astell.share` | `{kind, payload}` | スロット `astell.action` |
| `composer.media.replace` | `{token, bytesB64, mimeType, name?}` | スロット `composer.media` |
| `composer.media.preview` | `{token, bytesB64, mimeType, width?, height?}` | スロット `composer.media` |
| `composer.media.overlay` | `{token, root}`(§2.2、v0.5) | スロット `composer.media` |
| `composer.text.replace` | `{token, start, end, text}` | スロット `composer.text` |
| `viewer.overlay.set` | `{token, root}` | スロット `viewer.overlay` |
| `page.render` | `{token, root, title?}`(§2.13、v0.18) | スロット `page` + `render` |
| `shared.set` | `{token, key, value, ifRev?, ttl?}`(§2.13。`ttl` は v0.22) | スロット `page` + `shared.session` |
| `shared.op` | `{token, key, op: incr\|append\|remove\|merge, value, ttl?}`(§2.13) | スロット `page` + `shared.session` |
| `shared.call` | `{token, fn, args}`(§2.13) | スロット `page` + `shared.session` |
| `media.editor.open` | `{token, format: "svg", aspect?, svg?, title?}`(§2.13、v0.22) | スロット `page` + `render` |

`token` はホストが `call` で渡した値をそのまま返す(古い呼び出しに対する反映を捨てるため)。

## 5. ホスト側(rasq-client)の読み込み

- 「WASMプラグイン試験 (dev)」→ `.wasm` を読み込み → **スロットに登録**。登録はパスを設定に残し、次回起動でも読み直す。
- 登録されたプラグインはワーカー isolate で常駐し、各スロットの場所(AsTell の下段ボタン / 添付タイルの … / コンポーザー / ビューワー / TL / トレンド / ページ)から `call` される。
- 権限外のホスト関数は `E01`。スロットに宣言の無い `call` は来ない。
- **FOW の着き方(v0.2)**: プラグインの `fow.open` は、呼び出し元の面の**子**として開く(コンポーザー → MFM ピッカーと同じ
  インライン FOW、AsTell ルーム FOW → その下、メディアエディター → 下付き)。親が閉じれば連動して閉じる。

### 5.1 サイドロードの 2 つのモード(全機能モード / 制限モード)

試験画面などからストアを通さずに入れたもの(サイドロード)は、署名で 2 つに分かれる。正典は [PROTOCOL §6.1](../PROTOCOL.md)。

- **全機能モード** = `.rspkg` が**自分の開発者鍵**(自分の RASQ アカウントに登録した鍵)で署名されている。`rasq build` は
  `~/.config/rasq/dev-key.toml` の鍵で署名し、`rasq key show` が指紋を出す。鍵の登録は Web コンソールの `/console/keys`。
- **制限モード** = それ以外(署名の無いもの、ほかの人の鍵、`rasq dev` の生の `.wasm`、あとから入れた `asset:`)。
  アプリに同梱しているアセット(バイト列が同梱ファイルと一致するもの)はアプリと同じ信頼でフル。
- **ストアの審査を通ったものは、いつも全機能**。チャイルドセーフで見守られている子の端末には開発者の鍵が無いので、
  そこでのサイドロードは**すべて制限モード**。

| 制限モードで | 扱い |
|---|---|
| `net.fetch` | 閉じる(`E01`。`network` を宣言していても) |
| `gpu.compute` / `storage.unsafe` | 閉じる(`E01`。`storage.safe` は使える) |
| `fow.open` | `fow.widget` だけ(`fow.interstitial` / `fow.system` は断る) |
| `composer.open` / `astell.share` / `mute.add` / `url.open`(§4) | 断る(`E01`)。`clipboard.set` / `toast` は使える |
| 燃料 / 時間の上限 | 半分 |
| `actions[].auto`(§2.11) | 読まない(ホスト既定) |
| 描画のスロット(`post.transform`、`tl`、`viewer.overlay` の描画) | 動く |

断られたホスト関数は `{ok:false, error:{code:"E01"}}` を返す(トラップしない)。プラグインは戻りの `ok` を見て、その機能を隠すか縮退させる。
