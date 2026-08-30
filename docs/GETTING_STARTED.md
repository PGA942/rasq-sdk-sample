# はじめる — RASQ プラグインの形と試し方

> 規約の正典は [`PROTOCOL.md`](../PROTOCOL.md) / [`CONTRACT.md`](../CONTRACT.md) / [`schema/`](../schema/)。
> このガイドは「まず動かす」ための最短ルート。

## プラグインの正体

RASQ プラグインは **wasm32 の cdylib** で、次の 5 つを export するだけのもの:

| export | 役割 |
|---|---|
| `snapshot() -> u64` | 画面の RRL ツリー全体(`rasq.boundary/1` の `snapshot` メッセージ) |
| `patch() -> u64` | 保留中の差分(`patch` メッセージ。無ければ空 ops) |
| `call(fn, args, callId) -> u64` | ホストからの関数呼び出しに `result` で答える(イベント/SNS 抽象関数/通知すべてここ) |
| `alloc(len) -> ptr` / `dealloc(ptr, len)` | ホストとの線形メモリ受け渡し |

戻り値の `u64` は `(ptr << 32) | len` に詰めた JSON バッファの位置。
ホスト機能(storage / net.fetch / fow.open …)は import `rasq.host_fn` で呼ぶ(同期 v1、PROTOCOL §4.1)。

最小の実装は [`rust/hello-rasq-plugin/src/lib.rs`](../rust/hello-rasq-plugin/src/lib.rs)
(約 200 行、コメント付き)。フル機能の参考実装は
[`rust/misskey-wasm-sample/src/lib.rs`](../rust/misskey-wasm-sample/src/lib.rs)。

## ビルド

```bash
cd rust
cargo build --release --target wasm32-unknown-unknown -p hello-rasq-plugin
# → target/wasm32-unknown-unknown/release/hello_rasq_plugin.wasm
```

## rasq-client で試す

rasq-client(Flutter)を起動し、カタログ → **「WASMプラグイン試験 (dev)」**:

1. `.wasm のパスからインポート` にビルド成果物のパスを入れて **読み込み**
2. 初期 snapshot の RRL 画面が描画される(hello なら Text + ボタン 2 個)
3. **挨拶する** → `call("greet")` → storage 往復 + 本体 patch で Text が更新される
4. **パネルを開く** → `fow.open` で本物の FOW が開く。**+1** → surface patch で開いたまま差分更新
5. **境界TL** → `call("sns.timeline")` の Post ワイヤがホストの投稿タイル(アクションバー付き)で描画される

misskey-wasm-sample は同梱アセットとして最初から読み込まれており、
`TL取得` で実際の Misskey local TL(net.fetch 経由)を RRL で描画する。

## 何をどこに書くか(デバイス側の基本思想との対応)

- **画面コンストラクタ** = `snapshot`/`patch` が返す RRL(→ [rrl.md](rrl.md))
- **イベントハンドラー** = `call` の各関数。UI イベント(`on: {tap: fn}`)も、
  SNS 抽象関数(`sns.*`、→ [sns-boundary.md](sns-boundary.md))も、
  ホストからの通知(`lifecycle` / FOW の `onClosed`)も全部ここに届く
- **ホスト機能** = `host_fn`(→ [host-functions.md](host-functions.md))
- **FOW(拡張面)** = `fow.open` で要求するホスト所有の面(→ [fow.md](fow.md))
