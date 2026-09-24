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

1. **hello** ボタン(同梱アセット)を押す — ビルド不要で最小サンプルが動く。
   自分でビルドしたものは `.wasm のパスからインポート` に**絶対パス(`~/` 可)**を入れて **読み込み**
   (例: `~/Documents/rasq-sdk-sample/rust/target/wasm32-unknown-unknown/release/hello_rasq_plugin.wasm`。
   相対パスはアプリの作業ディレクトリ基準になるため使えない — ホーム配下なら補完は効く)
2. 初期 snapshot の RRL 画面が描画される(hello なら Text + ボタン 2 個)
3. **挨拶する** → `call("greet")` → storage 往復 + 本体 patch で Text が更新される
4. **パネルを開く** → `fow.open` で本物の FOW が開く。**+1** → surface patch で開いたまま差分更新
5. **境界TL** → `call("sns.timeline")` の Post ワイヤがホストの投稿タイル(アクションバー付き)で描画される

misskey-wasm-sample は同梱アセットとして最初から読み込まれており、
`TL取得` で実際の Misskey local TL(net.fetch 経由)を RRL で描画する
(アプリに同梱しているアセットはアプリと同じ信頼で**フル**なので、そのまま動く。自分で作ったものをネットワーク込みで試すときは、
下の**制限モード**を避けるために自分の鍵で署名した `.rasqpkg` を入れる)。

### サイドロードの 2 つのモード(全機能モード / 制限モード)

試験画面から入れたもの(サイドロード)は、**署名で動き方が変わる**(正典は [PROTOCOL §6.1](../PROTOCOL.md))。

- **全機能モード**: 自分の開発者鍵で署名した `.rasqpkg`。宣言した権限どおりに全部試せる。
  1. `rasq key init`(1 度だけ。`~/.config/rasq/dev-key.toml` に鍵を作る)→ `rasq key show` で指紋を見る
  2. Web コンソールの `/console/keys` で、その鍵を自分の RASQ アカウントに登録する
  3. `rasq build` → `dist/<id>-<version>.rasqpkg`(+ `.sig.json`)を、同じ RASQ アカウントでログインした rasq-client の試験画面から入れる
- **制限モード**: それ以外 — 署名の無いもの、ほかの人の鍵のもの、`rasq dev` の**生の `.wasm`**(上の「パスからインポート」)、あとから入れた `asset:`。
  アプリに同梱しているアセットは例外でフル。
  `net.fetch` / `gpu.compute` / `storage.unsafe` は `network` などを宣言していても `E01`、FOW は `fow.widget` だけ、
  `composer.open` / `astell.share` / `mute.add` / `url.open` も `E01`(`clipboard.set` / `toast` は使える)、燃料と時間の上限は半分、
  `actions[].auto` は読まない。描画のスロット(`post.transform` / `tl` / `viewer.overlay` の描画)は動く。

ストアの審査を通ったものは、いつも全機能。チャイルドセーフで見守られている子の端末には開発者の鍵が無いので、
そこでのサイドロードは**いつも制限モード**(自分のプラグインの制限モードでの見え方は、生の `.wasm` を読み込めば確かめられる)。

## 何をどこに書くか(デバイス側の基本思想との対応)

- **画面コンストラクタ** = `snapshot`/`patch` が返す RRL(→ [rrl.md](rrl.md))
- **イベントハンドラー** = `call` の各関数。UI イベント(`on: {tap: fn}`)も、
  SNS 抽象関数(`sns.*`、→ [sns-boundary.md](sns-boundary.md))も、
  ホストからの通知(`lifecycle` / FOW の `onClosed`)も全部ここに届く
- **ホスト機能** = `host_fn`(→ [host-functions.md](host-functions.md))
- **FOW(拡張面)** = `fow.open` で要求するホスト所有の面(→ [fow.md](fow.md))
