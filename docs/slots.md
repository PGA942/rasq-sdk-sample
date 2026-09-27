# RASQ スロットプラグイン契約 v0.17(正典)

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

- 形は [`../schema/manifest.schema.json`](../schema/manifest.schema.json)(v0.3)。`kind` はその enum(`sns` / `action` / `view` / `trend` / `mediaViewer` / `postProcessor`。wasm を持たない配布物の `assets`(§2.10)/ `theme`(§2.12)も)。
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
- 配布物(`.rasqpkg`): `bundle.json` の `root` は `{id, version, kind: "assets", manifest}` で **`wasm` を持たない**。`children` / `libs` /
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
- 配布物(`.rasqpkg`): `bundle.json` の `root` は `{id, version, kind: "theme", manifest}` で **`wasm` を持たない**。`children` / `libs` /
  `models` は空、`assets[]` は全部 `kind: "image"` で `sha512` / `sizeBytes` 必須(色だけのテーマなら空)。アセットパックと同じ扱い
  ([`bundle.schema.json`](../schema/bundle.schema.json))。
- CLI: `rasq new theme <name>` → `packs/<name>/`(`manifest.json`(kind = theme、上の例に近い雛形)+ `theme/` の画像 1 枚)、
  rasq.toml に `[[packs]]`。`rasq check` はアセットパックと同じ検査(スキーマ / 形式 / 大きさ / 書き忘れ)に加えて上の整合
  (asset の参照、色の形、面のキー、asset と gradient のどちらか 1 つ、stops の数と順)を見る。どこからも指されていない `use: "theme"` の画像は警告。`rasq build` はパックごとに
  配布物 1 つ(kind theme)、`rasq publish` は掲載のカテゴリを `theme` にする。
- ホスト(rasq-client): インストールすると画像を端末に保存し、設定 → アプリの外観 → フォントやカラースキーム に並ぶ。
  有効にできるのは 1 つだけ(別のものを選ぶと前のものは外れる)。`above` の層は描くだけでタッチを通す。
  ホストは配布物に wasm が 1 つでも入っていたら着せ替えとして扱わない(アセットパックと同じ)。

## 3. RRL のホスト拡張(rasq-client D-167 以降。古いホストは無視/⚠ボックス)

- `Image.p.src` に **`data:image/png;base64,…`** を受ける。`p.width` / `p.height` で大きさ指定。
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

`token` はホストが `call` で渡した値をそのまま返す(古い呼び出しに対する反映を捨てるため)。

## 5. ホスト側(rasq-client)の読み込み

- 「WASMプラグイン試験 (dev)」→ `.wasm` を読み込み → **スロットに登録**。登録はパスを設定に残し、次回起動でも読み直す。
- 登録されたプラグインはワーカー isolate で常駐し、各スロットの場所(AsTell の下段ボタン / 添付タイルの … / コンポーザー / ビューワー / TL / トレンド)から `call` される。
- 権限外のホスト関数は `E01`。スロットに宣言の無い `call` は来ない。
- **FOW の着き方(v0.2)**: プラグインの `fow.open` は、呼び出し元の面の**子**として開く(コンポーザー → MFM ピッカーと同じ
  インライン FOW、AsTell ルーム FOW → その下、メディアエディター → 下付き)。親が閉じれば連動して閉じる。

### 5.1 サイドロードの 2 つのモード(全機能モード / 制限モード)

試験画面などからストアを通さずに入れたもの(サイドロード)は、署名で 2 つに分かれる。正典は [PROTOCOL §6.1](../PROTOCOL.md)。

- **全機能モード** = `.rasqpkg` が**自分の開発者鍵**(自分の RASQ アカウントに登録した鍵)で署名されている。`rasq build` は
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
