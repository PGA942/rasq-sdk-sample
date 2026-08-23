# RASQ Plugin Contract — Draft v0.3 (第一期 / Host ⇄ SNS plugin)

最終更新: 2026-08-22 / 根拠: `packages/rasq_plugin_api`(Dart 実装)・`packages/sns_misskey`(参照実装)・決定台帳 D-001〜D-015

この文書は **ホスト(RASQ クライアント)が SNS プラグインに期待する振る舞い**を、言語非依存の形でまとめたものです。
第一期は Dart 直書きアダプタ(`sns_misskey`)がこの契約を満たして動いており、M3 で WASM プラグインが同じ契約を境界(Snapshot/Patch/Call)越しに満たします。
`ORGATE-question/rasq-sdk-sample` はこの文書を正として SDK 側の型/スキーマを起こします。

表記: `?` = 省略可 / `[]` = 配列 / `{}` = マップ。ID 文字列はプラグインが発行し、ホストは不透明に扱う。

---

## 0. 用語と責務分離(D-003)

| 用語 | 意味 |
|---|---|
| **Plugin** | `PluginManifest` を持つ配布単位。種別 `kind`: `sns` / `action` / `view` / `trend` / `mediaViewer` / `postProcessor` |
| **Session** | 1アカウントぶんの認証済み API 面。`SnsPlugin.open(AccountSession)` で生成。ホストは `AccountId.key` ごとに1つ保持 |
| **Action** | `ActionRef` で呼ばれる副作用関数(リノート・リアクション…)。ノードではなく**参照** |
| **FOW** | Overlay Manager 上のオーバーレイ状態(`fow` レイヤー)。関数オーバーライド機構ではない |
| **Component** | RRL を返す純粋な UI 合成(M3)。第一期はホスト実装の L2 部品で代替 |
| **Store** | 状態は DSL の外。プラグインの返り値 → Store → Props(D-002/統合方針) |

---

## 1. マニフェスト `PluginManifest`(function.json の最小サブセット)

```jsonc
{
  "id": "rasq.official.misskey",        // 逆DNS風・安定ID
  "name": "Misskey Plugin",
  "version": "0.1.0",                    // SemVer。アカウントごとに異なる版を適用可
  "kind": "sns",
  "author": "RASQ Official",
  "description": "Misskeyに接続するためのプラグイン。",
  "supportedSns": ["misskey"],           // bluesky|misskey|mastodon|mixi2|other
  "permissions": ["network", "storage.safe", "register_action"],   // §8 操作語彙
  "actions": [ /* ActionDescriptor[] (§4) */ ],
  "termsOfService": "## 規約 …",         // Markdown。初回ログイン前に表示(同意が必要)
  "verified": true,                      // ストア審査済み。サイドロードは false
  "official": true
}
```
JSON Schema: `rasq-sdk-sample/schema/manifest.schema.json`

---

## 2. `SnsPlugin`(プラグイン単位・ネットワーク前提なし)

| 操作 | 入力 | 出力 | 備考 |
|---|---|---|---|
| `normalizeHost(input)` | 文字列 | host | `https://`・パス除去・小文字化(ホスト既定実装) |
| `inspectServer(host)` | host | `ServerInfo{host,name,software?,version?,description?,iconUrl?,registrationOpen?,maintainer?}` | サーバー検索/PDS変更 |
| `beginAuth(host, callback?)` | host, ディープリンク | `AuthFlow` | §3 |
| `open(AccountSession)` | セッション | `SnsSession` | ネットワークなし・安価 |

---

## 3. 認証 `AuthFlow` / `AccountSession`

```
AuthFlow = BrowserAuthFlow { url: URL, hint: 文字列, complete(): AuthResult }   // MiAuth / OAuth
         | CredentialAuthFlow { login(identifier, secret): AuthResult }      // ユーザー名+パスワード
AuthResult = { session: AccountSession, me: UserSummary }
AccountSession = { account: AccountId, pluginId, token, createdAt, displayName?, avatarUrl? }
AccountId = { kind, instance, handle }   key = "kind:instance:handle"
```
- トークンはホストのセキュアストレージにのみ保存。プラグインには `open()` 経由でのみ渡る。
- ブラウザ戻り後の確認はホスト UI(「認証を確認」)が `complete()` を呼ぶ(D-010)。
- Misskey 参照実装: `https://{host}/miauth/{session}?name&permission&callback` → `POST /api/miauth/{session}/check`。要求スコープ: read/write:account,notes,notifications,reactions,following,blocks,mutes,favorites,drive,messaging,chat。

---

## 4. アクション `ActionDescriptor` / `ActionRef` / `ActionResult`

```jsonc
// ActionDescriptor — アクションバーに登録するボタン1つ
{
  "id": "renote",
  "genreTag": "Repost",            // Repost|Reaction|Reply|Quote|Share|Bookmark|… 審査時付与。同タグは同スロットに束ねる
  "label": "リノート",
  "icon": "repeat",                // ホストのアイコン名(Material 名で開始)
  "doneIcon": "repeat_on",         // 未定義なら済み状態で無効化
  "confirmMode": "immediate",      // immediate|confirm|saveDiscard|stick
  "fow": "none",                   // none|widget|interstitial|system
  "countSource": "self",           // self|first|none
  "autoHookForbidden": false,      // 規約で自動呼び出し禁止 → 自動Hook候補から除外
  "destructive": false,
  "iconPerPost": true              // 投稿ごとにアイコンを差し替え可(SNSプラグインとその配下のみ。D-017)
}
// automation(導出): forbidden ? forbidden : immediate ? allowed : humanGate
```
- **短押し**(`performAction(actionId, postId)` 引数なし)の中身はプラグインが決める — Misskey 本番: 予測モデルで最有力の絵文字をリアクション。**長押し**は絵文字ピッカー FOW → `args.reaction`。
- `ActionResult.icon`(`{url}` または `{material}`)で**投稿ごとにアイコンを差し替え**(操作前に予測絵文字のエッジ抽出アイコンを出す等)。差し替えできるのは SNS プラグイン+配下プラグインの `iconPerPost: true` のアクションのみ。他プラグインは視認性のため固定(ホスト検証ゲート V7 が無視)。バンドル用既定アイコン `icon`/`doneIcon` は必須。
- `ActionRef` = `pluginId:accountKey:pluginVersion:fn`(D-006【提案】)。accountKey は `kind:instance:handle` なので両端から分割。
- `performAction(actionId, postId, args)` → `ActionResult{done, count?, message?}`。`args` 例: `{"reaction":"🎉"}`, `{"undo":true}`。
- 既定アクション(Misskey): `reply`(confirm, interstitial)/`renote`/`reaction`(長押しで絵文字ピッカー FOW)/`favorite`(countSource none)。

---

## 5. `SnsSession` 操作一覧

| 分類 | 操作 | 戻り値 | Misskey 参照 |
|---|---|---|---|
| 基本 | `account`, `pluginId`, `timelines: TimelineKind[]`, `actions: ActionDescriptor[]`, `maxPostLength` | — | 3000 |
| 自分 | `me()` | `UserSummary` | `i` |
| TL | `timeline(kind, cursor?, limit)` | `Page<Post>` | §6 動的ID |
| 投稿 | `post(id)`, `thread(id)`→`Thread{post,ancestors,descendants}`, `createPost(NewPost)`, `deletePost(id)` | | `notes/show`,`notes/conversation`,`notes/children`,`notes/create`,`notes/delete` |
| アクション | `performAction(actionId, postId, args)`, `actionStates(posts)` | `ActionResult` / `{postId:{actionId:ActionResult}}` | reactions/create|delete, notes/create(renoteId), favorites |
| 通知 | `notifications(cursor?,limit)`, `markNotificationsRead()` | `Page<NotificationItem>` | `i/notifications-grouped`(404→`i/notifications`) |
| ユーザー | `user(id)`, `userByHandle(handle,host?)`, `userPosts(id,cursor?,limit,filter)`, `users(ids)`, `relationships(ids)` | `UserProfile` / `Page<Post>` / `{id:FollowState}` | `users/show`, `users/notes`, `users/relation` |
| 関係 | `followers/following(id)`, `relationship(id)`, `setFollowing/setMuted/setBlocked(id,bool)`, `followRequests()`, `accept/rejectFollowRequest(id)`, `mutedUsers()`, `blockedUsers()`, `reportUser(id,comment)` | | `following/*`, `mute/*`, `blocking/*`, `users/report-abuse` |
| リアクション | `reactions(postId,type?,cursor?,limit)`, `reposters(postId)` | `Page<ReactionEntry>` / `Page<UserSummary>` | `notes/reactions`, `notes/renotes` |
| 検索 | `searchPosts(q)`, `searchUsers(q)`, `trendingTags()` | | `notes/search`, `users/search`, `hashtags/trend` |
| メディア/絵文字 | `uploadMedia(bytes,filename,mime?,sensitive,alt?)`→`MediaItem`, `customEmojis()`, `emojiUrl(shortcode)` | | `drive/files/create`(multipart), `emojis` |
| プロフィール | `updateProfile(ProfileUpdate)` | `UserProfile` | `i/update` |
| リスト/ブックマーク | `lists()`, `listInfo(id)`, `listMembers(id)`, `createList(name)`, `addToList/removeFromList`, `favorites()` | | `users/lists/*`, `i/favorites` |
| DM | `dmRooms()`→`DmRoom[]{id,kind:direct|chat,peer,lastPost?,unreadCount}`, `sendDm(room, NewPost)` | | §7 |
| ライフサイクル | `close()` | | |

未対応の操作は `UnsupportedError` を投げる(ホストは該当 UI を隠す)。エラーは `SnsApiException{code: ErrorCode}`(§9)。

---

## 6. タイムライン ID(`TimelineKind.id`)

| id | 意味 | Misskey |
|---|---|---|
| `home` / `local` / `social` / `global` | 基本TL(プラグインが `timelines` で宣言) | `notes/timeline` 等 |
| `recommended` | おすすめ(ホスト側でタブ名「おすすめ」= social に割当) | — |
| `relational` | リレーショナルTL(予約、D-011 保留) | — |
| `list:{listId}` | リストTL | `notes/user-list-timeline` |
| `tag:{tag}` | トレンドTL(ハッシュタグ) | `notes/search-by-tag` |
| `featured` | Today(ハイライト、ページングなし) | `notes/featured` |
| `dm:{peerUserId}` | DMルーム(限定公開投稿) | `notes/mentions(specified)` + 自分の `users/notes` |
| `chat:{peerUserId}` | DMルーム(ネイティブChat) | `chat/messages/user-timeline` |

`Page{items, nextCursor?}`。cursor はプラグイン定義(Misskey は `untilId`)。

---

## 7. DM(D-013)

- 「限定公開がDM代わりの SNS」をまとめるため、`visibility == specified|direct` の投稿を **相手ユーザー単位**に束ねて `DmRoom(kind: direct)`。
- ネイティブChat(Misskey 2025.4+ `chat/*`)があれば `DmRoom(kind: chat)` を追加。`chat/*` が 404/403 のインスタンスでは無視。
- 送信: direct → `notes/create{visibility:specified, visibleUserIds:[peer]}` / chat → `chat/messages/create-to-user{toUserId,text,fileId?}`(Chat は画像1枚)。

---

## 8. 操作語彙(権限・D-004)

`render` `replace_tile` `transform_content` `tag_media` `replace_media` `register_action` `decorate_avatar` `fow.widget` `fow.interstitial` `fow.system` `network` `gpu_ml` `storage.safe` `storage.unsafe` `auto_hook`

PostTile スロット×権限(原典一致): avatar=`decorate_avatar`(審査済みのみ) / content=`transform_content`(sideload は GPU/Net 不可) / media=sideload は `tag_media` のみ、審査済みは `replace_media` / action=`register_action`(審査済み) / 完全置き換え=`replace_tile`(審査済み+審査制約)。interstitial の GPU/ML は審査済みのみ。ストアは審査済みのみ掲載。メディアビューワープラグインは全画面遷移時のみ RRL 描画。

---

## 9. エラーコード(`ErrorCode`)

`000-999` = HTTP / `Axx` 設定 / `Bxx` 影響設定(認証・通信) / `Cxx` プラグイン間受け渡し / `Dxx` データ破損 / `Exx` 規制 / `Fxx` 予約。
既知: `404`(簡易表示), `408`(簡易表示), `405 Method not Acceptable`, `500`, `401`, `A25 No Trend Requirement`, `D2F Relational Timeline Broken`, `B01 Network Unreachable`, `B02 TLS Handshake Failed`, `B03 HTTP Client Error`。

---

## 10. ホスト側の表示契約(プラグインが前提にしてよいもの)

- **Overlay Manager**: すべてのオーバーレイは `OverlayManager` 経由。階層 `fow < sidebar < dropdown < vcVideo < bottomSheet < modal < rai`。modal/bottomSheet/fow はスタック、dropdown/sidebar/vcVideo は排他。背面タップ: modal 無効 / bottomSheet・dropdown・interstitial FOW は破棄 / sidebar は閉じる。
- **FOW**(D-012): 下部ドック・ドラッグハンドルで上下リサイズ(`heightFactor` min/max)・複数はボトムアップ配置・`confirmMode` = immediate|confirm|saveDiscard|stick・子FOWは `ownerId` で連動クローズ。
- **状態**: アクション済み/カウント/フォロー/既読は Store(`ActionStateStore`/`FollowStateStore`/`ReadStateStore`)。`actionStates()` や `ActionResult` の返り値だけが Store を更新する。
- **L2 部品**(ホスト実装): Avatar/Badge/RichText/MediaGrid/ActionButton/Timestamp/Button/ListTile系/SectionHeader/Input/Tabs/UserRow/UserTileLarge/エラー・空・ローディング。ジオメトリトークン(spacing・最小タップ域44dp)はプラグイン変更不可。
- **RSL(本文)**: ホストがインライン解析(URL/@mention/#tag/:emoji:)。MFM の装飾関数は第一期では剥がして本文のみ表示(M3 でレンダラー対応)。
- **RRL 意味タグ**(D-019): 投稿内容を構成するノードにのみ `tag` を付ける(`main_text` `cw` `pic` `video` `gif` `file` `vote` `vote_option` `quote` `link` `hashtag` `mention` `emoji` `reaction` `user_name` `user_handle` `time`)。`Icon`・`Avatar`・`ActionBar`・レイアウトには付けない。RAi の位置検索、タイムラインソート、リレーショナルTL の抽出が参照する。
- **実行/ワイヤ**(D-018/D-020): 中心は WASM。onnx / llama.cpp は `ml.infer` ホスト関数経由の補助(常時処理は想定外)。ワイヤは JSON v1(`rasq.boundary/1`)。内部表現は JSON でも RRL そのままでも可。詳細は `rasq-sdk-sample/PROTOCOL.md`。

---

## 11. 未決(M3 までに閉じる)

- D-005 content/action を分けたスロット仕様の形式定義(旧 D1〜D10 は素材)
- D-006 `ActionRef` 形式の確定
- D-007 WASM ランタイム選定(iOS インタープリタ基準)と境界プロトコル(Snapshot/Patch/Call/Lifecycle)の IDL 化 — `rasq-sdk-sample` 側で行う
