#!/usr/bin/env python3
"""審査料のピース数の参照実装(計測方法 xlmr-unigram-v1、rasq-sdk-sample docs/REVIEW_FEE.md)。

Rust の `rasq-review-pieces`(rasq-central と `rasq fee` が使う)と同じ数を出す(テストの期待値 tests/fixtures/expected.json はこれで作る)。
Rust に依らずに仕様を確かめるためのもので、遅い(1 MB に数十秒)。

決まり:

- 語彙 = MiniLM(paraphrase-multilingual-MiniLM-L12-v2)のトークナイザー(XLM-R の SentencePiece Unigram、250,002 個)の
  `model.vocab`([piece, score] の並び)から、特別なトークン(`<s>` `<pad>` `</s>` `<unk>` `<mask>`)を除いたもの。
- 配布物(.rasqpkg = zstd の tar)なら、中の通常ファイル(typeflag '0' / NUL)をそれぞれ数えて足す。tar の見出し・ディレクトリ・リンクは数えない。
- バイト列は UTF-8 として読み(壊れたところは U+FFFD に置き換え)、Python の `str.split()` と同じ空白で語に分ける
  (正規化 = Precompiled はしない)。
- 語 w ごとに '▁' + w を、ピースの score の和が最大になるように切る(Viterbi)。語彙に無い 1 文字は score −30 の 1 ピース。
  同点は先に見つけたもの(始まりの位置が前、同じ位置なら短いもの)を残す。
- ピースの数を足す。額 = max(150, round(100 × (ピース数 ÷ 1,876,000) ^ 1.21 ÷ 10) × 10)(0.5 は切り上げ)、上限 1,000,000。

使い方:
    reference_pieces.py --vocab VOCAB FILE...        # ファイルごとのピース数(.rasqpkg は中のファイルごと + 合計と額)
    reference_pieces.py --make-vocab tokenizer.json OUT.json.zst   # 埋め込む語彙を作る

VOCAB は tokenizer.json か、--make-vocab で作った .json.zst(どちらでも同じ数になる)。
"""
import io
import json
import math
import subprocess
import sys
import tarfile

SPECIAL = {"<s>", "<pad>", "</s>", "<unk>", "<mask>"}
UNK_SCORE = -30.0
FEE = {"baseJpy": 100.0, "divisorPieces": 1_876_000, "exponent": 1.21, "minimumJpy": 150, "roundToJpy": 10}
MAX_FEE_JPY = 1_000_000


def load_vocab(path):
    if path.endswith(".zst"):
        raw = subprocess.run(["zstd", "-dc", path], check=True, capture_output=True).stdout
        doc = json.loads(raw)
        return {p: s for p, s in doc["vocab"]}
    doc = json.load(open(path, encoding="utf-8"))
    return {p: s for p, s in doc["model"]["vocab"] if p not in SPECIAL}


def count_word(word, vocab, max_len):
    s = "▁" + word
    n = len(s)
    best = [float("-inf")] * (n + 1)
    count = [0] * (n + 1)
    best[0] = 0.0
    for i in range(n):
        if best[i] == float("-inf"):
            continue
        for l in range(1, min(max_len, n - i) + 1):
            piece = s[i:i + l]
            score = vocab.get(piece)
            if score is None:
                if l != 1:
                    continue
                score = UNK_SCORE
            cand = best[i] + score
            if cand > best[i + l]:
                best[i + l] = cand
                count[i + l] = count[i] + 1
    return count[n]


def count_bytes(data, vocab, max_len):
    text = data.decode("utf-8", errors="replace")
    return sum(count_word(w, vocab, max_len) for w in text.split())


def package_files(raw):
    """.rasqpkg の中の通常ファイル(tar の順)。"""
    tar_bytes = subprocess.run(["zstd", "-dc"], input=raw, check=True, capture_output=True).stdout
    out = []
    with tarfile.open(fileobj=io.BytesIO(tar_bytes), mode="r:") as t:
        for m in t:
            if m.type in (tarfile.REGTYPE, tarfile.AREGTYPE):
                out.append((m.name, t.extractfile(m).read()))
    return out


def fee_jpy(pieces):
    f = FEE
    raw = f["baseJpy"] * (pieces / f["divisorPieces"]) ** f["exponent"]
    unit = max(f["roundToJpy"], 1)
    # 額は正なので、0 から遠い方への丸め(Rust の f64::round)= 0.5 の切り上げ。
    rounded = min(math.floor(raw / unit + 0.5) * unit, MAX_FEE_JPY)
    return min(max(int(rounded), f["minimumJpy"]), MAX_FEE_JPY)


def main(argv):
    if len(argv) >= 3 and argv[0] == "--make-vocab":
        doc = json.load(open(argv[1], encoding="utf-8"))
        vocab = [[p, s] for p, s in doc["model"]["vocab"] if p not in SPECIAL]
        out = json.dumps({
            "source": "paraphrase-multilingual-MiniLM-L12-v2 tokenizer.json (XLM-R SentencePiece Unigram)",
            "excluded": sorted(SPECIAL),
            "vocab": vocab,
        }, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        subprocess.run(["zstd", "-19", "-q", "-f", "-o", argv[2]], input=out, check=True)
        print(f"{len(vocab)} pieces -> {argv[2]}")
        return
    if len(argv) < 3 or argv[0] != "--vocab":
        print(__doc__)
        sys.exit(2)
    vocab = load_vocab(argv[1])
    max_len = max(len(p) for p in vocab)
    for path in argv[2:]:
        with open(path, "rb") as f:
            raw = f.read()
        if path.endswith(".rasqpkg"):
            total = 0
            for name, data in package_files(raw):
                n = count_bytes(data, vocab, max_len)
                total += n
                print(f"{n}\t{path}:{name}")
            print(f"{total}\t{path}\t¥{fee_jpy(total)}")
        else:
            print(f"{count_bytes(raw, vocab, max_len)}\t{path}")


if __name__ == "__main__":
    main(sys.argv[1:])
