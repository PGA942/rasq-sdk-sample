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
| `net.fetch` | `{ method, url, headers, body }`(ホスト代理・dedupe・キャッシュ) | `network`(審査済み) |
| `ml.infer` | `{ backend: onnx|llama, model, input }` | `gpu_ml`(審査済み) |
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
- **配置はホストが解決**(端末 RAM・電源・従量回線・ポリシー・審査状態)。`placementHint` はヒント。結果の `value.placement` / `value.runtime` で実際にどこで動いたかが返る。
- onnx 実行時のテンソル詰め替え・トークナイズはマニフェストの `inputs[].semantic` / `tokenizer` に従ってホストが行う。
- **seq2seq 生成(onnx)**: KV キャッシュ無しの単発 forward で export されたモデルは、`runtimes.onnx.generation`(`decoderStartTokenId` / `eosTokenId` / `padTokenId` / `maxNewTokens` / `strategy: greedy`)に従って**ホストがデコーダーループ**(semantic `decoder_token_ids` のテンソルを伸ばしながら再実行)を回す。サンプリング付き生成はサーバー側(vLLM)に任せる。
- 例: [`examples/reaction-predictor.model.json`](examples/reaction-predictor.model.json)(onnx int8 + Triton)/ [`examples/astell-summarizer.model.json`](examples/astell-summarizer.model.json)(onnx fp32 単発 forward + generation / gguf + vLLM)。

## 4.3 SNS 抽象関数(イベントハンドラーの boundary 写像、M5d / D-074)

SNS プラグインのイベントハンドラー関数は「一定程度は抽象的な関数で共通化」
(デバイス側の基本思想)— **既存 SnsSession 契約の boundary 写像**として、
ホスト → プラグインの `call("sns.<fn>", args)` で呼ぶ。データは RRL では
なく **Post ワイヤ(`rasq.post/1`)** で返す:

- Post ワイヤ: `{id, author: {id, kind, instance, handle, displayName, avatarUrl?, bio?}, createdAt(ISO), body, bodyFormat, cw?, media?: [{id, type: image|video|gif|file, url, thumbnailUrl?, sensitive?, …}], poll?, quote?: Post, replyTo?: Post, replyToId?, repostedBy?, counts?, visibility?, localOnly?, directToIds?, reactions?, reactionEmojis?, myReaction?, url?}`。閲覧アカウントのキー付けはワイヤに含めない(取り込み時にホストが与える)。
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
- 以降(notifications / follow 系 / DM …)は同じ形で 1 本ずつ追加する。未実装関数は C01 — ホストは該当 UI を隠す(SnsSession の UnsupportedError と同じ扱い)。認証トークンは v1 ではホストが `args.token` で毎回渡す(サンプルは `i` に積む)— 実セッション登録の口と同時に整備する。

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

## 7. 未決
- D-005 content/action スロット仕様の形式定義 — `roles`/`slots` 宣言の JSON 化
- D-006 `ActionRef` 形式(`pluginId:accountKey:pluginVersion:fn`)の確定
- D-007 WASM ランタイム選定 — `wasmi` で仮組み済み(rust/)。wasm3 / WAMR との実測比較と iOS 実機計測が残
- host_fn の非同期化(v2: callId + poll/notify)。v1 は同期
- `net.fetch` のストリーミング / バイナリ大容量(v1 は bodyB64)
- モデルマニフェストの配布・審査フロー(ストア側)と `function.json` への `models` 欄追加
