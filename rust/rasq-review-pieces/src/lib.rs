//! RASQ の審査料の「ピース数」— 計測方法 `xlmr-unigram-v1`(仕様は rasq-sdk-sample `docs/REVIEW_FEE.md`)。
//!
//! 審査料の額は、申し込む版の配布物(`.rasqpkg`)の**中のファイルの内容**を、このクレートの方法で数えたピース数で決まる
//! (額の式の正は ORGATE-Server。[`fee`] は同じ式の写しで、`rasq fee` が手元で額を出すのに使う)。
//! rasq-central(申込みのときに数える)と SDK の `rasq fee`(開発者が手元で数える)が**同じこのクレート**を使う。
//! Central は `crates/rasq-review-pieces/` にこのディレクトリをそのまま写し、CI が SDK との差を `diff -r` で確かめる。
//!
//! 決まり(参照実装 `tool/reference_pieces.py` と同じ数を出す。テストの期待値 `tests/fixtures/expected.json` はそれで作った):
//!
//! 1. 対象: `.rasqpkg`(zstd で圧縮した tar)を解いた中の**通常ファイル**(typeflag `0` / NUL)全部。tar の見出し・ディレクトリ・
//!    リンクは数えない。ファイルごとに数えて足す(順は和に関係しない)。
//! 2. バイト列 → 文字列: UTF-8 として読み、壊れたところは U+FFFD(Rust の `String::from_utf8_lossy` = Python の
//!    `bytes.decode("utf-8", errors="replace")`)。正規化(NFKC / Precompiled)はしない。
//! 3. 語に分ける: 空白(Python の `str.isspace()` と同じ集合 = Rust の `char::is_whitespace` + U+001C〜U+001F)で切り、空の語は捨てる。
//! 4. 語 w ごとに `'▁'(U+2581) + w` を、ピースの score の和が最大になるように切る(Unigram の Viterbi)。語彙に無い 1 文字は
//!    score −30 の 1 ピース。同点は先に見つけた切り方(始まりが前、同じなら短いピース)。長さの上限・切り詰めは無い。
//! 5. 語彙: `assets/xlmr-unigram-vocab.json.zst`(paraphrase-multilingual-MiniLM-L12-v2 の tokenizer.json の `model.vocab` から、特別な
//!    トークン `<s>` `<pad>` `</s>` `<unk>` `<mask>` を除いた 249,997 個。ピースは最長 16 文字)。**バイナリに埋め込む**。
//!    [`VOCAB_ZST_SHA256`] / [`VOCAB_JSON_SHA256`] がこの版の語彙の指紋。語彙を変えると値段が変わるので、変えるときは
//!    [`METHOD`] の版を上げる。
//!
//! 速さ: 語彙の木(文字の辺を `FxHashMap` に)を最初に使うときに 1 度だけ作る(約 0.3 秒・数十 MB)。数えるのは 1 文字あたり数回の
//! 表引き(リリースビルドで 20 MB の wasm が 0.3 秒ほど)。

use std::io::Read;
use std::sync::OnceLock;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// 計測方法の名前と版。申込み(`fee_basis.pieceMethod`)と `rasq fee` の出力に出す。
pub const METHOD: &str = "xlmr-unigram-v1";

/// 語彙の出どころ。
pub const VOCAB_SOURCE: &str =
    "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2 tokenizer.json (XLM-R SentencePiece Unigram)";

/// 埋め込みの語彙ファイル(`assets/xlmr-unigram-vocab.json.zst`)の SHA-256。
pub const VOCAB_ZST_SHA256: &str =
    "c1da5503a926fa72e2d7253fc462dfa750927481b101efb95b3c76b51dd9db49";

/// 語彙ファイルを zstd で解いた JSON(正の語彙)の SHA-256。
pub const VOCAB_JSON_SHA256: &str =
    "4d3556e2073785a9706911a4351eaa36bbc3853a5dab4807feb0deba4656f44a";

/// 語彙を作ったもとの tokenizer.json の SHA-256(`tool/reference_pieces.py --make-vocab` の入力)。
pub const SOURCE_TOKENIZER_SHA256: &str =
    "cad551d5600a84242d0973327029452a1e3672ba6313c2a3c3d69c4310e12719";

/// 語彙の数(特別なトークン 5 つを除く)。
pub const VOCAB_LEN: usize = 249_997;

/// 語彙に無い 1 文字の score。
pub const UNK_SCORE: f64 = -30.0;

const VOCAB_ZST: &[u8] = include_bytes!("../assets/xlmr-unigram-vocab.json.zst");

#[derive(Deserialize)]
struct VocabFile {
    vocab: Vec<(String, f64)>,
}

/// Unigram の語彙の木。
pub struct PieceCounter {
    /// (親の節 << 21 | 文字) → 子の節。根は 0。
    edges: FxHashMap<u64, u32>,
    /// 節で終わるピースの score(ピースでなければ NaN)。
    scores: Vec<f64>,
    max_len: usize,
}

fn key(node: u32, c: char) -> u64 {
    (u64::from(node) << 21) | u64::from(c)
}

/// Python の `str.isspace()` と同じ(Rust の `char::is_whitespace` に U+001C〜U+001F を足したもの)。
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

impl PieceCounter {
    pub fn new(vocab: &[(String, f64)]) -> Self {
        let mut edges = FxHashMap::default();
        edges.reserve(vocab.len() * 3);
        let mut scores = vec![f64::NAN];
        let mut max_len = 1;
        for (piece, score) in vocab {
            let mut node = 0u32;
            let mut len = 0;
            for c in piece.chars() {
                len += 1;
                let next = scores.len() as u32;
                node = *edges.entry(key(node, c)).or_insert_with(|| {
                    scores.push(f64::NAN);
                    next
                });
            }
            if len > 0 {
                scores[node as usize] = *score;
                max_len = max_len.max(len);
            }
        }
        Self {
            edges,
            scores,
            max_len,
        }
    }

    /// 埋め込みの語彙(`xlmr-unigram-v1`)。最初の呼び出しで作る。
    pub fn embedded() -> &'static Self {
        static COUNTER: OnceLock<PieceCounter> = OnceLock::new();
        COUNTER.get_or_init(|| {
            let raw = zstd::decode_all(VOCAB_ZST).expect("embedded vocab is zstd");
            let file: VocabFile = serde_json::from_slice(&raw).expect("embedded vocab is json");
            Self::new(&file.vocab)
        })
    }

    /// 語彙の数。
    pub fn len(&self) -> usize {
        self.scores.iter().filter(|s| !s.is_nan()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// いちばん長いピースの文字数。
    pub fn max_len(&self) -> usize {
        self.max_len
    }

    /// バイト列 1 つ(ファイル 1 つ)のピース数。
    pub fn count_bytes(&self, data: &[u8]) -> u64 {
        let text = String::from_utf8_lossy(data);
        let mut buf: Vec<char> = Vec::new();
        let mut best: Vec<f64> = Vec::new();
        let mut count: Vec<u64> = Vec::new();
        let mut total = 0u64;
        for word in text.split(is_space).filter(|w| !w.is_empty()) {
            buf.clear();
            buf.push('\u{2581}');
            buf.extend(word.chars());
            total += self.count_word(&buf, &mut best, &mut count);
        }
        total
    }

    /// `'▁' + 語` の Viterbi(score の和が最大の切り方の、ピースの数)。
    fn count_word(&self, s: &[char], best: &mut Vec<f64>, count: &mut Vec<u64>) -> u64 {
        let n = s.len();
        best.clear();
        best.resize(n + 1, f64::NEG_INFINITY);
        count.clear();
        count.resize(n + 1, 0);
        best[0] = 0.0;
        for i in 0..n {
            let base = best[i];
            if base == f64::NEG_INFINITY {
                continue;
            }
            let mut node = 0u32;
            for l in 1..=self.max_len.min(n - i) {
                let score = match self.edges.get(&key(node, s[i + l - 1])) {
                    Some(&child) => {
                        node = child;
                        let sc = self.scores[child as usize];
                        if sc.is_nan() {
                            if l != 1 {
                                continue;
                            }
                            UNK_SCORE
                        } else {
                            sc
                        }
                    }
                    None => {
                        if l == 1 {
                            // 語彙に無い 1 文字。これより長いピースも無い。
                            let cand = base + UNK_SCORE;
                            if cand > best[i + 1] {
                                best[i + 1] = cand;
                                count[i + 1] = count[i] + 1;
                            }
                        }
                        break;
                    }
                };
                let cand = base + score;
                if cand > best[i + l] {
                    best[i + l] = cand;
                    count[i + l] = count[i] + 1;
                }
            }
        }
        count[n]
    }
}

/// 配布物を開くときの上限(zstd 爆弾の対策。rasq-central の版を出す口と同じ値)。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 展開後の合計(バイト)。
    pub max_unpacked: u64,
    /// ファイルの数。
    pub max_entries: usize,
}

impl Limits {
    /// Central と同じ: 展開後 256 MiB、2000 ファイル。
    pub const DEFAULT: Limits = Limits {
        max_unpacked: 256 * 1024 * 1024,
        max_entries: 2000,
    };
}

/// 配布物の中のファイル 1 つ。
#[derive(Debug, Clone)]
pub struct PackageFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// 配布物(`.rasqpkg` = zstd の tar)を解いて、中の通常ファイルを tar の順に返す。
pub fn package_files(pkg: &[u8], limits: Limits) -> Result<Vec<PackageFile>, String> {
    let decoder =
        zstd::stream::read::Decoder::new(pkg).map_err(|_| "not a .rasqpkg (zstd)".to_string())?;
    let mut archive = tar::Archive::new(decoder.take(limits.max_unpacked + 1));
    let mut out = Vec::new();
    for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if out.len() >= limits.max_entries {
            return Err("too many files in package".into());
        }
        let path = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("tar read: {e}"))?;
        out.push(PackageFile { path, bytes });
    }
    Ok(out)
}

/// ファイル 1 つの数え。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCount {
    pub path: String,
    pub bytes: u64,
    pub pieces: u64,
}

/// 配布物 1 つの数え。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageCount {
    /// 計測方法([`METHOD`])。
    pub method: &'static str,
    /// 配布物(圧縮)のバイト数。
    pub package_bytes: u64,
    /// 数えたファイルの合計のバイト数(展開後)。
    pub unpacked_bytes: u64,
    /// ピース数(額の元)。
    pub pieces: u64,
    pub files: Vec<FileCount>,
}

/// ファイル(名前とバイト列)の並びを数える。
pub fn count_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> PackageCount {
    let c = PieceCounter::embedded();
    let files: Vec<FileCount> = files
        .into_iter()
        .map(|(path, bytes)| FileCount {
            path: path.to_string(),
            bytes: bytes.len() as u64,
            pieces: c.count_bytes(bytes),
        })
        .collect();
    PackageCount {
        method: METHOD,
        package_bytes: 0,
        unpacked_bytes: files.iter().map(|f| f.bytes).sum(),
        pieces: files.iter().map(|f| f.pieces).sum(),
        files,
    }
}

/// 配布物を開いて数える(Central が申込みのときに使うのと同じ)。
pub fn count_package(pkg: &[u8]) -> Result<PackageCount, String> {
    let files = package_files(pkg, Limits::DEFAULT)?;
    let mut count = count_files(files.iter().map(|f| (f.path.as_str(), f.bytes.as_slice())));
    count.package_bytes = pkg.len() as u64;
    Ok(count)
}

/// 審査料の式の写し(正は ORGATE-Server `src/payments/fee.rs`。定数は ORGATE の `ORGATE_REVIEW_FEE_*` で変えられ、
/// 公開の `GET /api/v1/payments/config` の `reviewFee.basis` に出る)。
pub mod fee {
    use serde::{Deserialize, Serialize};

    /// 式の定数(`fee_basis`)。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FeeBasis {
        pub base_jpy: f64,
        pub divisor_pieces: u64,
        pub exponent: f64,
        pub minimum_jpy: i32,
        pub round_to_jpy: i32,
    }

    /// 式が壊れた値を出しても、桁違いの請求をしない上限(ORGATE と同じ)。
    pub const MAX_FEE_JPY: i32 = 1_000_000;

    impl Default for FeeBasis {
        /// 既定(ユーザー決定 2026-10-01): max(150, 10 円単位(100 × (ピース数 ÷ 1,876,000) ^ 1.21))。
        fn default() -> Self {
            Self {
                base_jpy: 100.0,
                divisor_pieces: 1_876_000,
                exponent: 1.21,
                minimum_jpy: 150,
                round_to_jpy: 10,
            }
        }
    }

    impl FeeBasis {
        /// ピース数 → 額(円・税込)。
        ///
        /// `raw = base × (pieces ÷ divisor) ^ exponent`(f64)→ `round(raw ÷ unit) × unit`(f64 の `round` = 0.5 は 0 から遠い方、
        /// 額は正なので四捨五入)→ 上限 [`MAX_FEE_JPY`] → 最低額との大きい方。
        pub fn amount_jpy(&self, pieces: u64) -> i32 {
            let raw =
                self.base_jpy * (pieces as f64 / self.divisor_pieces as f64).powf(self.exponent);
            let unit = f64::from(self.round_to_jpy.max(1));
            let rounded = ((raw / unit).round() * unit).min(f64::from(MAX_FEE_JPY));
            (rounded as i32).max(self.minimum_jpy).min(MAX_FEE_JPY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    fn expected() -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(format!("{FIXTURES}/expected.json")).unwrap())
            .unwrap()
    }

    #[test]
    fn matches_the_python_reference_on_the_fixtures() {
        let expected = expected();
        let c = PieceCounter::embedded();
        assert_eq!(c.len(), VOCAB_LEN);
        assert_eq!(c.max_len(), 16);
        let pieces = expected["pieces"].as_object().unwrap();
        assert!(pieces.len() >= 4);
        for (file, want) in pieces {
            let data = std::fs::read(format!("{FIXTURES}/{file}")).unwrap();
            assert_eq!(c.count_bytes(&data), want.as_u64().unwrap(), "{file}");
        }
    }

    #[test]
    fn the_embedded_vocab_is_the_published_one() {
        use sha2::{Digest, Sha256};
        let hex = |b: &[u8]| -> String {
            Sha256::digest(b)
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect()
        };
        assert_eq!(hex(VOCAB_ZST), VOCAB_ZST_SHA256);
        assert_eq!(
            hex(&zstd::decode_all(VOCAB_ZST).unwrap()),
            VOCAB_JSON_SHA256
        );
    }

    #[test]
    fn small_cases() {
        let vocab: Vec<(String, f64)> = [("▁ab", -1.0), ("▁a", -2.0), ("b", -2.0), ("▁", -3.0)]
            .iter()
            .map(|(p, s)| (p.to_string(), *s))
            .collect();
        let c = PieceCounter::new(&vocab);
        // ▁ab が 1 つ。
        assert_eq!(c.count_bytes(b"ab"), 1);
        // 空白で分ける(U+001C も空白)。
        assert_eq!(c.count_bytes(b" ab\tab\x1cab "), 3);
        // ▁ + 語彙に無い x(-30)+ b … ▁a x b = 3、▁ x b でなく。
        assert_eq!(c.count_bytes(b"axb"), 3);
        // 語彙に無い文字は 1 文字ずつ(NUL も空白ではない)。
        assert_eq!(c.count_bytes(b"\0\0\0"), 4);
        assert_eq!(c.count_bytes(b""), 0);
        assert_eq!(c.count_bytes(b" \n "), 0);
        // 壊れた UTF-8 は U+FFFD(語彙に無い 1 文字)。
        assert_eq!(c.count_bytes(b"a\xffb"), 3);
    }

    fn tar_zst(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, kind, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            if *kind == tar::EntryType::Symlink {
                h.set_link_name("manifest.json").unwrap();
            }
            b.append_data(&mut h, name, *data).unwrap();
        }
        zstd::encode_all(b.into_inner().unwrap().as_slice(), 3).unwrap()
    }

    #[test]
    fn counts_only_regular_files_of_the_package() {
        let expected = expected();
        let data = std::fs::read(format!("{FIXTURES}/mixed_text.txt")).unwrap();
        let pkg = tar_zst(&[
            (
                "bundle.json",
                tar::EntryType::Regular,
                b"{\"schema\": \"rasq.bundle/1\"}",
            ),
            ("assets/", tar::EntryType::Directory, b""),
            ("assets/text.txt", tar::EntryType::Regular, &data),
            ("assets/link", tar::EntryType::Symlink, b""),
        ]);
        let c = count_package(&pkg).unwrap();
        assert_eq!(c.method, METHOD);
        assert_eq!(c.package_bytes, pkg.len() as u64);
        let paths: Vec<&str> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["bundle.json", "assets/text.txt"]);
        assert_eq!(
            c.files[1].pieces,
            expected["pieces"]["mixed_text.txt"].as_u64().unwrap()
        );
        assert_eq!(c.pieces, c.files[0].pieces + c.files[1].pieces);
        assert_eq!(c.unpacked_bytes, 27 + data.len() as u64);
        assert!(count_package(b"not zstd").is_err());
    }

    #[test]
    fn matches_the_python_reference_on_the_package_fixture() {
        // sample.rasqpkg: 通常ファイル 4 つ + ディレクトリ + シンボリックリンク(数えない)。
        let expected = expected();
        for (name, want) in expected["packages"].as_object().unwrap() {
            let pkg = std::fs::read(format!("{FIXTURES}/{name}")).unwrap();
            let c = count_package(&pkg).unwrap();
            assert_eq!(c.pieces, want["pieces"].as_u64().unwrap(), "{name}");
            let files = want["files"].as_object().unwrap();
            assert_eq!(c.files.len(), files.len());
            for f in &c.files {
                assert_eq!(
                    f.pieces,
                    files[&f.path].as_u64().unwrap(),
                    "{name}:{}",
                    f.path
                );
            }
            assert_eq!(
                i64::from(fee::FeeBasis::default().amount_jpy(c.pieces)),
                want["amountJpy"].as_i64().unwrap()
            );
        }
    }

    #[test]
    fn fee_matches_the_orgate_calibration() {
        // ORGATE-Server src/payments/fee.rs の calibration_points と同じ値。
        let f = fee::FeeBasis::default();
        assert_eq!(f.amount_jpy(5 * 930_000), 300);
        assert_eq!(f.amount_jpy(20 * 930_000), 1610);
        assert_eq!(f.amount_jpy(1_876_000), 150);
        assert_eq!(f.amount_jpy(0), 150);
        let one = fee::FeeBasis {
            round_to_jpy: 1,
            ..fee::FeeBasis::default()
        };
        assert_eq!(one.amount_jpy(20 * 930_000), 1605);
        assert_eq!(f.amount_jpy(u64::MAX), fee::MAX_FEE_JPY);
        // 期待値の表(expected.json の fee)も同じ。
        for row in expected()["fee"].as_array().unwrap() {
            assert_eq!(
                i64::from(f.amount_jpy(row["pieces"].as_u64().unwrap())),
                row["amountJpy"].as_i64().unwrap(),
                "{row}"
            );
        }
    }

    #[test]
    fn real_tokenizer_still_matches_the_embedded_vocab() {
        // もとの tokenizer.json があれば(RASQ_MINILM_TOKENIZER か端末内モデルの置き場)、埋め込みの語彙と同じかを確かめる(CI には無いので飛ばす)。
        let path = std::env::var("RASQ_MINILM_TOKENIZER").unwrap_or_else(|_| {
            "/workspace/models/minilm-mobilenet/onnx/tokenizer/tokenizer.json".into()
        });
        let Ok(raw) = std::fs::read(&path) else {
            return;
        };
        use sha2::{Digest, Sha256};
        let sha: String = Sha256::digest(&raw)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        assert_eq!(sha, SOURCE_TOKENIZER_SHA256);
        let doc: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let vocab = doc["model"]["vocab"].as_array().unwrap();
        assert_eq!(vocab.len(), 250_002);
        let special = ["<s>", "<pad>", "</s>", "<unk>", "<mask>"];
        let n = vocab
            .iter()
            .filter(|v| !special.contains(&v[0].as_str().unwrap()))
            .count();
        assert_eq!(n, PieceCounter::embedded().len());
    }

    /// 速さを測る: `RASQ_PIECES_BENCH=<ファイル …> cargo test --release -- --ignored pieces_bench --nocapture`。
    #[test]
    #[ignore]
    fn pieces_bench() {
        let c = PieceCounter::embedded();
        for path in std::env::var("RASQ_PIECES_BENCH")
            .unwrap_or_default()
            .split_whitespace()
        {
            let data = std::fs::read(path).unwrap();
            let t = std::time::Instant::now();
            let n = c.count_bytes(&data);
            let secs = t.elapsed().as_secs_f64();
            println!(
                "{path}\t{} bytes\t{n} pieces\t{:.0} pieces/MB\t{secs:.3}s\t{:.1} MB/s",
                data.len(),
                n as f64 / (data.len() as f64 / 1e6),
                data.len() as f64 / 1e6 / secs
            );
        }
    }
}
