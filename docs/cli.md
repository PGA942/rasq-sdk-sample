# `rasq` CLI — プラグイン作者向けコマンドの設計(D-244)

> 正典: スロットは [`slots.md`](slots.md) / [`../schema/slots.json`](../schema/slots.json)、マニフェストは
> [`../schema/manifest.schema.json`](../schema/manifest.schema.json)、配布物の目次は
> [`../schema/bundle.schema.json`](../schema/bundle.schema.json)(この文書で新設)。
> 流れそのものは [`../README.md`](../README.md#sdkcliの想定フローユーザー指定) の 5 段。ここはそれを
> コマンド・ディレクトリ・ファイル形式・ホスト側の前提まで落としたもの。
>
> **実装状況**: §9 の ①〜⑤(kit の引き上げ、雛形 10 個、`init / new / check / test / build [--raw] / registry`、
> `token set|list|remove` / `dev [--watch]`、`.rasqpkg` の `build` / `key init|show` / `inspect`、`publish [--dry-run]`)は
> [`../rust/rasq-cli`](../rust/rasq-cli) に実装済み(第一期は完了)。第二期の項目は §8 / §10。

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
| D-244i | 12 リポジトリに同一内容で複製されている `kit/`(`rasq-plugin-kit`)を SDK 側 `rust/rasq-plugin-kit` に引き上げ、`rasq init` がワークスペースの `kit/` へ写す(雛形は `path = "../../kit"` で依存)。plugin-sample 側の切り替えは各リポジトリの次の更新時(今回は既定ブランチを触らない) | md5 が 12 個とも同じ。契約と同じく正典を SDK に置く。git 依存にしないのはリポジトリが非公開でもネット無しでも `rasq check` が通るように |

## 1. 流れとコマンドの対応

```
   README の段        コマンド                          生成物 / 参照物
   1 土台を読む        rasq init [--base <pkg|id>]       rasq.toml, base/, Cargo.toml(ワークスペース)
                      rasq token set <accountKey>       ~/.config/rasq/credentials.toml
   2 雛形を選ぶ        rasq new <slot> <name>            plugins/<name>/ (Cargo.toml, src/lib.rs, manifest.json)
   3 コーディング      rasq add lib|asset|model <path>   libs/ assets/ models/ + rasq.toml の項目
   4 確認             rasq check / rasq test / rasq dev  wasm32 ビルド + スキーマ + manifest 往復 + cargo test / 取り込みパス
                      rasq call <name> <fn>             ビルド済みの wasm を wasmi で叩く(ホスト関数の答えを差し込める)
   5 ビルド           rasq build [--release]            dist/<id>-<version>.rasqpkg (+ .sig.json)
   (5')公開           rasq publish --central <url>      Central 管理 API への 3 要求(--dry-run で JSON)
   -                 rasq registry [slots|templates]   同梱の slots.json を表示
   -                 rasq help <言葉…> [--central url]  同梱の文書 / 登録簿から探す(+ Central の掲載)
```

## 2. ワークスペース

```
my-misskey-ext/
├── rasq.toml              # ワークスペース定義(下記)
├── Cargo.toml             # [workspace] members(CLI が rasq.toml から作り直す)
├── kit/                   # rasq-plugin-kit の写し(rasq init が置く。D-244i)
├── base/                  # 土台(読み取り専用。rasq init --base が展開。git には入れない)
│   ├── manifest.json      #   土台の function.json
│   └── misskey.wasm
├── plugins/
│   └── later/             # rasq new post.action later
│       ├── Cargo.toml     #   rasq-plugin-kit = { path = "../../kit" }、cdylib
│       ├── manifest.json  #   id = rasq.official.misskey.later (土台が自分のものの場合) / net.example.later (他人の土台)
│       └── src/lib.rs     #   plugin.manifest は manifest.json を include_str! で返す(往復検査が常に一致する)。テストは同じファイルの #[cfg(test)]
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
id = "rasq.official.misskey.later"   # rasq new が決める(D-244c)。manifest.json の id と一致していること

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

- 雛形は **登録簿の `status: stable` なスロットごと**に 1 つ。`rasq registry templates` で一覧。planned(`screen.header` など)は
  `--allow-planned` を付けたときだけ出せ、`rasq check` が警告を出す(validate.py と同じ扱い)
- 雛形の中身は plugin-sample の各リポジトリの**最小化版**(manifest を返す `plugin.manifest`、そのスロットの `call` を
  1 つ、`fow.open` が要るスロットは空の FOW)。フル機能の例としては plugin-sample を README から指す
- 生成される `manifest.json` は `manifest.schema.json` v0.3 に合う形で、`id` は §0 D-244c の規則で決める
  (`owned = true` → `<base.id>.<name>`、そうでなければ `<namespace>.<name>`)、`slots = [<slot>]`、`permissions` は
  登録簿の `permissions.required` をそのまま入れる
- **`rasq new assets <name>`** はスロットではなく**アセットパック**(`kind: "assets"`、wasm 無し、[slots.md §2.10](slots.md))を `packs/<name>/` に作り、rasq.toml に `[[packs]]`(`name` / `id`)を足す。id は土台に依らず常に `<namespace>.<name>`。`rasq check` / `build` / `publish` はコードのプラグインと一緒に扱う(パックだけのワークスペースなら Rust のビルドは走らない)
- **`rasq new theme <name>`** は**着せ替え(テーマ)パック**(`kind: "theme"`、wasm 無し、[slots.md §2.12](slots.md))を同じく `packs/<name>/` に作る(`manifest.json` の `theme`(色 / アイコンの差し替え / 面ごとの背景)+ `theme/` の画像 1 枚)。rasq.toml の `[[packs]]` もアセットパックと共通で、kind は `manifest.json` が持つ。`rasq check` はアセットパックと同じ検査に加えて参照の整合(`theme.surfaces.*.layers[].asset` と `theme.icons` の `asset:<id>` が `assets` に `use: "theme"` であること)を見て、どこからも指されていない画像は警告。`rasq build` は kind theme の配布物(wasm 無し)、`rasq publish` は掲載のカテゴリを `theme` にする
- 生成直後に `rasq check` が通る状態であること(雛形の CI: SDK 側 `rust` ジョブで全雛形を `rasq new` → `check`)

雛形の対応表(第一期、登録簿の stable 10 スロット。`page` は v0.18):

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
| `page` | shared のカウンター(`page.open` → `shared.op incr` → `page.event var` → `page.render`、セッション無しは端末だけ) | custom-page-plugin |

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

- ビルドの置き場は cargo と同じ決め方: `CARGO_TARGET_DIR` があればそこ(相対ならワークスペースからの相対)、無ければ `target/`。
  CI や複数リポジトリの共有ビルドでリポジトリの外に置いても、`rasq check` / `build` / `call` / `dev` は同じ場所を見る。
- `rasq init` が置く `.gitignore` は `/target` `/base` `/.rasq` `*.sig.json`。`dist/` は無視しない(配布物をリポジトリに残す流儀 =
  plugin-sample。要らなければ足す)。署名 `.sig.json` は開発者の鍵ごとのものなので、リポジトリに入れない。
- `rasq init --namespace <ns>` の名前空間は 1 ラベル(`sample`)でも 2 ラベル以上(`net.example`)でもよい(`<ns>.<name>` が pluginId)。

`rasq call <name> <fn> [--args JSON|@file] [--reply FN=JSON]… [--then FN[=JSON|@file]]… [--full]`(実装済み)— **ビルド済みの wasm そのもの**を
wasmi で動かして `call` を叩く。`cargo test` は native で走るので、wasm32 でだけ起きる問題(下の落とし穴)はここでしか見つからない。

- `--args` の値の中の `"@b64:<path>"` はそのファイルの base64 に置き換わる(画像を渡す用)
- ホスト関数は `--reply 'fow.open={"opened":true}'` のように答えを差し込める。無い fn は `E01`(ホスト無し)。呼ばれた
  ホスト関数は `host <fn> <args>` として出る。戻りが `{patch: true}` なら続けて `patch()` の中身も出す
- `--then rmqr.0 --then link.open` のように、同じインスタンスで続けて呼べる(状態を持つプラグインの一連の操作)。
  `--then 'astell.event={"token":"t","type":"share"}'` / `--then media.commit=@args.json` で、続きの call にも args を渡せる(`@b64:` も効く)

```sh
rasq call rmqr viewer.open --args '{"token":"v1","mimeType":"image/png","width":344,"height":104,"bytesB64":"@b64:plugins/rmqr/tests/fixtures/rmqr_url.png"}' \
  --reply 'fow.open={"opened":true}' --then rmqr.0 --then link.open --then viewer.close
```

#### 落とし穴: native テストは通るのに wasm でトラップする

雛形の release プロファイルは `panic = "abort"` なので、wasm の中の panic は `wasm unreachable instruction executed` になる。
`wasm32-unknown-unknown` には OS が無く、次のものは**コンパイルは通るが実行時に panic する**:

| 呼ぶと panic するもの | よくある入り口 | 代わり |
|---|---|---|
| 時刻(`SystemTime::now` / `Instant::now` / `chrono::Utc::now`) | 結果に timestamp を付けるライブラリ(例: `rxing` の Reader が作る `RXingResult`) | 時刻を使わない下位 API を直接呼ぶ。時刻が要るなら呼び出し元(ホストの args)からもらう |
| 乱数(`getrandom` の既定) | `rand` / `uuid` / ハッシュの種 | 種を args / `storage` からもらう、決定的な生成にする |
| スレッド / ファイル / ソケット | `rayon`、`std::fs`、`std::net` | 単一スレッド、`storage.*` / `net.fetch`(ホスト関数) |

クレートの「wasm 対応」feature(`wasmbind` / `js` / `wasm_support` など)は js-sys の import を足すものが多く、RASQ のホストは
`rasq.host_fn` 以外の import を満たせない(`rasq check` が「許されない import」で止める)。
実例: rMQR の検出プラグイン(`rxing` 0.9)は `cargo test` 12 件が通った状態で `viewer.open` がトラップし、`rasq call` で見つけて
`FindFinderPatterns` → `SampleRMQR` → `Decode` の下位 API に切り替えた。

`rasq dev [--watch] [--debug]`(実装済み):

- ビルド(既定 release: wasm はインタープリタで動くので debug は数 MB になり遅い。`--debug` で切り替え)→ `rasq check` と同じ往復検査
  → rasq-client の「WASMプラグイン試験 (dev)」に入れる**絶対パス**(sns と子ごと)を表示する。`--watch` は sns/ / plugins/ / kit/ / rasq.toml の
  mtime を 1 秒ごとに見て、変わったら再ビルドして出し直す(Ctrl-C で止める。依存を増やさないためポーリング)
- 土台に流すデータ: `rasq.toml` の `account`(`[base]` か `[sns]`)から `credentials.toml` を引き、`.rasq/dev.json`(`rasq.dev/1`)に
  `{account, host, tokenRef: "credentials:<accountKey>", base, sns, plugins: [{id, slot, wasm}]}` を書く。トークンそのものは書かない。
  第一期の dev 画面は単体 wasm の読み込みなので、**土台のデータが要るスロット**(`post.*` / `tl` など)はホスト側が既に持つアカウントで
  動かし、トークンは SNS プラグイン自体を作るときの `sns.login` 確認に使う
- 第二期: `rasq dev --serve` で `http://<host>:7391/` にワークスペースを出し、dev 画面に「ワークスペースから読み込み」(= `dev.json` の読み込み)を足してホットリロード
- 試験画面で全機能を試すには、署名した .rasqpkg を入れる(rasq build)。rasq dev の生の wasm は制限モード
  ([PROTOCOL §6.1](../PROTOCOL.md)。`net.fetch` / `gpu.compute` / `storage.unsafe` などが `E01`)

`rasq token`(実装済み、D-244e):

- `rasq token set <kind:instance:handle> [--from-env VAR] [--host HOST] [--no-workspace]` — 端末なら非表示入力、パイプなら標準入力の 1 行、
  `--from-env` なら環境変数。**argv では受け取らない**(シェル履歴に残るため)。`~/.config/rasq/credentials.toml`(ディレクトリ 0700 / ファイル 0600、
  `RASQ_CREDENTIALS` で場所を変えられる)に `[accounts."<accountKey>"] host / token / updated_at` として保存し、ワークスペース内なら
  `rasq.toml` の `[base].account` か `[sns].account` に accountKey だけ書く。`--host` はテスト用サーバーに向けるとき(既定は accountKey の instance)
- `rasq token list` — accountKey / host / 更新時刻と、トークンの先頭 4 文字だけ
- `rasq token remove <accountKey>`
- Central のトークン(`[central."<host>"]`)は同じファイルに置く想定(⑤ publish で使う)

### ボタンの共通属性の検査(v0.14)

`rasq check` は `manifest.actions[]` の `auto`(面が作られた瞬間の初期状態 = temp)と `placement`(auto / manual)を
[slots.md §2.11](slots.md) の規則で見る: `auto: true` × `destructive` / `autoHookForbidden` は error、post.action の
`auto: true` に権限 `auto_hook` が無ければ error、composer.media の `auto` は `output: "filter"` だけ、`placement: manual` で
`tags` が無ければ warning、`auto_hook` を持つのに `auto: true` の post.action が無ければ warning(過剰宣言)。

## 5.5 文書を探す(`rasq help`)

```
rasq help                         同梱の文書の一覧
rasq help net.fetch               関数名 / スロット名 / 属性名 → その節(登録簿の hostFns、PROTOCOL、slots.md)
rasq help auto                    manifest.actions[].auto の説明(スキーマの description)
rasq help 全画面 初期状態 -n 3      言葉で探す(見出しの一致 > 本文の語の一致 > 文字 2-gram の Dice)
rasq help 制限モード -n 2            サイドロードの全機能モード / 制限モード(PROTOCOL §6.1、slots.md §5.1 …)
rasq help --full post.action      節を省略せずに出す
rasq help --central <url> QR       Central の掲載(ほかのプラグインの名前 / 概要)も探す。トークン不要
rasq help --central <url> --plugin sample.media-fullscreen   そのプラグインの説明と権限の理由
```

探し方は rasq-client の RAi リファレンスと同じ考え(語の一致を最優先、それ以外は 2-gram の Dice)。埋め込みモデルは使わない
(CLI はネット無し・軽量で動くことを優先)。文書は `docs/*.md` と `PROTOCOL.md` を見出し(`#`〜`###`)ごとに、登録簿はスロットごとに、
マニフェストのボタン属性は 1 つずつ節にしている。

## 6. ビルド(`rasq build`)と配布物(実装済み)

`rasq build [--raw] [--no-sign] [--no-build] [--with-source]` は release の wasm32 ビルド → `rasq check` と同じ往復検査(エラーがあれば止まる)→
`dist/<root.id>-<root.version>.rasqpkg`(+ `.sig.json`)。`--raw` を付けると生の `.wasm` も `dist/` に出す(今のホストのパス読み込み用)。

試験画面で全機能を試すには、署名した .rasqpkg を入れる(rasq build)。rasq dev の生の wasm は制限モード。全機能になるのは、
自分の RASQ アカウントに登録した鍵(Web コンソールの `/console/keys`)で署名したものだけ([PROTOCOL §6.1](../PROTOCOL.md))。
`--raw` の `.wasm` / `--no-sign` の `.rasqpkg` も制限モード。
`rasq inspect <x.rasqpkg> [--extract DIR]` が目次と中身(sha512 / sizeBytes / `SHA512SUMS` / 余分なファイル)と `.sig.json` を確かめる。
目次と中身の食い違い、または `.sig.json` があって署名が合わないときは**終了コード 1**(表示は同じ)。`.sig.json` が無いのは 0
(開発者鍵の署名は任意)。署名が合っても、公開鍵は `.sig.json` 自身に書かれた**自己申告**なので、`inspect` は
「自己申告の開発者鍵: 指紋 …」と出すだけで「誰の鍵か」は保証しない — 指紋を開発者が登録している鍵(Central の掲載 /
開発者本人の告知)と見比べること。

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

- 圧縮は zstd(レベル 19、フレームのチェックサム付きなので 1 バイトの破損も `Data corruption detected` で止まる)、tar は GNU、
  パスは `bundle.schema.json` の `path` の文法(`..` / 先頭 `/` / 先頭 `.` 不可)。`bundle.json` は組む前に `bundle.schema.json` で検査する
- `dist/<…>.rasqpkg.sig.json` = `{ "alg": "ed25519", "publicKey": "<base64>", "signature": "<base64>", "digest": "sha512:<hex>" }`。
  鍵は `rasq key init` で `~/.config/rasq/dev-key.toml`(0600、`RASQ_DEV_KEY` で場所を変えられる)に作り、`rasq key show` が公開鍵と
  指紋(公開鍵の SHA-512 先頭 8 バイト)を出す。署名対象は配布物バイト列の SHA-512 ダイジェスト(生 64 バイト)で、ホストの
  `verifyArtifact` と同じ。鍵が無ければ `build` は署名せずに注意だけ出す(`--no-sign` で黙る)。ホストのサイドロード(WF P-23
  `PluginImportPanel`)はこの鍵で検証して「開発者鍵」の警告を出す。ストア署名は Central が付ける(D-034)ので CLI は関与しない
- `[sns]` のワークスペースは root = sns、`[base] owned = true` は root = `base/` の土台で、どちらも `[[plugins]]` を `children` に同梱する
  (子の id が `<root>.` で始まらなければ止まる)。`owned = false`(他人の土台)や土台無しでは `[[plugins]]` **ごとに** root = その子の
  `.rasqpkg` を出し、土台があれば `root.requires = [{ id: base.id, version: "^<base.version>" }]` を書く。同梱はしない
- `[[libs]]` は wasm の export が `exports` にあることを wasmi で確かめて `libs/<file>` に、`[[assets]]` はそのパスのまま、`[[models]]` は
  manifest と(`lazy = false` なら)`runtimes.*.file` / `tokenizer.file` の本体を入れる。`lazy = true` は manifest だけ + `artifactId = model-<name>`。
  `rasq inspect` は、model manifest の `runtimes.*.file` / `tokenizer.file` を「目次に載っているもの」として数える(lazy でないのに無ければ問題、それ以外の目次に無いファイルは問題)
- `rasq init --base <x.rasqpkg>` は `inspect` と同じ検査を通してから root(`kind = sns` のみ)の manifest / wasm / `bundle.json` を `base/` に置く
- 他人の `.rasqpkg` を読む(`inspect` / `init --base`)ときの展開の上限(zstd 爆弾の対策、`pkg::ExtractLimits::DEFAULT`):
  展開後の合計 **1 GiB**、1 ファイル **512 MiB**(同梱のモデル本体を見込んだ値)、エントリ **4096 個**。tar の申告の
  大きさで読む前に切り、zstd を解いた流れそのものも合計 + 64 MiB で切る。超えたら展開せずにエラー

### 権限の理由(`[listing.safety]`)

```toml
[listing.safety]
external_hosts = []                       # 外部通信先(人向けの申告。マニフェストの network.hosts とは別)
data_handling = "データは端末の中だけで扱い、外部には送信しません。"

[[listing.safety.permissions]]
permission = "storage.safe"
reason = "最近使ったハッシュタグを端末に保存して、候補の先頭に出すため"
```

`rasq build` はこれを目次(`bundle.json`)の `root.safety` に入れる。別ファイルにしないのは、目次に無いファイルを
拒否する既存の検査(アプリのインストール、`rasq inspect`)を壊さないためで、目次は開発者の署名の範囲に入る。
Central はどの経路(Web コンソール、`/dev/plugins/publish`、`rasq publish` の管理 API)で受け取っても掲載の
`safety` に反映する。審査(rasq-judge の 1 段目)は**宣言した権限すべてに理由があるか**を見るので、無いと自動では
承認されない。`rasq check` が同じ基準で警告する(理由の無い権限 / 宣言していない権限の理由)。

### 審査用のソース(`--with-source`)

`rasq build --with-source` は配布物と別に `dist/<root.id>-<root.version>.source.tar.zst` を出す。中身はワークスペースのうち
自分で書いた部分 — `rasq.toml` / `Cargo.toml` / `Cargo.lock` と、ビルド対象のディレクトリ(`sns` / `plugins/<name>`、`base/` は
`owned = true` のとき、`[[libs]]` のディレクトリ)— で、`target/` / `dist/` / `.git` / 隠しファイル / symlink は入らない
(合計 32 MiB、1 ファイル 4 MiB、4000 ファイルまで)。先頭の `SOURCE.json`(`rasq.source/1`)に、どの wasm のソースか
(`wasm[].sha512` = 配布物の目次と同じ値)、ツールチェーン(`rustc --version` / `cargo --version`)、ビルドコマンド、
ファイル一覧とそのハッシュ(`treeSha512`)を書く。

**配布はされない。** `rasq publish --with-source` が `PUT …/artifacts/source?kind=source` で送り、Central は公開の
manifest / artifacts から除いて、審査(rasq-judge の 3 段目 = 大きな更新の深い読み)だけが読む。審査から求められる
(Web コンソールに「ビルドコマンドの変更のお知らせ」が出る / `GET /dev/plugins` の `sourceRequired`)と、以後の版は
これが無いと自動承認されない。開発者トークンで後から付けるなら `POST /api/v1/dev/plugins/{id}/versions/{v}/source`
(本文 = tar.zst。審査を通った版には付けられない)。

再現ビルドの検証はしていない: ソースとバイナリの一致は開発者の申告で、審査は wasm から拾った文字列の差分を裏取りに使う。
ツールチェーンとコマンドを記録しておくのは、後から再現ビルドで検証する道を残すため。

ホスト側の現状: ストアのインストールは `.rasqpkg` をまだ展開せず、配布物が生の wasm(`\0asm`)ならスロット登録する(D-168)。
そのため `rasq build --raw` は子 1 つにつき `.wasm` もそのまま `dist/` に出す(今日の plugin-sample の `dist/` と同じ)。
`.rasqpkg` の展開・検証(M4 の 4 番、D-034)が入り次第 `--raw` は不要になる。ホスト側の展開は `rasq inspect` と同じ手順
(zstd → tar → 先頭 `bundle.json` → スキーマ → sha512 / sizeBytes → `SHA512SUMS` → `.sig.json`)で書ける。

## 7. 公開(`rasq publish`、実装済み)

```
rasq publish --central http://100.91.218.56:8080 [--channel stable|beta] [--notes-file NOTES.md] [--dry-run] [--skip-listing] [--no-make-current] [--no-build] [--with-source]
```

既定では `rasq build` を通してから(`--no-build` で `dist/` のものをそのまま)、配布物ごとに Central の管理 API
(`rasq-central/src/admin/mod.rs`)へ順に投げる。トークンは `RASQ_CENTRAL_TOKEN` 環境変数(`--token-env` で変更)か
`credentials.toml` の `[central."<host[:port]>"]`(`rasq token set --central <host>` で保存)。`Authorization: Bearer <token>`。

0. `GET /api/v1/plugins/{id}/manifest`(公開中の版)… `permissionsAdded` の差分の元。404 / 403(未掲載・審査中)なら新規扱い
1. `PUT /api/v1/admin/plugins/{id}` … `UpsertPlugin`(`name` / `summary` / `description` / `sns` / `category` / `iconUrl` / `hasAds` / `price` / `developer`)。
   値は `rasq.toml` の `[listing]` から、無ければ root の manifest(`name` / `description` / `supportedSns`)。`origin` / `status` は
   送らないので Central の既定(`community` / `in_review` = 審査待ち)。既に掲載があれば `--skip-listing`
2. `POST /api/v1/admin/plugins/{id}/versions` … `NewVersion`(`version` / `channel` / `manifest` / `notes` / `permissionsAdded` / `makeCurrent`)。
   `manifest` は `.rasqpkg` の中の `manifest.json`(Central が id / version をパスと突き合わせる)
3. `PUT /api/v1/admin/plugins/{id}/versions/{version}/artifacts/bundle?kind=bundle`(`Content-Type: application/zstd`)… `.rasqpkg` のバイト列。
   応答の `artifact.sha512` が手元と一致することを確かめ、ストア署名と `storePublicKey` を表示する
4. `lazy` のモデルごとに `PUT …/artifacts/<artifactId>?kind=model&lazy=true`(`runtimes.*.file` の本体。手元に無ければ飛ばして案内)
5. `--with-source` なら `PUT …/artifacts/source?kind=source`(`Content-Type: application/zstd`)… `dist/<…>.source.tar.zst`(§6。配布されない)

`--dry-run` は要求を `dist/publish/<pkg>/NN-<label>.json`(+ 配布物は `.bin` と `bodySha512`)に書き出すだけ(トークン不要、
`Authorization` は `Bearer $RASQ_CENTRAL_TOKEN` のまま)。Central を触れない作者はこれを審査側に渡す(WF p9 の「公開申請」が
この束を受け取る口になる想定 — 第二期)。

手元の Central(`http://100.91.218.56:8080`、管理トークン)で `net.example.later` / `net.example.bylen` を投入して、掲載 → 版 →
配布物(sha512 一致、ストア署名付き)まで通ることを確認済み。

## 8. ホスト側に要る変更(第一期の CLI が前提にするもの / しないもの)

| 項目 | 第一期の CLI | ホストの状態 |
|---|---|---|
| 単体 wasm のパス読み込み | 使う | あり(D-078) |
| `.rasqpkg` の展開・検証・`bundle.json` の解釈 | 出す(`--raw` で生 wasm も) | **あり**(rasq-client `lib/features/store/rasqpkg.dart`。zstd はランタイム内蔵の `rasqZstdDecompress` = 純 Rust の ruzstd、tar 以降は Dart。`rasq inspect` と同じ手順で、ストアのインストール / 再起動時の取り直し / dev 画面のサイドロードが通る) |
| `wasm.call`(lib / child) | 目次に書くだけ | 未配線(`host-functions.md`) |
| 添付の読み出し `storage.get({ns: "asset"})` | 目次に書くだけ | 無し(この文書で提案。`ns` を 1 つ足す形が最小) |
| 開発者鍵の検証とサイドロード警告 | `.sig.json` を出す(`rasq key`) | dev 画面の `.rasqpkg` 取り込みで検証して指紋を表示(WF P-23 の専用パネルは未) |
| dev サーバーからの読み込み | 使わない | 第二期 |

## 9. 実装計画

- `rust/rasq-cli`(bin `rasq`): `clap` / `serde` / `toml` / `jsonschema` / `wasmi` / `sha2` / `ed25519-dalek` + `getrandom` / `tar` + `zstd` / `rpassword` / `ureq`(rustls、publish のみ)。
  `schema/*.json` と `templates/` は `include_dir!` で同梱し、`rasq registry` はその同梱物を出す
- `rust/rasq-plugin-kit`: plugin-sample の `kit/src/lib.rs` をそのまま移す(D-244i)。`rasq init` がワークスペースの `kit/` に写し、雛形は `path = "../../kit"`
- `templates/<slot>/`: §3 の 9 つ + `sns` + `assets` / `theme`(wasm 無しのパック)。`{{id}}` / `{{name}}` / `{{display}}` / `{{slot}}` / `{{kit_path}}` の置換だけ(テンプレートエンジンは使わない)
- CI(`.gitlab-ci.yml`): `rust` ジョブで `rasq-cli` を組み、全雛形を `new → check` する
- 順番: ✅ ① kit 引き上げ + 雛形 → ✅ ② `init / new / check / build --raw` → ✅ ③ `token / dev` → ✅ ④ `build`(.rasqpkg)+ `key` + `inspect` → ✅ ⑤ `publish`(第一期完了)。
  ①〜② で今日の plugin-sample と同じものが CLI から作れる

## 10. 未決

- `wasm.call` の同期呼び出しでライブラリの大きな入出力(画像)をどう渡すか(base64 か線形メモリ共有か)。第二期の計測と一緒に決める
- 他人の土台に対する `requires` をホストがどう解決するか(バージョン範囲の比較と、土台の更新で子が壊れたときの縮退)
- `credentials.toml` を OS のキーチェーンに移すか(第一期はファイル 0600。`rpassword` で非表示入力)
- 作者の自己申請 API(WF p9)の形。`--dry-run` の出力をそのまま受け取れる形にする
