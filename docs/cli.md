# `rasq` CLI — プラグイン作者向けコマンドの設計(D-244、第一期は設計のみ)

> 正典: スロットは [`slots.md`](slots.md) / [`../schema/slots.json`](../schema/slots.json)、マニフェストは
> [`../schema/manifest.schema.json`](../schema/manifest.schema.json)、配布物の目次は
> [`../schema/bundle.schema.json`](../schema/bundle.schema.json)(この文書で新設)。
> 流れそのものは [`../README.md`](../README.md#sdkcliの想定フローユーザー指定未実装) の 5 段。ここはそれを
> コマンド・ディレクトリ・ファイル形式・ホスト側の前提まで落としたもの。

## 0. 決めたこと(要約)

| # | 決定 | 根拠 |
|---|---|---|
| D-244a | CLI は Rust の単一バイナリ `rasq`(`rust/rasq-cli`)。スキーマ・登録簿・雛形をバイナリに同梱し、ネット無しで `init / new / check / build` が完結する | 作者環境に Python を要求しない。`schema/validate.py` と同じ検査を `jsonschema` クレートで行う |
| D-244b | 作業単位は **ワークスペース**(`rasq.toml`)。土台の SNS プラグイン 1 つ(無しも可)+ 配下の子プラグイン群 + ライブラリ + 添付 | README の流れ 1〜2(土台を読む → 雛形を配下にバンドル) |
| D-244c | 配下の子プラグインの id は `<土台 id>.<名前>`。土台が**自分のもの**なら 1 つの `.rasqpkg` に同梱(root + children)、**他人のもの**なら子を root にした `.rasqpkg` を別に出し `requires` で土台を宣言 | pluginId の文法(`:` 無し、`.` 区切り)をそのまま使え、ActionRef / ActionKey が変わらない。第三者作者が公式 SNS プラグインを配布し直す事故を防ぐ |
| D-244d | 配布物 `.rasqpkg` = zstd 圧縮 tar(D-034)、先頭に `bundle.json`(`rasq.bundle/1`)。ハッシュは目次に、署名は `.rasqpkg` 全体の SHA-512 ダイジェスト(生 64 バイト)への ed25519 | ホストの `verifyArtifact` と同じ署名対象。ストア鍵は Central が `upload_artifact` で付け、CLI は**開発者鍵**でサイドロード用に署名するだけ |
| D-244e | Access Token は**リポジトリの外**(`~/.config/rasq/credentials.toml`、0600)。`rasq.toml` には accountKey だけ書く。実行時は `sns-boundary.md` の暫定規約どおり args に `host` / `token` を追い乗せ | トークンをコミットさせない。ホスト側の受け渡し規約が固まるまで暫定規約に乗る |
| D-244f | wasm 化した既存ライブラリは **画面を持たない wasm モジュール**として `libs/` に置き、`wasm.call({lib, fn, args})` で呼ぶ。Rust クレートとして直接リンクできるものは普通に `Cargo.toml` の依存にする(CLI は関与しない) | `wasm.call` は登録簿にあるが未配線。目次に `libs` を持たせることが「構成許可」の根拠になる |
| D-244g | 確認(Flutter)は第一期は **パス読み込み**(rasq-client の「WASMプラグイン試験 (dev)」の絶対パス取り込み)。`rasq dev` はビルドして取り込み用のパスを出し、`--watch` で再ビルドする。ホットリロード用の dev サーバーは第二期 | 今のホストに口がある方法だけを第一期に入れる |
| D-244h | 公開は Central の管理 API(`PUT /admin/plugins/{id}` → `POST …/versions` → `PUT …/artifacts/{id}`)に**そのまま**流す。作者の自己申請 API(WF p9 の公開申請)は第二期 | 今ある口は管理 API だけ。`--dry-run` で 3 つの要求を JSON に書き出せるので、審査担当が投入する運用でも回る |
| D-244i | 12 リポジトリに同一内容で複製されている `kit/`(`rasq-plugin-kit`)を SDK 側 `rust/rasq-plugin-kit` に引き上げ、雛形はそれに依存する。plugin-sample 側の切り替えは各リポジトリの次の更新時(今回は既定ブランチを触らない) | md5 が 12 個とも同じ。契約と同じく正典を SDK に置く |

## 1. 流れとコマンドの対応

```
   README の段        コマンド                          生成物 / 参照物
   1 土台を読む        rasq init [--base <pkg|id>]       rasq.toml, base/, Cargo.toml(ワークスペース)
                      rasq token set <accountKey>       ~/.config/rasq/credentials.toml
   2 雛形を選ぶ        rasq new <slot> <name>            plugins/<name>/ (Cargo.toml, src/lib.rs, manifest.json)
   3 コーディング      rasq add lib|asset|model <path>   libs/ assets/ models/ + rasq.toml の項目
   4 確認             rasq check / rasq test / rasq dev  wasm32 ビルド + スキーマ + manifest 往復 + cargo test / 取り込みパス
   5 ビルド           rasq build [--release]            dist/<id>-<version>.rasqpkg (+ .sig.json)
   (5')公開           rasq publish --central <url>      Central 管理 API への 3 要求(--dry-run で JSON)
   -                 rasq registry [slots|templates]   同梱の slots.json を表示
```

## 2. ワークスペース

```
my-misskey-ext/
├── rasq.toml              # ワークスペース定義(下記)
├── Cargo.toml             # [workspace] members = ["plugins/*"](CLI が管理)
├── base/                  # 土台(読み取り専用。rasq init --base が展開。git には入れない)
│   ├── manifest.json      #   土台の function.json
│   └── misskey.wasm
├── plugins/
│   └── later/             # rasq new post.action later
│       ├── Cargo.toml     #   rasq-plugin-kit 依存、cdylib
│       ├── manifest.json  #   id = rasq.official.misskey.later (土台が自分のものの場合) / net.example.later (他人の土台)
│       ├── src/lib.rs
│       └── tests/         #   set_host_hook でホスト関数をモックしたテスト(bluesky-wasm と同じ型)
├── libs/                  # wasm 化した既存ライブラリ(rasq add lib)
├── assets/                # 添付(rrl / rsl / 画像 / 辞書)
├── models/                # *.model.json + 本体(lazy なら本体は置かない)
├── dist/                  # rasq build の出力
└── .rasq/                 # ローカル状態(最後のビルド、dev の取り込みパス)。トークンは置かない
```

`rasq.toml`:

```toml
schema = "rasq.workspace/1"
sdk = "0.1"                          # 使った SDK の契約版(slots.md の版ではなく CLI の版)

[base]                               # 無ければ「SNS プラグインを作る」ワークスペース(§2.1)
id = "rasq.official.misskey"
version = "0.1.0"
source = "store:https://100.91.218.56:8080"   # or "file:base/rasq.official.misskey-0.1.0.rasqpkg"
owned = false                        # true なら子を同梱して 1 つの .rasqpkg にする(D-244c)
account = "misskey:misskey.io:yamada" # データを流すアカウント(トークンは credentials.toml)

[[plugins]]
name = "later"
slot = "post.action"                 # 雛形。manifest.json の slots と一致していること(rasq check)

[[libs]]
name = "rustface"
path = "libs/rustface.wasm"
exports = ["detect"]

[[assets]]
path = "assets/settings.rrl.json"
kind = "rrl"

[[models]]
manifest = "models/gliner-ner.model.json"
lazy = true
```

### 2.1 SNS プラグインを作る場合

`rasq init --sns <id>` は `[base]` を書かず、ワークスペース直下に `sns/`(雛形 = `rust/hello-rasq-plugin` の `sns.*` 部分、
本格例は `rust/misskey-wasm-sample`)を作る。この場合は自分が root なので、`rasq new` で足した子は常に同梱される。
トークンは `rasq token set` で同じく外に置き、`sns.login` の実装確認に使う。

### 2.2 土台を読む(`rasq init --base`)

- `file:<path>.rasqpkg` … 展開して `base/` に置く。`bundle.json` の root が `kind = sns` でなければ拒否
- `store:<central url>` … `GET /plugins/{id}/manifest` → `downloadArtifact` で `bundle` を取り、`sha512` を照合して展開。
  Central が無い開発中は `file:` を使う(同梱サンプルは `rust/misskey-wasm-sample` をビルドして `rasq pack --sns` で作れる)
- `owned` は既定 `false`。`true` にできるのは `base/manifest.json` の `id` が自分の名前空間(`rasq.toml` の `[author] namespace`)で始まるときだけ

## 3. 雛形(`rasq new <slot> <name>`)

- 雛形は **登録簿の `status: stable` なスロットごと**に 1 つ。`rasq registry templates` で一覧。planned(`page` など)は
  `--allow-planned` を付けたときだけ出せ、`rasq check` が警告を出す(validate.py と同じ扱い)
- 雛形の中身は plugin-sample の各リポジトリの**最小化版**(manifest を返す `plugin.manifest`、そのスロットの `call` を
  1 つ、`fow.open` が要るスロットは空の FOW)。フル機能の例としては plugin-sample を README から指す
- 生成される `manifest.json` は `manifest.schema.json` v0.3 に合う形で、`id` は §0 D-244c の規則で決める
  (`owned = true` → `<base.id>.<name>`、そうでなければ `<namespace>.<name>`)、`slots = [<slot>]`、`permissions` は
  登録簿の `permissions.required` をそのまま入れる
- 生成直後に `rasq check` が通る状態であること(雛形の CI: SDK 側 `rust` ジョブで全雛形を `rasq new` → `check`)

雛形の対応表(第一期、登録簿の stable 9 スロット):

| slot | 雛形の骨子 | 元にする plugin-sample |
|---|---|---|
| `post.transform` | `post.transform` で本文を書き換えて返す | post-process-plugin |
| `tl` | `tl.sort` で並べ替え、タブ名を返す | timeline-plugin |
| `trend.suggest` | 共起辞書からチップを返す | trend-plugin |
| `viewer.overlay` | 画像を受けて `viewer.overlay.set` | media-fullscreen-plugin |
| `composer.media` | `composer.media.preview` → FOW で適用 | composer-media-plugin(変形 / 加工 / 描画は `--variant`) |
| `composer.text` | `text.changed` → 候補 FOW → `composer.text.replace` | composer-text-plugin |
| `astell.action` | ルーム FOW → `astell.share` | astell-plugin |
| `post.share` | 共有 FOW の項目 → `composer.open` | post-share-plugin |
| `post.action` | `actions[].tags` 付き manifest + `fow.open` | post-action-plugin |

## 4. コーディング支援(`rasq add`)

| コマンド | すること | 記録先 |
|---|---|---|
| `rasq add lib <name> <x.wasm> --exports f1,f2 [--source ..] [--license ..]` | `libs/` にコピー、`wasm32` で export が存在するか `wasmi` で確認 | `[[libs]]` |
| `rasq add lib <name> --crate <crate>` | `cargo build --target wasm32-unknown-unknown` で薄い cdylib ラッパを作って `libs/` へ(第二期。第一期は `.wasm` 持ち込みのみ) | 同上 |
| `rasq add asset <file> [--kind rrl\|rsl\|image\|audio\|data]` | `assets/` にコピー。`rrl` / `rsl` は `rrl-node` / `rsl-span` スキーマで検査 | `[[assets]]` |
| `rasq add model <x.model.json> [--lazy]` | `model-manifest.schema.json` で検査。`lazy` でなければ本体の `sha512` を照合して `models/` へ | `[[models]]` |

`wasm.call` の引数(ホスト側の配線待ち、登録簿では「構成許可」):

```json
{ "lib": "rustface", "fn": "detect", "args": { "png": "<base64>" } }      // ライブラリ
{ "plugin": "rasq.official.misskey.later", "fn": "later.add", "args": {} } // root → child(一方向)
```

ホストは `bundle.json` の `libs[].name` / `children[].id` に無い呼び先を `E02`(権限なし)で断る。

## 5. 確認(`rasq check` / `rasq test` / `rasq dev`)

`rasq check` は次を順に行い、どれかで止まる:

1. `cargo build --release --target wasm32-unknown-unknown`(ワークスペース全体)
2. wasm の export 検査: `snapshot` / `patch` / `call` / `alloc` / `dealloc` があること、import が `rasq.host_fn` だけであること
3. `wasmi`(`rasq_wasm_runtime_core` と同じ)で `call("plugin.manifest")` を呼び、答えが `plugins/<name>/manifest.json` と**同じ**であること
   (SNS プラグインは `function.json` を静的に見る)
4. manifest を `manifest.schema.json` で検査し、`slots` が登録簿にあり `kind` が許され、`permissions.required` を持つこと
   (validate.py の 3 と同じ規則。`rasq.toml` の `slot` と `manifest.slots` の食い違いもここ)
5. `rasq.toml` の libs / assets / models の実在と、`bundle.json` を組んだときに `bundle.schema.json` を通ること

`rasq test` = `cargo test`(ホスト関数は `rasq-plugin-kit` の `set_host_hook` でモック)。`rasq check --no-test` で分けられる。

`rasq dev`:

- debug ビルドし、rasq-client の「WASMプラグイン試験 (dev)」に入れる**絶対パス**(子ごと)を表示する。`--watch` で変更のたびに再ビルド
- 土台に流すデータ: `credentials.toml` からトークンを引き、`.rasq/dev.json` に `{ accountKey, host, tokenRef }` を書く。
  第一期の dev 画面は単体 wasm の読み込みなので、**土台のデータが要るスロット**(`post.*` / `tl` など)はホスト側が既に持つアカウントで
  動かし、トークンは SNS プラグイン自体を作るときの `sns.login` 確認に使う
- 第二期: `rasq dev --serve` で `http://<host>:7391/` にワークスペースを出し、dev 画面に「ワークスペースから読み込み」を足してホットリロード

## 6. ビルド(`rasq build`)と配布物

`dist/<root.id>-<root.version>.rasqpkg`:

```
bundle.json                 # rasq.bundle/1(schema/bundle.schema.json)。tar の最初のエントリ
manifest.json               # root のマニフェスト
<root>.wasm
plugins/<name>/manifest.json, <name>.wasm   # owned = true のとき(children)
libs/<name>.wasm
assets/…
models/<x>.model.json (+ 本体、lazy でなければ)
SHA512SUMS                  # 目次と同じ値の一覧(手で確かめる用)
```

- 圧縮は zstd(レベル 19)、tar は POSIX、パスは `bundle.schema.json` の `path` の文法(`..` / 先頭 `/` 不可)
- `dist/<…>.rasqpkg.sig.json` = `{ "alg": "ed25519", "publicKey": "<base64>", "signature": "<base64>", "digest": "sha512:<hex>" }`。
  鍵は `rasq key init` で `~/.config/rasq/dev-key.ed25519` に作る。ホストのサイドロード(WF P-23 `PluginImportPanel`)は
  この鍵で検証して「開発者鍵」の警告を出す。ストア署名は Central が付ける(D-034)ので CLI は関与しない
- `owned = false` のワークスペースで `rasq build` すると、`[[plugins]]` **ごとに** root = その子の `.rasqpkg` を出し、
  `root.requires = [{ id: base.id, version: "^<base.version>" }]` を書く。同梱はしない

ホスト側の現状: ストアのインストールは `.rasqpkg` をまだ展開せず、配布物が生の wasm(`\0asm`)ならスロット登録する(D-168)。
そのため第一期の `rasq build --raw` は子 1 つにつき `.wasm` をそのまま `dist/` に出す(今日の plugin-sample の `dist/` と同じ)。
`.rasqpkg` の展開・検証(M4 の 4 番、D-034)が入り次第 `--raw` は不要になる。

## 7. 公開(`rasq publish`)

```
rasq publish --central https://…:8080 --channel stable|beta [--notes-file NOTES.md] [--dry-run]
```

Central の管理 API(`rasq-central/src/admin/mod.rs`)に順に投げる。トークンは `RASQ_CENTRAL_TOKEN` 環境変数か
`credentials.toml` の `[central.<host>]`。

1. `PUT /api/v1/admin/plugins/{id}` … `UpsertPlugin`(`name` / `summary` / `description` / `sns` / `category` …)。
   値は `rasq.toml` の `[listing]` から。既に掲載があれば省略できる(`--skip-listing`)
2. `POST /api/v1/admin/plugins/{id}/versions` … `NewVersion`(`version` / `channel` / `manifest` / `notes` / `permissionsAdded` / `makeCurrent`)。
   `permissionsAdded` は `GET /plugins/{id}/manifest`(公開中の版)との差分から CLI が作る(D-033 の権限増加表示)
3. `PUT /api/v1/admin/plugins/{id}/versions/{version}/artifacts/bundle?kind=bundle` … `.rasqpkg` のバイト列。
   応答の `sha512` が手元と一致することを確かめる。`lazy` のモデルは `artifacts/<artifactId>?kind=model` に別送

`--dry-run` は 3 つの要求を `dist/publish/<n>-<route>.json` と `.bin` に書き出すだけ。Central を触れない作者は
これを審査側に渡す(WF p9 の「公開申請」がこの束を受け取る口になる想定 — 第二期)。

## 8. ホスト側に要る変更(第一期の CLI が前提にするもの / しないもの)

| 項目 | 第一期の CLI | ホストの状態 |
|---|---|---|
| 単体 wasm のパス読み込み | 使う | あり(D-078) |
| `.rasqpkg` の展開・検証・`bundle.json` の解釈 | `--raw` で回避 | 無し(M4 の 4 番) |
| `wasm.call`(lib / child) | 目次に書くだけ | 未配線(`host-functions.md`) |
| 添付の読み出し `storage.get({ns: "asset"})` | 目次に書くだけ | 無し(この文書で提案。`ns` を 1 つ足す形が最小) |
| 開発者鍵の検証とサイドロード警告 | `.sig.json` を出す | 無し(WF P-23) |
| dev サーバーからの読み込み | 使わない | 第二期 |

## 9. 実装計画

- `rust/rasq-cli`(bin `rasq`): `clap` / `serde` / `toml` / `jsonschema` / `wasmi` / `sha2` / `ed25519-dalek` / `tar` + `zstd` / `reqwest`(publish のみ)。
  `schema/*.json` と `templates/` は `include_dir!` で同梱し、`rasq registry` はその同梱物を出す
- `rust/rasq-plugin-kit`: plugin-sample の `kit/src/lib.rs` をそのまま移す(D-244i)。雛形は `rasq-plugin-kit = { git = "<sdk repo>", tag = "kit-0.1.0" }`
- `templates/<slot>/`: §3 の 9 つ。`{{id}}` / `{{name}}` / `{{slot}}` の置換だけ(テンプレートエンジンは使わない)
- CI(`.gitlab-ci.yml`): `rust` ジョブに `cargo run -p rasq-cli -- registry templates` と、各雛形の `new → check` を足す
- 順番: ① kit 引き上げ + 雛形 9 つ → ② `init / new / check / build --raw` → ③ `token / dev` → ④ `build`(.rasqpkg)+ `key` → ⑤ `publish`。
  ①〜② で今日の plugin-sample と同じものが CLI から作れる

## 10. 未決

- `wasm.call` の同期呼び出しでライブラリの大きな入出力(画像)をどう渡すか(base64 か線形メモリ共有か)。第二期の計測と一緒に決める
- 他人の土台に対する `requires` をホストがどう解決するか(バージョン範囲の比較と、土台の更新で子が壊れたときの縮退)
- `credentials.toml` を OS のキーチェーンに移すか(第一期はファイル 0600)
- 作者の自己申請 API(WF p9)の形。`--dry-run` の出力をそのまま受け取れる形にする
