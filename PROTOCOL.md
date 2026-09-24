# RASQ Boundary Protocol — Draft v0.1 (M3 / Host ⇄ WASM plugin)

最終更新: 2026-08-22 / 根拠: 決定台帳 D-001(実行境界)・D-002(4層)・D-012(FOW)・D-017〜D-020、Qwen壁打ち「転送プロトコル v0」
対になる文書: [`CONTRACT.md`](CONTRACT.md)(操作レベルの契約) / スキーマ: [`schema/rrl-node.schema.json`](schema/rrl-node.schema.json), [`schema/boundary.schema.json`](schema/boundary.schema.json)

## 0. 原則

1. **実行境界**(D-001): 画面コンストラクタはプラグインごとに隔離された WASM インスタンスで動き、RRL の AST を生成してホスト(Dart レンダラー)へ渡す。ホストは WASM 出力を信頼せず境界で再検証する。
2. **動かす中心は WASM**(D-018)。onnx / llama.cpp などの ML ランタイムは**補助**で、ホスト関数 `ml.infer` 経由で呼ぶ(審査済み+`gpu_ml` 権限)。SmallLM に常時処理をさせる構成は想定外。iOS はインタープリタ実行が基準なので、描画ホットパスは WASM に載せない。
3. **ワイヤ形式は JSON v1**(D-020)。内部表現は JSON でも RRL そのままでも可。性能が必要な経路(Snapshot)は後で FlatBuffers/ZeroCopy を**同じ論理モデルのまま**追加する。メッセージには `schema: "rasq.boundary/1"` を付け、版交渉する。
4. **状態は DSL の外**。RRL ノードはステートレス。変更はイベント → function.json の関数 → ホスト Store → Props(Bind)の一方向。
5. **ID は安定**。path hash は使わない。`{instanceScope}:{explicitKey}`(§2)。

## 1. インスタンスとスコープ

```
instanceScope = "{pluginId}:{accountKey}:{pluginVersion}:{instanceId}"
accountKey    = "{kind}:{instance}:{handle}"
nodeId        = "{instanceScope}:{key}"      // key はプラグインがツリー内で付ける。ForEach はアイテムIDを key にする
```
アカウント分離(アカウントごとのプラグイン版)と Store / キャッシュ / 関数解決のスコープを同じキー体系で解決する。

## 2. RRL ノード(JSON)

```jsonc
{ "t": "Text", "k": "body", "tag": "main_text", "p": { "source": "$.bodyRSL" }, "on": { "tap": "openThread" }, "c": [] }
```
| フィールド | 意味 |
|---|---|
| `t` | ノード型。L1 Primitive / L2 Standard Component / L3 Control(§2.1) |
| `k` | 明示 key(兄弟内で一意)。省略時はホストが `t#index` を補うが **差分更新の対象にはならない** |
| `tag` | **意味タグ**(§2.2)。投稿内容に関係しないノード(Icon, Spacer, ActionBar 等)には付けない |
| `p` | Props。`$.path` は Bind(ホスト Store/投稿オブジェクトへの参照) |
| `on` | イベント → function.json の関数名(`tap/longPress/pointerDown/pointerMove/pointerUp/confirm/mount`) |
| `c` | 子ノード |
| `role` | 検証ゲート用の構造ロール(`threadNav` / `contentBody` / `mediaItem` / `actionBar`)— 完全置き換えタイルの必須ロール保証に使う |

### 2.1 ノード型(D-002)
- **L1 Primitive**: `Text` `Icon` `Image` `Row` `Column` `Stack` `Grid` `Scroll` `Canvas` `Spacer`
- **L2 Standard Component**(ホスト実装・Props のみ): atomic `Input` `Avatar` `Badge` `RichText` `MediaGrid` `ActionButton` `Timestamp` `Button` `ListTile` `SwitchListTile` `RadioListTile` `ValueListTile` `WarningListTile` `DestructiveListTile` `SectionHeader` / composite `PostTile` `ActionBar` `TrendHeader` `UserRow` `UserTileLarge` `PollWidget` `NotificationTile` `PluginTile` `VCCard` `CollapsedPost` `HeaderedListView` `SortableList` `SegmentedControl` `Tabs`
- **L3 Control**: `If` `ForEach` `Call` `Bind` `Slot` `Stream`
- **L4 Host-owned Surface** はノードとして現れない(ホスト関数 `fow.open` 等で要求)。`AdSlot` はレンダラー挿入地点(プラグインからは見えない)。

### 2.2 意味タグ(D-019)
RAi の位置検索、タイムラインソート、リレーショナルTL(本文/画像抽出)が参照する。**投稿内容を構成するノードにのみ付ける**。

| tag | 付ける先 |
|---|---|
| `main_text` | 本文(RSL) |
| `cw` | 注釈(CW)テキスト |
| `pic` / `video` / `gif` / `file` | メディアタイル(`MediaGrid` の各タイル、または単体 `Image`/`Video`) |
| `vote` / `vote_option` | 投票本体 / 選択肢 |
| `quote` | 引用(埋め込み投稿) |
| `link` / `hashtag` / `mention` / `emoji` | 本文中のインライン要素(RSL スパンにも同じ名前で付く) |
| `reaction` | リアクション表示(絵文字+数) |
| `user_name` / `user_handle` | 投稿者名 / ハンドル |
| `time` | 投稿時刻 |

付けないもの: `Icon`(装飾)、`Avatar`、`ActionBar`/`ActionButton`、`Spacer`、レイアウト(`Row/Column/...`)。

### 2.3 RSL スパン(本文のワイヤ)

`RichText` の `p.source` は **生文字列**(ホストが plain/MFM/HTML を解析)または **スパン配列**(プラグインが解析済み)のどちらでも良い。スパンの JSON は [`schema/rsl-span.schema.json`](schema/rsl-span.schema.json)、例は [`examples/mfm-body.rsl.json`](examples/mfm-body.rsl.json)。

**設定ページ (D-031/D-044)**: 設定第二階層以降は wasm 不要の**静的 RRL ドキュメント**([`schema/settings-page.schema.json`](schema/settings-page.schema.json)、`rasq.settings-page/1`)。値は Bind `$.settings.<key>`、タイルの `change` イベント(`on: {"change": "settings.set"}`、設定キー = ノードの `k`)をホストが書き込みへ変換する。SNS プラグインは同形式のページを `settingsPages` として提供する (D-044):

- `area` は短名(`account` / `timeline` …)。ホストが `sns.<pluginId>.` を前置して名前空間を強制し、ルート `/settings/dsl/sns.<area>`・KV `sns.<pluginId>.<area>.v1`(`storage.safe` 相当)に割り当てる。
- ローカル保存ページの値は、変更毎に `SnsSession.applySettings(area, values)`(defaults マージ済み)でプラグインへ即時反映される(例: Misskey の `timeline.withRenotes` → TL 取得パラメーター)。
- `source: "remote"` のページは KV を使わず、ホストが `SnsSession.settingsRead(area)` / `settingsWrite(area, key, value)`(戻り値 = 最新値マップ)でサーバーに読み書きする(例: Misskey の `account` → `i` / `i/update`)。
- `on: {"tap": "web.open"}` は `k` をパスとして接続先インスタンスの web 設定を開く(パスワードが要る操作の逃がし先)。

- 種別: `text`(bold/italic/strike/small/code)/ `emoji` / `image` / `link` / `mention` / `hashtag` / `transform`
- **`transform` = MFM 装飾関数**。`{s:"transform", fn, args, c:[...]}` で入れ子可。ホストが必ず描画するコアセット:
  `x2 x3 x4 small blur rotate flip fg bg center shake spin jump bounce rainbow twitch jelly tada plain`
  未知の `fn` は **中身をそのまま描画**(剥がし)。派手系(アニメーション)の描画はホストのモーション設定で無効化できる。
- 検証ゲート: sideload content のスパンは非インタラクティブ化(旧D2)、`link`/`mention` の先はホストが開く。

## 3. メッセージ

すべて `{ "schema": "rasq.boundary/1", "scope": instanceScope, ... }` を共通ヘッダに持つ。

### 3.1 プラグイン → ホスト
| type | 内容 | 備考 |
|---|---|---|
| `snapshot` | `{ rev, root: Node }` | 初期ツリー全体。読み取り専用。ZeroCopy 対象(将来) |
| `patch` | `{ rev, baseRev, surface?, ops: [ {op: insert|remove|setProp|replaceSubtree|move, target: nodeId, index?, node?, prop?, value?} ] }` | **原子バッチ**。`baseRev` がホストの現行 rev と不一致 → 拒否 → `resync` を返す。**`surface` = FOW surface の id**(D-073: FOW ドキュメントへの適用。rev 系列は `fow.open` 時に 1 から。不一致/未知 surface は破棄 — 再 `fow.open` で resync)。v1 では `result.value.patch: true` を返すとホストが `patch()` を引き取る。サイズ/頻度上限超過は異常として記録(B系) |
| `hostFn` | `{ callId, fn, args }` | §4 ホスト関数。戻りは `hostFnResult{callId, ok, value|error}` |
| `result` | `{ callId, ok, value|error }` | ホストからの `call` への応答 |
| `emit` | `{ name, payload }` | 同一インスタンス内の ephemeral UI 状態(開閉・フォーカス)。ビジネス状態は Store へ |

### 3.2 ホスト → プラグイン
| type | 内容 | 備考 |
|---|---|---|
| `event` | `{ nodeId, kind: tap|longPress|pointerDown|pointerMove|pointerUp|confirm, payload }` | `Canvas` は raw pointer を必ず転送 |
| `lifecycle` | `{ kind: mount|unmount|visible|hidden|cancel, streamId? }` | `mount` は論理マウントで一度だけ。可視性は `visible/hidden` で分離(仮想化TL)。**v1 wasm ABI では `call("lifecycle", {kind, surface?})` として届く**(FOW surface の mount/unmount 実装済み、D-072。未対応プラグインは C01 を返してよく、ホストは無視する) |
| `call` | `{ callId, fn, args }` | `CONTRACT.md` §5 の各操作(function.json で宣言した関数) |
| `resync` | `{ reason }` | スナップショットの再送要求 |

### 3.3 ライフサイクル / 障害隔離
- WASM がトラップしても**レンダラーは生存**。最後に成功した snapshot を描画し続け、当該プラグインのみ縮退表示(エラーコード B系で記録)。スロット単位の障害隔離(P3)。
- ホストは `closeOwnedBy(pluginId)` で当該プラグインのオーバーレイ(FOW 含む)を一括クローズ。

## 4. ホスト関数(`hostFn`)と必要権限

| fn | args | 権限 |
|---|---|---|
| `fow.open` | `{ id, spec: FowSpec(type, confirmMode, initialHeightFactor, title, cancelText, confirmText), root: Node, onClosed?, onCancel? }` — `onClosed` は閉時にホストが call する関数名(args `{id, reason, result?}`)。`onCancel` は **✕/キャンセル押下時**に call する関数名(args `{id}`): 指定時ホストは閉じず、プラグインが画面そのまま確認ポップアップ(確認 FOW 等)を出して自分で `fow.close` する。`spec.attachTop` = アプリヘッダーに下付き。`anchor {node?|role?}` = **自ドキュメントの実物ノードへアンカー**(例: 自分のアクションバー。ホストが role 付きノードのジオメトリ登録簿から現在矩形を解決。基本下付き・余白不足で上付き、別カラムへ動かすと壁付き表示)。`parentId` は**親 FOW の id**(同プラグイン内): 子 FOW は親の付く方向・カラムに追従(方向は親側指定 — 親が動けば更新)し、親は子の高さと方向をリアルタイムに受けて**入力部が被らないよう伸縮**する。親のクローズ/スイッチャーの親選択で子は連動クローズ。戻り `{opened:true}` は**受理**(表示は UI isolate 側で非同期)。同一 id の再要求 = root の差し替え | `fow.widget` / `fow.interstitial` / `fow.system` |
| `fow.close` | `{ id, result?, reason? }` | 同上 |
| `alert` | `{ title, message, confirmText, cancelText?, destructive? }` | — |
| `storage.get/set/delete` | `{ ns: safe|unsafe, key, value? }`(`pluginId` で自動プレフィックス) | `storage.safe` / `storage.unsafe` |
| `net.fetch` | `{ method, url, headers, body }`(ホスト代理・dedupe・キャッシュ。繋げるのはマニフェスト `network` の宣言先だけ、リダイレクトは追わない) | `network` + `network.hosts` / `network.instance`(審査済み) |
| `ml.infer` | `{ backend: onnx|llama, model, input }` | `gpu_ml`(審査済み) |
| `gpu.compute` | `{ wgsl, entry?, input(base64 → binding0 read-only storage), outputSize(binding1 read_write storage), workgroups? }` → `{ output(base64), placement: "device" }` — **WGSL compute 専用の狭い GPU 口** (D-081)。カスタム TL の重いソート・メディア編集フィルター向け。ネイティブは wgpu(Metal/Vulkan)でランタイム内短絡、Web ビルドは WebGPU にそのまま写る想定。アダプター無し/検証エラーは E01 — その時プラグインは**クラウドカルクレーター(`ml.infer`)へバイパス**する。権限の門はランタイム内(埋込側が `rasq_wasm_set_gpu_compute` で開けたインスタンスだけ。既定は E01)、引数の上限(入力 64 MiB / outputSize は 4 の倍数で 64 MiB まで / WGSL 256 KiB / workgroups は端末の上限かつ積 2^20 まで → D01)と 10 秒の壁時計は [docs/host-functions.md](docs/host-functions.md#gpucompute-の権限と上限) | `gpu_ml`(審査済み) |
| `wasm.call` | `{ pluginId, fn, args }`(子プラグイン呼び出し) | 構成で許可された子のみ |
| `store.read` | `{ kind: actionState|followState|readState, key }` | — |
| `geometry.lookup` | `{ tag? , role?, nodeId? }` → 画面上の矩形(RAi 位置検索・アクセシビリティ共用) | `fow.system` |

各 fn の args / value の形は [`schema/host-fn.schema.json`](schema/host-fn.schema.json) を正とする。

### 4.1 wasm ABI(v1・同期)

ホスト関数は wasm の **import** として渡す。v1 は同期(呼び出し中プラグインはブロック。非同期化は callId ベースの poll/notify を将来の v2 で):

```
(import "rasq" "host_fn" (func (param i32 i32) (result i64)))
  ;; param  = リクエスト JSON (`rasq.hostfn/1`: {schema, callId, fn, args}) の ptr/len
  ;; result = レスポンス JSON ({callId, ok, value|error}) を書いた wasm メモリの (ptr<<32)|len
  ;;          ホストはプラグインの `alloc` でメモリを確保して書き込む。プラグイン側が `dealloc` で返す
```

権限の無い fn・未知の fn は `{ok:false, error:{code:"E01"|"C01"}}` を返す(トラップさせない)。

### 4.2 `ml.infer` の配置解決(D-018)

- モデルは [`schema/model-manifest.schema.json`](schema/model-manifest.schema.json) のマニフェストで宣言し、`ml.infer` は **マニフェスト id** で参照する。プラグインはエンドポイントもファイルも見えない。
- マニフェストの `runtimes` に載せられるもの: **デバイス側 = `onnx`(onnx runtime)/ `llama`(llama.cpp)**、**サーバー側 = `server`(Triton Inference Server / vLLM)**。同じ論理 IO(`io.input`/`io.output`)を全ランタイムで満たすこと。
  ほかに **`embedded`** = プラグインの wasm の中で読むモデル(`include_bytes!` などで同梱し、`ml.infer` を使わずプラグイン自身が推論する。ホストはランタイムを用意しない。`format` / `library` は人が読むためのもので、`file` は審査用に本体を同梱するときのバンドル内パス)。plugin-sample の media-process(rustface の SeetaFace カスケード)がこれ。
- **配置はホストが解決**(端末 RAM・電源・従量回線・ポリシー・審査状態)。`placementHint` はヒント。結果の `value.placement` / `value.runtime` で実際にどこで動いたかが返る。
- onnx 実行時のテンソル詰め替え・トークナイズはマニフェストの `inputs[].semantic` / `tokenizer` に従ってホストが行う。
- **seq2seq 生成(onnx)**: KV キャッシュ無しの単発 forward で export されたモデルは、`runtimes.onnx.generation`(`decoderStartTokenId` / `eosTokenId` / `padTokenId` / `maxNewTokens` / `strategy: greedy`)に従って**ホストがデコーダーループ**(semantic `decoder_token_ids` のテンソルを伸ばしながら再実行)を回す。サンプリング付き生成はサーバー側(vLLM)に任せる。
- 例: [`examples/reaction-predictor.model.json`](examples/reaction-predictor.model.json)(onnx int8 + Triton)/ [`examples/astell-summarizer.model.json`](examples/astell-summarizer.model.json)(onnx fp32 単発 forward + generation / gguf + vLLM)。

## 4.3 SNS 抽象関数(イベントハンドラーの boundary 写像、M5d / D-074)

SNS プラグインのイベントハンドラー関数は「一定程度は抽象的な関数で共通化」
(デバイス側の基本思想)— **既存 SnsSession 契約の boundary 写像**として、
ホスト → プラグインの `call("sns.<fn>", args)` で呼ぶ。データは RRL では
なく **Post ワイヤ(`rasq.post/1`)** で返す:

- Post ワイヤ: `{id, author: {id, kind, instance, handle, displayName, avatarUrl?, bio?}, createdAt(ISO), body, bodyFormat, cw?, media?: [{id, type: image|video|gif|file, url, thumbnailUrl?, sensitive?, …}], poll?: {options: [{text, votes, imageUrl?}], multiple, expiresAt?, voted: [投票した選択肢の番号…](真偽値ではない。未投票は `[]`)}, quote?: Post, replyTo?: Post, replyToId?, repostedBy?, repostTargetId?, counts?, visibility?, localOnly?, directToIds?, reactions?, reactionEmojis?, myReaction?, url?}`。閲覧アカウントのキー付けはワイヤに含めない(取り込み時にホストが与える)。
  - **純リノートの展開 (D-100/D-112)**: text 空・添付なし・投票なしのリノートは中身のノートに展開して返す(`repostedBy` = 外側の投稿者、`id`/`createdAt` は外側)。`repostTargetId` に**中身のノート id** を載せること — ホストはアクション/スレッド/リアクション一覧等をこの id へ読み替える(公式クライアントの appearNote と同じ)。
  - **reactionEmojis (D-112)**: キーはリアクション文字列そのもの(`:name@host:` / `:name@.:`)。ローカルカスタム(`:name@.:`)はサーバーの reactionEmojis に URL が来ないため、`https://<host>/emoji/<name>.webp` を補完して載せる。
- `call("sns.emojis")` (D-111) → `{emojis: [{name, url, category?, aliases?}]}`: カスタム絵文字一覧。TL 内の `:shortcode:` 描画と絵文字ピッカーにホストが使う。
- v1 実装済み(ホスト側アダプター = `WasmSnsSession`。エラーは result.error のコードのまま `SnsApiException` へ写る):
  - `sns.timeline {kind, limit, cursor?}` → `{page: {items: [Post], nextCursor?}}`
  - `sns.thread {id}` → `{thread: {post, ancestors: [Post](古い順), descendants: [Post]}}` (D-075)
  - `sns.performAction {actionId, postId, args}` → `{result: {done, count?, message?}}` — 短押しの中身はプラグインが決める(CONTRACT §5)。未対応アクションは C01 (D-075)
  - `sns.actions {}` → `{actions: [ActionDescriptor JSON(register_action と同形)]}` — アクションバー宣言 (D-075)
  - `sns.createPost {draft: NewPost ワイヤ}` → `{post}` — draft = `{text, cw?, replyToId?, quoteOfId?, repostOfId?, visibility?, localOnly?, reactionAcceptance?, directToIds?, mediaIds?, poll?: {choices, multiple?, expiresAfterSec?}}` (D-076)
  - `sns.deletePost {id}` → `{}` (D-076)
  - `sns.me {}` → `{user}` / `sns.user {id | handle+userHost?}` → `{profile: {user, bannerUrl?, description?, followersCount?, followingCount?, postsCount?, fields?, createdAt?, location?, isLocked?, isBot?, isSuspended?, pinnedPosts?, profileUrl?}}`(`host` は接続先と衝突するため、ユーザーの所属ホストは `userHost`)(D-076)
  - `sns.userPosts {id, filter: posts|withReplies|replies|media|quotes, limit, cursor?}` → `{page}` — 返信/引用の振り分け規約 (D-069) はプラグイン側で適用 (D-076)
  - `sns.reactions {postId, type?, limit, cursor?}` → `{page: {items: [{id, user, reaction, createdAt, emojiUrl?}], nextCursor?}}` (D-076)
  - `sns.reposters {postId, …}` → `{page: {items: [user]}}` / `sns.quotes {postId, …}` → `{page: {items: [Post]}}` (D-076)
- v1 追加分 (D-134) — 通知 / 関係 / DM / 検索。`SnsSession` 契約 (CONTRACT §5) の
  残りを同じ規則(引数は素の JSON、戻りはワイヤ)で写したもの:
  - `sns.inspectServer {host}` → `{server: {host, name, software?, version?,
    description?, iconUrl?, registrationOpen?, maintainer?}}` — サーバー検索 /
    PDS 変更 (CONTRACT §2) が出す情報。**認証前に呼ばれる**ので `token` は無い。
    取り方は SNS ごとに違う(Misskey は `meta`、Mastodon は `/api/v2/instance`)ので
    ホストは知らなくてよい。未実装/失敗ならホストはホスト名だけで縮退する。
  - `sns.capabilities {}` → `{maxPostLength?}` — セッション単位の能力値。
    **投稿の文字数上限はインスタンス設定で変わる**ので、ホストが決め打ちせず
    プラグインから受け取る(Misskey は既定 3000、Mastodon は 500)。
    未実装 = C01 でホスト既定に据え置き。将来の能力フラグもここへ足す。
  - `sns.timelines {}` → `{timelines: [{id, label?}]}` — **タブに並べる基本 TL の宣言**
    (CONTRACT §6 の id 体系)。SNS ごとに揃う TL が違うため、ホストは決め打ちせず
    ここから受け取る。未実装(C01)はホスト既定へ縮退。
  - `sns.notifications {limit, cursor?}` → `{page: {items: [Notification], nextCursor?}}`
    - Notification ワイヤ: `{id, kind, createdAt(ISO), read?, actors: [user],
      post?: Post, reaction?, reactionsByActor?: {userId: reaction}, title?, body?}`。
    - `kind`: `reaction` `repost` `quote` `reply` `mention` `follow` `followRequest`
      `followAccepted` `listAdded` `pollEnded` `timer` `vc` `system` `other`。
      **未知の値はホストが `other` に倒す**ので、新しい種別を返してよい。
    - **グループ通知は畳んで返す**: `actors` に全員、`reactionsByActor` に
      actor id → リアクション文字列。単体通知は `actors` が 1 件。
    - `system` (D-045) はサーバー/アプリ発の本文つき通知で、`title`/`body` を使う。
  - `sns.markNotificationsRead {}` → `{}`
  - `sns.relationships {ids: [id]}` → `{states: {id: FollowState}}`
    - FollowState ワイヤ: `{following, followedBy, requested, muted, blocked, blockedBy}`
      (すべて bool、省略は false)。
  - `sns.setRelation {id, relation: following|muted|blocked, on}` → `{state: FollowState}`
    — 3 つのセッターを 1 本に束ねる。**戻りは書き込み後にサーバーを読み直した状態**に
    すること: 鍵アカウントへのフォローは `following` ではなく `requested` で返る。
  - `sns.follows {id, direction: followers|following, limit, cursor?}` → `{page: {items: [user]}}`
  - `sns.userList {kind: muted|blocked|followRequests, limit, cursor?}` → `{page: {items: [user]}}`
    — 「人の一覧」は形が同じなので 1 本に束ねる。未知の kind は C01。
  - `sns.resolveFollowRequest {id, accept}` → `{}`
  - `sns.reportUser {id, comment}` → `{}`
  - `sns.search {kind: posts|users, q, limit, cursor?}` → `{page}`(posts は Post、users は user)
  - `sns.trendingTags {}` → `{tags: [{tag, count?}]}`
  - `sns.favorites {limit, cursor?}` → `{page}` — ブックマーク(お気に入り)。
  - `sns.dmRooms {}` → `{rooms: [{id, kind: direct|chat, peer: user, lastPost?: Post,
    unreadCount?, title?}]}` — CONTRACT §7 のとおり、`visibility == specified|direct` の
    投稿を**相手ユーザー単位**に束ねて `direct` ルームにし、ネイティブ Chat があれば
    `chat` ルームを足す(404/403 のインスタンスでは足さない)。
  - `sns.sendDm {roomKind: direct|chat, peerId, draft: NewPost}` → `{post}`
- v1 追加分 (D-136) — メディア / プロフィール / リスト / クリップ:
  - `sns.uploadMedia {bytesB64, filename, mimeType?, sensitive?, alt?}` → `{media}` —
    **バイト列は base64 で境界を渡り、multipart の組み立てはプラグインの仕事**。
    SNS ごとにフィールド名も認証の載せ方も違う(Misskey は `i` フィールド、
    Mastodon は `Authorization` ヘッダー)ため、境界に multipart 専用の口を
    作るより面が増えない。組み立てた本体は `net.fetch` の `body: {b64}` で送る。
  - `sns.updateProfile {update}` → `{profile}`
  - `sns.users {ids}` → `{profiles: [Profile]}` — ユーザー一括取得。
  - `sns.reactedPosts {limit, cursor?}` → `{page}` (D-103) — 自分がリアクション
    した投稿。Mastodon ではお気に入り、`sns.favorites`(ホストのブックマーク欄)
    はブックマークで、**別物として分ける**。
  - `sns.savedFeeds {}` → `{feeds: [...]}`(同じワイヤ)— **保存済み / ピン留めのカスタムフィード**
    (D-195: リストページの「カスタムフィード」欄。Bluesky は getPreferences の savedFeedsPrefV2)。
  - `sns.saveFeed {uri, on}` → `{feeds: [...]}`(保存後、同じワイヤ)— **カスタムフィードを保存 / 保存から外す**
    (D-227: フィード行・フィード TL の「フィードを保存」。Bluesky は savedFeedsPrefV2 を `putPreferences` で書き戻す)。
  - `sns.feeds {query?, limit?}` → `{feeds: [{uri, name, description?, likeCount?, creator?}]}`
    (query が `at://…/app.bsky.feed.generator/<rkey>` か `https://bsky.app/profile/<actor>/feed/<rkey>` ならそのフィード 1 件、D-196)
    — **カスタムフィードの検索** (D-194: Bluesky のフィードジェネレーター。TL カスタマイズの
    「カスタムフィード」が使う)。取得は `sns.timeline {kind: "feed:<uri>"}`。持たない SNS は C01。
  - `sns.lists {}` / `sns.listInfo {id}` / `sns.listMembers {id}` /
    `sns.createList {name}` / `sns.updateListMember {listId, userId, add}`
    — リストワイヤ: `{id, name, memberIds?, description?, isPublic?}`。
  - `sns.clips {userId?}` / `sns.clipInfo {clipId}` / `sns.clipNotes {clipId, …}` /
    `sns.createClip {name, description?, isPublic?}` / `sns.deleteClip {clipId}` /
    `sns.updateClipNote {clipId, postId, add}` (D-032) — クリップワイヤ:
    `{id, name, description?, isPublic?, notesCount?}`。
  - `sns.userPlays {userId}` → `{plays: [{id, title, summary?, url}]}` (D-039) —
    RASQ 内では実行せず**外部リンク**として出す。
  - クリップと Play は SNS 固有機能。持たないプラグイン(Mastodon)は
    実装しない = C01 が返り、ホストが該当タブ/メニューを隠す。
- **設定ページ (D-044) の写像** (D-136):
  - ローカル保存ページの値は、**すべての `sns.*` 呼び出しの `args.settings` に
    area ごと入れ子で載る**(`{"timeline": {"withRenotes": false}, …}`)。
    プラグインは自分が知っている area / キーだけ読む。area で入れ子にして
    あるので、別ページの同名キーとぶつからない。
  - `sns.applySettings {area, values}` → `{}` — 値の変更をプラグインに通知する
    (保存したいプラグイン向け)。未実装 = C01 でよく、ホストは無視する。
    次回以降の呼び出しに `args.settings` として載るので実害はない。
  - `sns.settingsRead {area}` → `{values}` / `sns.settingsWrite {area, key, value}`
    → `{values}` — `source: "remote"` のページ。**戻りは書き込み後に読み直した
    最新値**にすること(サーバーが弾いた変更も画面に出るように)。
    知らない area は C01。
- 未実装関数は C01 — ホストは該当 UI を隠す(SnsSession の UnsupportedError と同じ扱い)。
  認証トークンは v1 ではホストが `args.token` で毎回渡す(サンプルは `i` に積む)。

## 4.4 タイムラインレンダラー契約 v1(M5e / D-080)

TL プラグイン = 「Widget コンポーネント + ソートアルゴリズム + 投稿カスタマイザ」
(デバイス側の基本思想)。v1 は**宣言とソートアルゴリズム**を提供する。
**データ取得はホスト(SNS セッション)の仕事** — プラグインは並べ替えと除外だけ:

- `tl.timelines {}` → `{timelines: [{id, label, base}]}` — カスタム TL の宣言。`base` はホストが餌にする基礎 TL(`home`/`local` …)。ホストはホーム TL のタブに `custom:<id>` として出す。
- `tl.sort {id, posts: [Post ワイヤ]}` → `{order: [postId…]}` — **表示順の id 列**。載っていない投稿は除外。`order` 無し = そのまま。未知の id は C01。

- `tl.customize {id, posts: [Post ワイヤ]}` → `{tiles: {postId: RRL Node}, badges: {postId: {icon?, text}}}` — **投稿カスタマイザ** (D-082/D-085)。`tiles` = 完全置き換えタイル(replace_tile)。`badges` = **上付きバナー行**(RT の「♻︎ 〇〇がリノートしました」と同スタイル): 通常タイルのままアイコン名 + テキストだけ足す軽量な口(アイコン名はホストのアイコンレジストリで解決)。完全置き換えタイル側は L2 `PostTile` の Props `bannerIcon`/`bannerText` で同じ行を出せる。**V5 検証**: タイルは必須ロール `contentBody` を含むこと — 欠けるとホストが破棄して通常タイルに縮退。**スレッドナビはホスト保証**(置き換えタイルでもタップでスレッド展開)。タイル内のイベントはプラグイン関数へ戻る。
- **可視性 lifecycle** (D-082): 仮想化 TL で行が生成/破棄されると `call("lifecycle", {kind: visible|hidden, surface: "tl:<id>", post})` が届く(置き換えタイル行)。

- `post.transform {posts: [Post ワイヤ]}` → `{posts: {postId: {body?, cw?}}}` — **投稿加工プラグイン(transform_content)** (D-083)。**全 TL 共通**で表示前に本文/注釈を差し替える(タイル構造は変えない — 構造は `tl.customize` の領分)。未実装/失敗は無加工。**ノード定義を持たないプラグインでもストア審査は通り**、その場合ホストは基礎 TL に加えて**ノードグラフの取得ノードが投稿を出す直前**にもこの加工を適用する(D-118 — リスト/検索/DM/リアクション済み等、グラフ経由の取得すべて)。ノード定義(入出力宣言)を持つプラグインはユーザーがノードとして配置した位置で実行される(ストア統合 M4 で拡張)。

v1 の口は SNS プラグイン自身のモジュール(実登録セッション)に付く — 独立 TL
/加工プラグインの常設インストールはストア統合後。

## 4.5 関連TL エンジン(特別枠、M5f / D-084)

リレーショナルTL は **TL プラグインではない特別枠**: ストア配布物ではなく
RASQ 同梱の公式 wasm モジュール(`relational-wasm`)で、アカウントバンドルに
含まれる**全アカウントの TL 系**を見る(v1 は現在アカウントの HTL + ゲスト)。
SNS プラグインが RRL の icon/body に**タグ付け必須**なのはこのため — ホストが
タグ規約(§2.2)経由で本文/引用/リンクを抽出し、SNS 非依存の最小ワイヤ
(`rasq.relational-post/1`)にしてエンジンへ渡す:

- 入力 post: `{key, authorKey, repostedByKey?, createdAtMs, body, quoteBody?, linkUrl?, quoteKey?}`
- `call("relational.build", {posts, guests, settings?, communities?, nowMs, lookbackMinutes?})` → `{events: [{postKeys(時系列順), lastActivityMs}]}`

中身は rasq-client の会話イベント方式 (D-027) ルールベース部の忠実な移植
(パリティテストで Dart エンジンと束ね一致を検証)。ホストは**バンドルの
全ログインアカウントの HTL** を混ぜて供給し (D-086)、**V11 検証ゲート**
(§2.2 の付与先違反タグは取り込み時に剥がす)でタグの読取りを守る。SigLIP / Louvain は
ホスト側資産のため、ML が効くときはホストが Dart エンジンを使う —
カルクレーター統合後に wasm 側から `ml.infer` を呼ぶ形へ置換する。

## 5. アクションと投稿ごとのアイコン(D-017)

- 短押し `performAction(actionId, postId)`(引数なし)は**プラグインが中身を決める**(Misskey 本番: 予測モデルで最有力の絵文字をリアクション)。長押しは絵文字ピッカー FOW → `args.reaction`。
- `actionStates()` / `performAction()` の `ActionResult.icon` で**投稿ごとにアイコンを差し替え**できる(操作前に予測絵文字のエッジ抽出アイコンを表示する等)。
- 差し替えできるのは **SNS プラグインとその配下プラグインの、`iconPerPost: true` を宣言したアクションのみ**。他プラグインのアイコンは視認性のため固定(ホスト検証ゲートが無視する)。バンドル用の既定アイコン(`icon`/`doneIcon`)は必須。

## 6. 検証ゲート(ホスト側、realize / スロット解決時)

| # | チェック | 違反時 |
|---|---|---|
| V1 | スロット×権限ティア(sideload は content のみ、media は tag のみ、avatar/action/full は審査済み) | アタッチ拒否(E系) |
| V2 | sideload content の GPU/ネットワーク使用 | アタッチ拒否(E系) |
| V3 | 安全フラグの単調性(センシティブ/CW の解除試行) | 無視 |
| V4 | ジオメトリトークン(spacing/margin/最小タップ域44dp/メディア余白) | ホスト値で上書き |
| V5 | 完全置き換えの必須 role(`threadNav` 等) | 標準 PostTile へフォールバック(B系) |
| V6 | アクションバー: 1投稿1本・幅指定1プラグイン1つ・容量超過 | 超過分をサイドメニューへ |
| V7 | `iconPerPost` 未宣言 / 非配下プラグインからのアイコン差し替え | 無視(既定アイコン) |
| V8 | 自動化時の confirmMode ゲート + `autoHookForbidden` | humanGate で停止 / 候補除外 |
| V9 | viewer 面の変換が全画面遷移時のみか | 無効化(E系) |
| V10 | `patch` の rev 不一致 / サイズ・頻度上限 | 拒否 + `resync` / 異常記録 |
| V11 | `tag` の付与先(投稿内容外ノードへの tag) | 無視 |
| V12 | スロット単位の障害隔離 | 当該スロットのみ標準部品化(B系) |

### 6.1 サイドロードの 2 つのモード(全機能モード / 制限モード)

ストアを通らずに入れたプラグイン(サイドロード)を、ホストは次の 2 つのどちらかで動かす。**ストアの審査を通ったものは、いつも全機能**
(この節の対象外)。V1 / V2 の「sideload」は、下の制限モードのサイドロードのこと。

- **全機能モード**: `.rasqpkg` が**利用者自身の開発者鍵**で署名されているとき。自分の鍵 = 自分の RASQ アカウントに登録した鍵
  (`rasq key init` で `~/.config/rasq/dev-key.toml` に作り、`rasq build` がこの鍵で `.sig.json` を付ける。`rasq key show` が指紋を出す。
  鍵は Web コンソールの `/console/keys` で登録する)。宣言した権限どおりに、ホスト関数・スロットを試せる。
- **制限モード**: それ以外のサイドロード全部 — 署名の無い `.rasqpkg`、ほかの人の鍵(自分のアカウントに登録していない鍵)で署名したもの、
  `rasq dev` の生の `.wasm`(パスからの読み込み)、あとから入れた `asset:` の名前のもの。
  **アプリに同梱しているアセット**(misskey-wasm-sample など、rasq-client 自身の assets/ にある wasm)はアプリと同じ信頼なので
  **フル**(ホストは同梱ファイルとバイト列が一致するときだけ同梱とみなす。名前だけ `asset:` の別物は制限)。

制限モードでの扱い(宣言した権限より、こちらが優先する):

| 対象 | 制限モード |
|---|---|
| `net.fetch` | 閉じる(`E01`)。マニフェストの `network` の宣言があっても同じ |
| `gpu.compute` | 閉じる(`E01`) |
| `storage.*` | `storage.unsafe` は閉じる(`E01`)。`storage.safe` は使える |
| `fow.open` | `fow.widget` だけ。`fow.interstitial` / `fow.system` は断る(`E01`) |
| `composer.open` / `astell.share` / `mute.add` / `url.open` | 断る(`E01`) |
| `clipboard.set` / `toast` | 使える |
| 燃料(命令数)/ 時間の上限 | 半分 |
| `manifest.actions[].auto` | 読まない(ホスト既定。[docs/slots.md §2.11](docs/slots.md)) |
| 描画のスロット(`post.transform`、`tl`、`viewer.overlay` の描画) | 動く |

- 断られた fn は、ほかの権限の無い fn と同じく `{ok:false, error:{code:"E01"}}`(トラップしない)。プラグインは戻りの `ok` を見て、
  その機能を隠すか縮退させる(`net.fetch` が E01 なら「ネットワークはストア版で」と出す、など)。
- **チャイルドセーフで見守られている子の端末**には開発者の鍵が無いので、その端末のサイドロードは**すべて制限モード**になる
  (保護者が Central の方針 `allow.plugins.ids` でその id を許しても同じ。rasq-client D-306 / D-308)。

## 7. 未決
- ~~D-005 content/action スロット仕様の形式定義~~ → `schema/slots.json`(登録簿)+ `schema/slot-registry.schema.json`。manifest v0.3 の `slots` / `roles` / `actions[].tags`
- ~~D-006 `ActionRef` 形式の確定~~ → `schema/action-ref.schema.json`(ActionRef = 実行時の完全修飾、ActionKey = `slot:<pluginId>:<actionId>` の永続キー、InstanceScope = この §3 の `scope`)
- ~~D-007~~ → 境界の IDL は `schema/boundary.schema.json` v0.2(`surface`、`change` / `drag` イベント、アプリ向けホスト関数)と `schema/host-fn.schema.json` v0.2。
  WASM ランタイムは **wasmi を第一期の確定**(rust/rasq_wasm_runtime_core)。wasm3 / WAMR との実測比較と iOS 実機計測は第二期
- host_fn の非同期化(v2: callId + poll/notify)。v1 は同期
- `net.fetch` のストリーミング / バイナリ大容量(v1 は bodyB64)
- モデルマニフェストの配布・審査フロー(ストア側)と `function.json` への `models` 欄追加
