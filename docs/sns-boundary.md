# SNS 抽象関数 — SnsSession 契約の boundary 写像

> 正典: [`PROTOCOL.md`](../PROTOCOL.md) §4.3。参考実装:
> [`rust/misskey-wasm-sample`](../rust/misskey-wasm-sample/src/lib.rs)(実 Misskey)/
> [`rust/hello-rasq-plugin`](../rust/hello-rasq-plugin/src/lib.rs)(canned・オフライン)。

SNS プラグインの**イベントハンドラー関数は抽象的な関数で共通化**する(デバイス側の基本思想)。
ホストの Dart 契約 `SnsSession` がそのまま wasm 境界の `call("sns.<fn>")` に写り、
ホスト側アダプター `WasmSnsSession` が取り込む。**未実装の関数は C01 を返すだけでよい**
(ホストは該当 UI を隠す)。1 本ずつ増やせる。

## データワイヤ: `rasq.post/1`

RRL(画面)とは別の**データの形**。投稿は:

```json
{ "id": "n1",
  "author": { "id": "u1", "kind": "misskey", "instance": "misskey.io",
              "handle": "douglas", "displayName": "ダグラス", "avatarUrl": null, "bio": null },
  "createdAt": "2026-08-30T12:00:00.000Z",
  "body": "本文", "bodyFormat": "mfm", "cw": null,
  "media": [{ "id": "f1", "type": "image", "url": "https://…", "thumbnailUrl": null, "sensitive": false }],
  "quote": null, "replyTo": null, "replyToId": null,
  "counts": { "reply": 0, "renote": 2, "reaction": 3 },
  "reactions": { "👍": 3 }, "visibility": "public", "myReaction": null }
```

- 閲覧アカウントのキー付けはワイヤに**含めない**(取り込み時にホストが与える)
- `quote` / `replyTo` は Post の入れ子(深さは自由。misskey サンプルは 1 段)
- ページは `{page: {items: [...], nextCursor?}}`

## 関数一覧(v1 実装済み)

| fn | args | 戻り |
|---|---|---|
| `sns.timeline` | `{kind, limit, cursor?}` | `{page}` |
| `sns.thread` | `{id}` | `{thread: {post, ancestors(古い順), descendants}}` |
| `sns.actions` | `{}` | `{actions: [ActionDescriptor JSON]}`(アクションバー宣言) |
| `sns.performAction` | `{actionId, postId, args}` | `{result: {done, count?, message?}}` — 短押しの中身はプラグインが決める |
| `sns.createPost` | `{draft}`(下記) | `{post, notice?: {code, message}}` — `notice` は投稿できたが利用者に知らせたいこと(v0.25、PROTOCOL §4.3) |
| `sns.deletePost` | `{id}` | `{}` |
| `sns.me` | `{}` | `{user}` |
| `sns.user` | `{id}` または `{handle, userHost?}` | `{profile}` |
| `sns.userPosts` | `{id, filter: posts\|replies\|quotes\|media\|withReplies, limit, cursor?}` | `{page}` — 返信/引用の振り分けはプラグイン側で適用 |
| `sns.reactions` | `{postId, type?, limit, cursor?}` | `{page: {items: [{id, user, reaction, createdAt, emojiUrl?}]}}` |
| `sns.reposters` / `sns.quotes` | `{postId, limit, cursor?}` | `{page: {items: [user]}}` / `{page: {items: [Post]}}` |

`draft`(NewPost ワイヤ):
`{text, cw?, replyToId?, quoteOfId?, repostOfId?, visibility?, localOnly?, reactionAcceptance?, directToIds?, mediaIds?, poll?: {choices, multiple?, expiresAfterSec?}}`

## 接続先とトークン(v1 の暫定規約)

- ホストは毎呼び出しの args に接続情報を**追い乗せ**できる(`WasmSnsSession.extraArgs`)。
  misskey サンプルは `host` / `baseUrl`(テスト用)/ `token` を読む
- ユーザーの**所属ホスト**は接続先の `host` と衝突するため `userHost` で渡す
- 認証トークンの正式な保管・受け渡しは「実セッション登録の口」(ログインフロー接続)と同時に整備する

## エラー

`result.error = {code, message}` のコードはそのままホストの `SnsApiException` に写る
(`404` → ユーザー無し扱い、`B01` → ネットワーク、`C01` → 未対応 = UI 非表示)。
コード体系は [`rasq-client` の ErrorCode](../CONTRACT.md) 準拠(HTTP 3 桁 or 英字+2 桁)。

## 最小実装(hello プラグイン)

ネットワーク無しでも成立する — canned の Post ワイヤを返すだけで、
ホストの投稿タイル・アクションバーがそのまま動く:

```rust
"sns.timeline" => Ok(json!({ "page": { "items": [wire_post("h1", "…")], "nextCursor": null } })),
"sns.actions" => Ok(json!({ "actions": [{ "id": "wave", "genreTag": "Reaction", "label": "手を振る", "icon": "favorite_border" }] })),
"sns.performAction" => Ok(json!({ "result": { "done": true } })),
```
