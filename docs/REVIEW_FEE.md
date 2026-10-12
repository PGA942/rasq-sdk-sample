# 審査料の額の決め方(ピース数の計測方法 `xlmr-unigram-v1`)

プラグインの掲載・新しい版の審査を申し込むときの**審査料**(規約案 C「プラグイン開発者の掲載・審査条件」第4条の2)は、
申し込む版の**配布物(`.rspkg`)の中のファイルの内容**を、この文書の方法で数えた**ピース数**から、運営(ORGATE-Server)の式で決まる。
配布物のバイト数は目安にすぎない(同じ大きさでも、内容によってピース数 = 額は変わる)。

開発者は **`rasq fee <配布物>`** で、Central が申込みのときに数えるのと**同じ数と額**を手元で出せる(§6)。
Central と `rasq fee` は同じクレート [`rust/rasq-review-pieces`](../rust/rasq-review-pieces/)(Central はその写しを CI で突き合わせる)を使う。
Rust に依らない参照実装は [`rust/rasq-review-pieces/tool/reference_pieces.py`](../rust/rasq-review-pieces/tool/reference_pieces.py)。

この文書の版: **`xlmr-unigram-v1`**(2026-10-01)。数え方・語彙を変えるときは版の名前を変え、変えた日からの申込みに使う
(それまでの申込みの額は変わらない)。申込みごとに、どの版で数えたかが API の `feeBasis.pieceMethod` に残る(§5)。

## 1. 数える対象

1. 配布物 = `rasq build` が出す `.rspkg`(**zstd** で圧縮した **tar**。[cli.md §6](cli.md))。zstd を解いて tar として読む。
2. tar の**通常ファイル**(typeflag が `0` または NUL)を**全部**、tar の順に数える。`bundle.json`・マニフェスト・wasm・アセット・
   モデル・`SHA512SUMS` など、名前や種類で除くものは無い。
3. 数えないもの: tar の見出し(512 バイトのヘッダ、GNU の長い名前 `L`/`K`、pax の `x`/`g`)・ディレクトリ・シンボリックリンク・
   ハードリンク・そのほかの種類のエントリ。ファイルの名前(パス)も数えない(中身だけ)。
4. 展開の上限(超えた配布物は数えずに断る。Central は申込みの前に `400` で断り、審査料はかからない):

   | 上限 | 値 | 説明 |
   |---|---|---|
   | 展開後の合計 | 256 MiB | Central の版を出す口と同じ |
   | 通常ファイルの数 | 2,000 | Central の版を出す口と同じ |
   | ファイル 1 つ | 256 MiB | tar の見出しの大きさで、読む前にも断る |
   | 圧縮率 | 1,000 倍 | 展開後が 64 MiB を超え、配布物(圧縮)の 1,000 倍より大きく展開する配布物は zstd 爆弾として断る。ふつうの配布物(数倍〜数十倍)には掛からない |

   上限の内の配布物の数は、上限を足す前(`rasq-review-pieces` 1.0.0 のはじめ)と変わらない。
5. ファイルごとに数えて(§2〜§3)、和をとる。ファイルの順は和に関係しない。

## 2. バイト列 → 文字列 → 語

1. ファイルのバイト列を **UTF-8** として読む。正しくない部分は **U+FFFD** に置き換える。置き換え方は Rust の
   `String::from_utf8_lossy`(= Python の `bytes.decode("utf-8", errors="replace")`。Unicode の「最大の部分列ごとに 1 つ」)。
2. **正規化はしない**(NFKC・SentencePiece の Precompiled の文字の置き換え・小文字化・空白の畳み込みのどれもしない)。
3. **空白**で区切って語にする。空白の集合は Python の `str.isspace()` と同じ = Unicode の `White_Space` に、
   U+001C〜U+001F(区切り文字)を足したもの(Rust では `char::is_whitespace(c) || ('\u{1c}'..='\u{1f}').contains(&c)`)。
   空の語(空白が続くところ)は捨てる。NUL(U+0000)などの制御文字は空白ではない(語の一部)。
4. 長さの上限・切り詰め・最大の長さは**無い**(どんなに長い語も、全部を数える)。
5. 数え方の実装は**流しながら**(`rasq-review-pieces` の `PieceStream`、セキュリティ確認 2026-10-02 #21)。ファイルを 64 KiB ずつ読み、
   塊の境で切れた UTF-8 の文字は次の塊へ持ち越す。§3 の Viterbi は、位置 i から伸ばせるのが最長のピース(16 文字)までなので、
   `best` / `count` を 17 個の輪、先読みを 16 文字だけ持てば足りる。語の長さやファイルの大きさにかかわらず、作業メモリは一定
   (語彙の木を除いて数 KiB)。数は、ファイル全体を一度に文字列にして数えた場合と**同じ**(区切り方を変えて突き合わせるテストがある)。

## 3. 語 → ピース(Unigram の Viterbi)

1. 語 `w` ごとに、先頭に **`▁`(U+2581)** を 1 つ付けた文字列 `s = "▁" + w` を作る。
2. `s` をピースの並びに切る。切り方の点数 = 使ったピースの **score の和**。**和が最大**になる切り方を選ぶ(動的計画法)。
   - ピースは語彙(§4)の文字列。語彙の score をそのまま使う(64 bit 浮動小数点で足す)。
   - 語彙に無い **1 文字**は、score **−30** の 1 ピースとして使える(2 文字以上の語彙に無い並びは使えない)。
   - **同点**のときは、先に見つかった切り方を残す: 位置 `i` を前から順に、各 `i` で長さ 1, 2, … の順にピースを試し、
     `best[i] + score > best[i + 長さ]` のとき(**厳密に大きいとき**だけ)更新する。
3. 語のピース数 = 選んだ切り方のピースの数。ファイルのピース数 = 語のピース数の和。

## 4. 語彙(トークナイザーの同定)

| 項目 | 値 |
|---|---|
| 計測方法の版 | `xlmr-unigram-v1` |
| もとのモデル | [`sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2`](https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2) の `tokenizer.json`(XLM-RoBERTa の SentencePiece **Unigram**、250,002 個)。Apache-2.0(もとの XLM-R の語彙は MIT) |
| もとの `tokenizer.json` の SHA-256 | `cad551d5600a84242d0973327029452a1e3672ba6313c2a3c3d69c4310e12719` |
| 語彙 | `model.vocab`(`[piece, score]` の並び)から、特別なトークン `<s>` `<pad>` `</s>` `<unk>` `<mask>` の 5 つを除いた **249,997 個**(いちばん長いピースは 16 文字) |
| 正の語彙ファイル | `rust/rasq-review-pieces/assets/xlmr-unigram-vocab.json.zst`(`{"source", "excluded", "vocab": [[piece, score], …]}` を zstd -19) |
| 語彙ファイル(.zst)の SHA-256 | `c1da5503a926fa72e2d7253fc462dfa750927481b101efb95b3c76b51dd9db49` |
| 語彙ファイルを解いた JSON の SHA-256 | `4d3556e2073785a9706911a4351eaa36bbc3853a5dab4807feb0deba4656f44a` |
| 作り方 | `tool/reference_pieces.py --make-vocab tokenizer.json OUT.json.zst` |

語彙はバイナリに**埋め込む**(Central も `rasq fee` も)。端末内モデルの置き場の `tokenizer.json` は読まない — モデルを差し替えても
値段が黙って変わらないように。クレートのテストが、埋め込みの語彙の SHA-256 と件数、(もとの `tokenizer.json` があれば)その一致を確かめる。

## 5. 額の式

式の正は **ORGATE-Server**(`src/payments/fee.rs`)。定数は運営が環境変数(`ORGATE_REVIEW_FEE_*`)で変えられ、いまの値は
ORGATE の公開の `GET /api/v1/payments/config` の `reviewFee.basis` / `reviewFee.formula` に出る。既定(ユーザー決定 2026-10-01):

```text
raw     = baseJpy × (ピース数 ÷ divisorPieces) ^ exponent        … 64 bit 浮動小数点
rounded = round(raw ÷ roundToJpy) × roundToJpy                   … round は 0.5 を 0 から遠い方へ(額は正なので 0.5 は切り上げ)
額(円・税込)= min(1,000,000, max(minimumJpy, min(1,000,000, rounded)))

既定: baseJpy = 100、divisorPieces = 1,876,000、exponent = 1.21、minimumJpy = 150、roundToJpy = 10
    = max(150, 10 円単位に四捨五入(100 × (ピース数 ÷ 1,876,000) ^ 1.21))
上限: 1,000,000 円(式が壊れた値を出しても、桁違いの請求をしない)
```

目安(**内容で変わる**): wasm はおよそ 93 万ピース / MB、Rust のソースはおよそ 37.5 万ピース / MB。

| ピース数 | 額 | 目安 |
|---:|---:|---|
| 0〜2,694,832 | ¥150 | 小さなプラグイン(最低額。wasm 2.9 MB ほどまで) |
| 4,650,000 | ¥300 | wasm 5 MB ほど |
| 9,300,000 | ¥690 | wasm 10 MB ほど |
| 18,600,000 | ¥1,610 | wasm 20 MB ほど |
| 46,500,000 | ¥4,860 | wasm 50 MB ほど |

### 申込みに残るもの(API)

版を出すと(Web コンソール / `rasq submit` / `POST /api/v1/dev/plugins/publish`)、Central が数えて ORGATE に額を聞き、申込みに残す。
`GET /api/v1/dev/review-submissions` と `POST /dev/plugins/publish` の応答の `submission` に出る:

| 欄 | 中身 |
|---|---|
| `feePieces` | ピース数(この文書の方法) |
| `packageBytes` / `unpackedBytes` | `.rspkg` のバイト数 / 数えたファイルの合計のバイト数 |
| `amountJpy` | 額(税込) |
| `feeFormula` | そのときの式の文(例 `max(150, 10 円単位に四捨五入(100 × (ピース数 ÷ 1,876,000) ^ 1.21))`) |
| `feeBasis` | `{baseJpy, divisorPieces, exponent, minimumJpy, roundToJpy, pieceMethod: "xlmr-unigram-v1", vocabSha256: "c1da55…"}` |

式が支払いまでに変わったら、Central は払わせずに見積もり直して見せ直す(`409 fee_changed`)。

### 5.1 サーバー変数の上限を広げる宣言(`sharedLimits`、契約 v0.22)

ページ型の shared がマニフェストに **`page.sharedLimits`**(値 1 つ 16 KiB / 1 セッション 1 MiB を超えて使う、[slots.md §2.13](slots.md))を
書いた版は、**強化審査**の対象になる(運営の保存と転送の負担が増えるため、使い方と量の妥当さを見る)。

- **リリース版からは、宣言した版の審査料を割り増す**(式の上に倍率を掛ける。倍率は運営が決めて `payments/config` に出す。強めに上げる予定)。
- **β では割り増さない**(審査だけ)。
- 申込みの `feeBasis` に **`sharedLimits`**(宣言の有無、`true` / `false`)を残す(Central S-106)。割り増しを始めたら、掛けた倍率も残す。
- `rasq check` は宣言に警告を出し(強化審査・リリース版の割り増し)、`rasq fee` は宣言のある配布物にその旨を出す(額は β の式のまま)。

## 6. 手元で数える(`rasq fee`)

```sh
rasq fee dist/net.example.hello-1.0.0.rspkg          # ファイルごとのピース数・合計・既定の式の額
rasq fee dist/                                          # dist/ の .rspkg / .rasqpkg 全部
rasq fee extracted/                                     # 展開した中身のディレクトリ(rasq inspect --extract の出力など)
rasq fee dist/x.rspkg --orgate https://orgate.example  # そのサーバーの式の定数(公開の payments/config)で額を出す
rasq fee dist/x.rspkg --json                          # {path, method, packageBytes, unpackedBytes, pieces, amountJpy, basis, files[]}
```

ネットは使わない(`--orgate` のときだけ ORGATE の公開の設定を読む)。ディレクトリは、中に `.rspkg` があればそれぞれを、
無ければ中の通常ファイル(再帰、シンボリックリンクはたどらない)を数える。同じファイルなら配布物と同じ数になる。

## 7. 確かめ方(期待値)

`rust/rasq-review-pieces/tests/fixtures/expected.json` が、参照実装(Python)で出した期待値:
ファイル 4 つ(日本語と英語の文・Rust のソース・乱数・wasm の先頭)、配布物 1 つ(`sample.rasqpkg`: 通常ファイル 4 つ +
ディレクトリ + シンボリックリンク → 64,359 ピース・¥150)、式の表。`cargo test -p rasq-review-pieces`(SDK)と
rasq-central の `cargo test`(写し)と `rasq-cli` のテスト(`rasq fee`)が、同じ期待値を確かめる。

```sh
python3 rust/rasq-review-pieces/tool/reference_pieces.py --vocab rust/rasq-review-pieces/assets/xlmr-unigram-vocab.json.zst \
  rust/rasq-review-pieces/tests/fixtures/sample.rasqpkg
```
