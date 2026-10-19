# bluesky-wasm — Bluesky (AT Protocol) SNS プラグイン

[`mastodon-wasm`](../mastodon-wasm) と同じ **`sns.*` 境界**([`../../PROTOCOL.md`](../../PROTOCOL.md) §4.3 /
[`../../CONTRACT.md`](../../CONTRACT.md))を Bluesky に当てた wasm プラグイン。画面コンストラクタは持たず、
`sns.*` に答えてホストの標準 PostTile に描かせる。

- plugin id: `rasq.official.bluesky` / `kind: bluesky` / 既定ホスト `bsky.social`
- scope: `rasq.official.bluesky:bluesky:bsky.social:sample:0.1.0:sns`

```sh
cargo test -p bluesky-wasm                                            # native: ホスト関数をフックでモック
cargo build --release --target wasm32-unknown-unknown -p bluesky-wasm # → target/wasm32-unknown-unknown/release/bluesky_wasm.wasm
```

## Mastodon / Misskey との違い

契約(ワイヤ)は同じ形。違いは全部 AT Protocol 側の作りに由来する。

| 項目 | Bluesky | 備考 |
|---|---|---|
| 接続先 | **すべてアカウントの PDS** `https://<host>/xrpc/<nsid>` | 読み取り系は PDS が AppView へ代理転送。`Accept: application/json`、JSON ボディは `Content-Type: application/json` |
| 認証 | `Authorization: Bearer <accessJwt>` | トークンは下記の JSON 文字列 |
| ログイン | `sns.login {host, identifier, password}`(**匿名ワーカー**・token 無し) | `com.atproto.server.createSession` → `{session:{did,handle,accessJwt,refreshJwt}, me}`。アプリパスワード推奨。誤りは `{code:"401"}` |
| トークン更新 | `ExpiredToken` → `refreshSession`(`Bearer <refreshJwt>`)→ 1 回だけ再試行 | 新しい組を `storage.set {ns:"safe", key:"session:<did>", value:{accessJwt, refreshJwt}}` に保存。以後の呼び出しは `storage.get` の組をホストの `token` より優先 |
| 投稿 id | **`at://` URI** | `url` は `https://bsky.app/profile/<handle>/post/<rkey>` |
| cid | メモリ上の `uri → cid` キャッシュ | いいね / リポスト / 返信 / 引用の strong ref 用。無ければ `getPosts` で引く |
| 本文 | プレーンテキスト + **facets** | 投稿側で UTF-8 バイト範囲の `#link` / `#tag` / `#mention`(`resolveHandle` で did 解決)を組む。表示はホストの RSL 解析 |
| 文字数 | **300 グラフェム** | 超過は `{code:"B02"}`。`sns.capabilities → {maxPostLength: 300}` |
| リアクション | 「いいね」のみ、⭐ 固定 | アクション id は `favorite`(ホストのお気に入りメニューと同じ)、`genreTag: Reaction`。`counts` はアクション id キー(`reply/repost/favorite/quote`) |
| リポスト | feed の `reasonRepost` を中身へ展開 (D-100/D-112) | 外側 id `<uri>#repost-by-<did>`、`repostedBy`、`repostTargetId` |
| TL | `home`=`getTimeline` / `global`=Discover フィード(`getFeed`) | `local`/`social` はホームに倒す。`tag:` は `searchPosts`、`chat:<did>` はチャット履歴 |
| ハッシュタグの候補 | `sns.search {kind: hashtags}`(コンポーザーの `#` の補完、rasq-client D-478) | タグの検索 API が無いので、トレンド(`getTrends`、失敗なら `getTrendingTopics`)+ Discover(`getFeed` 100 件の facets の `#tag` / `record.tags`)の池を前方一致(全角・半角カナ・大文字小文字を畳む)で絞る。同じタグ → 点(投稿の数 + トレンドの重み)の順、`count` 無し。池は `args.now` で 3 分メモリに置く。片方の失敗は無視、両方失敗で `{tags: []}` |
| 引用 | ネイティブ(`app.bsky.embed.record`) | `sns.quotes` = `getQuotes` |
| メディア | `uploadBlob`(生バイト) | `sns.uploadMedia` の `media.id` は **`{blob, alt, mimeType}` の JSON 文字列**、`url` は空。`sns.createPost` の `mediaIds` で戻すと `embed.images`(動画 mime なら `embed.video`)になる |
| DM | `chat.bsky.convo.*`(`atproto-proxy: did:web:api.bsky.chat#bsky_chat`) | `dmRooms` は失敗時 C01(ホストが DM 欄を隠す)。`sendDm` は `peerId` → `getConvoForMembers` → `sendMessage` |
| 関係 | フォロー / ブロックは `app.bsky.graph.*` **レコード**の作成・削除、ミュートは `muteActor` | フォロー申請は無い(`requested` は常に false) |
| 通知 | `listNotifications` | 同じ投稿への連続した like / repost を 1 件に畳む。対象投稿は `getPosts` 25 件ずつ |

## 未対応(C01 → ホストが UI を隠す)

- `sns.favorites`(ブックマークが無い)、`sns.resolveFollowRequest`、`sns.userList {kind: followRequests}`
- `sns.lists*`(まだ)、`sns.updateProfile`(まだ)
- `sns.clips*` / `sns.userPlays`(Misskey 固有)、`sns.applySettings` / `sns.settingsRead` / `sns.settingsWrite`
- `sns.emojis` は `{emojis: []}`(カスタム絵文字が無い)、`sns.trendingTags` は失敗しても空配列

## `now` の制限

wasm に時計は無い。`createdAt`(投稿 / いいね / フォロー)と `updateSeen` の `seenAt` は **ホストが
`args.now` に載せる ISO 8601**(rasq-client の `WasmSnsSession` は毎回付けている、D-175)を使う。
渡されなければ `1970-01-01T00:00:00.000Z` になる — 投稿は通るが TL では過去の日付に並ぶ。

## テスト用フック

native ビルドでは `host_fn` import の代わりに `set_host_hook(|fn, args| value)` /
`take_host_log()`([rasq-plugin-kit](../../../plugin-sample/rasq-plugin-kit) と同じ形)が有効で、
`net.fetch` / `storage.*` の応答を差し替えられる。wasm ABI(`alloc` / `dealloc` / `snapshot` / `patch` /
`call`)は Mastodon サンプルと同一。
