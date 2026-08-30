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
- `on` — イベント → 自分の関数名(`tap` / `longPress` / `pointer*` / `confirm` / `change`)。
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

## 本文(RSL)

`RichText` の `p.source` は生文字列(ホストが MFM/HTML を解析)でも、解析済みスパン配列でもよい。
スパンの形は [`schema/rsl-span.schema.json`](../schema/rsl-span.schema.json)、
MFM 装飾は `transform` スパン(例: [`examples/mfm-body.rsl.json`](../examples/mfm-body.rsl.json))。

## 設定ページ(wasm 不要の静的 RRL)

設定第二階層は `rasq.settings-page/1`([`schema/settings-page.schema.json`](../schema/settings-page.schema.json))の
静的ドキュメント。値は Bind `$.settings.<key>`、書き込みは `on: {"change": "settings.set"}` を
ホストが KV へ写す。SNS プラグインは `settingsPages` として同形式を提供する(PROTOCOL §2.3 の設定ページ注記)。

## 障害隔離

ノード単位で try/catch され、壊れたノードはインラインの ⚠ ボックスに縮退する(V12)。
未知の `t` も同じ — プラグインが古いホストで動いてもクラッシュはしない。
