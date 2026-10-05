# rasq-review-pieces

審査料のピース数(計測方法 `xlmr-unigram-v1`)。仕様は [docs/REVIEW_FEE.md](../../docs/REVIEW_FEE.md)。

- `count_package(&[u8])` … 配布物(`.rspkg` / 旧来の `.rasqpkg`)の中の通常ファイルを、流しながら数える(Central が申込みのときに使う。上限は `Limits::DEFAULT`、作業メモリは一定)。
- `PieceCounter::embedded().count_bytes(&[u8])` … バイト列 1 つ。
- `PieceCounter::embedded().stream()` … 塊ごとに `feed` して `finish`(`PieceStream`。区切り方によらず `count_bytes` と同じ数)。
- `fee::FeeBasis::default().amount_jpy(pieces)` … 額の式の写し(正は ORGATE-Server `src/payments/fee.rs`)。
- `tool/reference_pieces.py` … Rust に依らない参照実装(期待値 `tests/fixtures/expected.json` を作る)。

**rasq-central は `crates/rasq-review-pieces/` にこのディレクトリをそのまま写して使う**(CI の `review-pieces-drift` が `diff -r` で
確かめる)。変えるときは、ここ(SDK)を先に変えてマージし、Central へ写す。数え方・語彙を変えると値段が変わるので、`METHOD` の版を上げる。

## ライセンス

コードは MIT。`assets/xlmr-unigram-vocab.json.zst` は
[sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2](https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2)
(Apache-2.0)の `tokenizer.json` の語彙と score を取り出したもの(もとは XLM-RoBERTa の SentencePiece モデル、fairseq、MIT)。
