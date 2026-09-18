# RASQ SDK sample

RASQ(プラグインベース SNS クライアント)の **プラグイン SDK の仮組み**。
第一期はホスト側(`ORGATE-question/rasq-client` の `packages/rasq_plugin_api`)で Dart の契約を切り、
Misskey アダプタ(`packages/sns_misskey`)で検証したものを、ここで言語非依存の契約として固めていく。

## ガイド(まずここから)

- [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md) — プラグインの形・ビルド・rasq-client での試し方
- [`docs/rrl.md`](docs/rrl.md) — RRL 画面(ノード/意味タグ/Bind/イベント/patch/設定ページ)
- [`docs/host-functions.md`](docs/host-functions.md) — ホスト関数(storage / net.fetch / alert / fow.* / ml.infer)
- [`docs/fow.md`](docs/fow.md) — FOW 拡張面(open/close・anchor・親子・surface patch・ライフサイクル)
- [`docs/sns-boundary.md`](docs/sns-boundary.md) — SNS 抽象関数(`sns.*`)と Post ワイヤ `rasq.post/1`
- [`docs/slots.md`](docs/slots.md) — **スロットプラグイン契約(正典、v0.9)**: 9 スロットの call / ホスト関数 / FOW の着き方。機械可読な登録簿は [`schema/slots.json`](schema/slots.json)。
- [`docs/cli.md`](docs/cli.md) — **`rasq` CLI の設計(D-244)**: ワークスペース(`rasq.toml`)、雛形、配下へのバンドル、`.rasqpkg` の目次 [`schema/bundle.schema.json`](schema/bundle.schema.json)、確認・ビルド・公開。
  `plugin-sample/*/CONTRACT.md` はここへのポインタ

## サンプル

- [`rust/hello-rasq-plugin`](rust/hello-rasq-plugin) — **最小サンプル**(約 200 行・オフライン)。
  RRL 画面 / storage / patch / FOW + surface patch / sns.* canned を 1 つずつ最小の形で
- [`rust/misskey-wasm-sample`](rust/misskey-wasm-sample) — フル参考実装(実 Misskey を net.fetch で叩く)

- [`CONTRACT.md`](CONTRACT.md) — Host ⇄ SNS plugin 契約 draft v0.2(`rasq-client/docs/PLUGIN_CONTRACT.md` と同内容)
- [`PROTOCOL.md`](PROTOCOL.md) — 境界プロトコル draft v0.1(Snapshot / Patch / Call / Lifecycle / ホスト関数 wasm ABI / RSL スパン / RRL ノードと意味タグ / 検証ゲート / ml.infer 配置解決)
- [`schema/rrl-node.schema.json`](schema/rrl-node.schema.json) / [`schema/boundary.schema.json`](schema/boundary.schema.json) — RRL ノードと境界メッセージの JSON Schema
- [`schema/rsl-span.schema.json`](schema/rsl-span.schema.json) — 本文(RSL)スパンのワイヤ。MFM 装飾は `transform` スパン
- [`schema/host-fn.schema.json`](schema/host-fn.schema.json) — ホスト関数(`rasq.hostfn/1`)の fn 別 args/value
- [`schema/model-manifest.schema.json`](schema/model-manifest.schema.json) — ML モデルマニフェスト(デバイス: onnx runtime / llama.cpp、サーバー: Triton / vLLM。配置はホストが解決)
- [`examples/post-tile.rrl.json`](examples/post-tile.rrl.json) — 意味タグ付き PostTile の snapshot 例
- [`examples/mfm-body.rsl.json`](examples/mfm-body.rsl.json) — MFM transform 入り本文スパンの例
- [`examples/reaction-predictor.model.json`](examples/reaction-predictor.model.json) / [`examples/astell-summarizer.model.json`](examples/astell-summarizer.model.json) — モデルマニフェスト例(onnx+Triton / gguf+vLLM)
- [`schema/manifest.schema.json`](schema/manifest.schema.json) — `PluginManifest`(function.json 最小サブセット)の JSON Schema(v0.3: `schema` / `slots` / `roles` / `actions[].tags`)
- [`schema/slots.json`](schema/slots.json) / [`schema/slot-registry.schema.json`](schema/slot-registry.schema.json) — **スロット登録簿**(D-005): 差し込める面ごとのティア / 置き場所 / call / ホスト関数 / 権限 / ライフサイクル。仮案の面は `planned`
- [`schema/action-ref.schema.json`](schema/action-ref.schema.json) — `ActionRef` / `ActionKey` / `InstanceScope` の文法(D-006)
- [`examples/manifests/`](examples/manifests) — plugin-sample 12 個 + ページ型(計画中)のマニフェスト例
- `python3 schema/validate.py` — スキーマと例、登録簿とマニフェストの突き合わせ(CI の `schema` ジョブ。`pip install jsonschema referencing`)
- [`examples/misskey.function.json`](examples/misskey.function.json) — Misskey 公式プラグインのマニフェスト例

## SDK(CLI)の想定フロー(ユーザー指定)

プラグイン作者向けの CLI は次の流れになる想定。ここに書いておき、実装時に `schema/` の登録簿とマニフェストを土台にする。
コマンド・ディレクトリ・配布物の形まで落とした設計は [`docs/cli.md`](docs/cli.md)(D-244)。
**実装済み(第一期 ①〜⑤)**: [`rust/rasq-cli`](rust/rasq-cli) の `rasq init / new / check / test / call / build [--raw] / registry / token / dev / key / inspect / publish` と、
[`templates/`](templates)(stable 9 スロット + sns の雛形)、[`rust/rasq-plugin-kit`](rust/rasq-plugin-kit)(kit の正典)。

```sh
cd rust && cargo build --release -p rasq-cli          # → rust/target/release/rasq
rasq init --namespace net.example && rasq new post.action later && rasq check && rasq build --raw
rasq call later post.action --args '{"token":"t","actionId":"hello","post":{"body":"abc"},"account":{}}'   # wasm そのものを wasmi で叩く
rasq token set misskey:misskey.io:yamada   # トークンは非表示入力 → ~/.config/rasq/credentials.toml(0600)
rasq dev --watch                           # ビルド → 往復検査 → dev 画面に入れる絶対パス(変更のたびに再ビルド)
rasq key init && rasq build                # dist/<id>-<version>.rasqpkg(zstd tar + bundle.json)+ 開発者署名 .sig.json
rasq inspect dist/net.example.later-0.1.0.rasqpkg   # 目次 / ハッシュ / 署名を確かめる
rasq token set --central 100.91.218.56:8080 && rasq publish --central http://100.91.218.56:8080   # 掲載 → 版 → 配布物(--dry-run で要求の書き出しだけ)
```

1. **土台を読む**: ベースになる SNS プラグイン(と、データを流すための Access Token)を読み込む。SNS プラグイン自体を作るときは何も読まない。
2. **雛形を選ぶ**: スロット(`schema/slots.json`)ごとの雛形から選ぶ。1 つ作るごとに、土台の SNS プラグインの**配下にバンドル**される(ActionRef の `pluginId` と `wasm.call` の親子関係)。
3. **コーディング**: wasm 化した既存ライブラリの取り込みや、添付ファイル(アセット / モデル)の書き込みを含む。
4. **確認**: Flutter(rasq-client の「WASMプラグイン試験 (dev)」)での動作確認と簡易テスト(`cargo test` + `schema/validate.py`)。
5. **ビルド**: デプロイ向けのビルド(`.rasqpkg`、ストア掲載用の署名は Central 側)。

## 次にやること(M3)

1. ~~`misskey-wasm-sample` で `PROTOCOL.md` の `snapshot/patch/call` を WASM 側から出す最小サンプル(まずは JSON v1)~~ → [`rust/misskey-wasm-sample`](rust/misskey-wasm-sample)
2. ~~WASM ランタイム選定(D-007: wasm3 / WAMR / wasmi、iOS インタープリタ基準)の実測~~ → `wasmi` を [`rust/rasq_wasm_runtime_core`](rust/rasq_wasm_runtime_core) に組み込み、Flutter プラグイン [`rasq_wasm_runtime`](rasq_wasm_runtime) から実行を確認(macOS 実行、iOS シミュレータ / Android 全 ABI ビルド)。wasm3/WAMR との比較実測と iOS 実機計測は未着手(`rust/README.md` 参照)
3. ~~スロット宣言(D-005)と `ActionRef`(D-006)の形式確定~~ → `schema/slots.json` / `schema/action-ref.schema.json`。境界の IDL は `schema/boundary.schema.json` v0.2(D-007)。WASM ランタイムは wasmi を第一期の確定とし、比較計測は第二期
4. 性能が必要なら Snapshot を FlatBuffers/ZeroCopy 化(論理モデルは維持)

## mixi2 サンプル (`rust/mixi2-wasm`, D-206)

mixi2 の公式 API は申請制で、認証は OAuth(RASQ サーバーのブローカー `/auth/sns/mixi2/begin`)。どちらも揃うまでは
**ダミーモード**(ホスト `dummy` / トークン `dummy:...`)がプラグイン内のメモリで `sns.*` をこなす(ログイン → TL →
いいね / リポスト → 投稿 → 通知)。実モードのエンドポイント(`api` モジュール)と ワイヤ変換は未検証の仮置き。
ビルド: `cargo build --release --target wasm32-unknown-unknown -p mixi2-wasm` → クライアントの `assets/plugins/mixi2_wasm.wasm`。
