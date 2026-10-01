//! 審査料と審査の申込み(docs/REVIEW_FEE.md、docs/cli.md §7.1)。
//!
//! - `rasq fee <PKG|DIR>…` … 配布物のピース数と審査料を、Central と同じ方法(`rasq-review-pieces`、`xlmr-unigram-v1`)で数える。
//!   ネットは使わない(`--orgate <URL>` を付けたときだけ、ORGATE の公開の `GET /api/v1/payments/config` から式の定数を読む)。
//! - `rasq submit [PKG] --central <URL> [--resubmission-of <申込み id>]` … 開発者の API(`POST /api/v1/dev/plugins/publish`)で版を
//!   審査に出す(= 審査の申込み)。差戻しの修正として出すときは、差し戻された申込みの id を `--resubmission-of` で指す。
//! - `rasq submissions --central <URL>` … 自分の申込み(`GET /api/v1/dev/review-submissions`)と、着手 / 結果通知の目安の日付。

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use rasq_review_pieces::{fee::FeeBasis, PackageCount};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(600))
        .user_agent(concat!("rasq-cli/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// 3 桁ごとのカンマ。
pub fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 数える対象 1 つ(配布物 1 つ、または展開した中身のディレクトリ 1 つ)。
pub struct Counted {
    pub label: String,
    /// 展開した中身のディレクトリを数えた(配布物ではない)。
    pub unpacked_dir: bool,
    pub count: PackageCount,
}

/// パスから数える。ファイルは `.rasqpkg`、ディレクトリは中に `.rasqpkg` があればそれぞれを、無ければ中身(通常ファイルを再帰で)を数える。
pub fn count_path(path: &Path) -> Result<Vec<Counted>> {
    if path.is_file() {
        let bytes = std::fs::read(path).with_context(|| format!("{} を読めません", path.display()))?;
        let count = rasq_review_pieces::count_package(&bytes).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        return Ok(vec![Counted { label: path.display().to_string(), unpacked_dir: false, count }]);
    }
    if !path.is_dir() {
        bail!("{} がありません", path.display());
    }
    let mut pkgs: Vec<PathBuf> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "rasqpkg"))
        .collect();
    pkgs.sort();
    if !pkgs.is_empty() {
        let mut out = Vec::new();
        for p in pkgs {
            out.extend(count_path(&p)?);
        }
        return Ok(out);
    }
    // 展開した中身(rasq inspect --extract の出力など)。symlink はたどらない(配布物の tar でも数えない)。
    let mut files = Vec::new();
    walk(path, path, &mut files)?;
    files.sort();
    let data: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|rel| {
            let bytes = std::fs::read(path.join(&rel))?;
            Ok((rel, bytes))
        })
        .collect::<Result<_>>()?;
    let count = rasq_review_pieces::count_files(data.iter().map(|(p, b)| (p.as_str(), b.as_slice())));
    Ok(vec![Counted { label: path.display().to_string(), unpacked_dir: true, count }])
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let ty = e.file_type()?;
        if ty.is_dir() {
            walk(root, &e.path(), out)?;
        } else if ty.is_file() {
            out.push(e.path().strip_prefix(root)?.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// ORGATE の公開の設定から式の定数を読む(`reviewFee.basis`)。審査料を取らない構成なら None。
fn basis_from_orgate(url: &str) -> Result<Option<FeeBasis>> {
    let base = url.trim_end_matches('/');
    let v: Value = agent()
        .get(&format!("{base}/api/v1/payments/config"))
        .call()
        .with_context(|| format!("ORGATE に繋げません: {base}"))?
        .into_json()?;
    if v["reviewFee"]["enabled"] != true {
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(v["reviewFee"]["basis"].clone()).context("reviewFee.basis を読めません")?))
}

/// 式の文(ORGATE の `formula` と同じ書き方に、ピース数を入れたもの)。
pub fn formula_text(b: &FeeBasis, pieces: u64) -> String {
    let num = |x: f64| {
        let s = format!("{x}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    };
    format!(
        "max({min}, {unit} 円単位に四捨五入({base} × ({p} ÷ {div}) ^ {exp}))",
        min = b.minimum_jpy,
        unit = b.round_to_jpy,
        base = num(b.base_jpy),
        p = commas(pieces),
        div = commas(b.divisor_pieces),
        exp = num(b.exponent),
    )
}

/// `rasq fee`
pub fn fee_cmd(paths: &[PathBuf], as_json: bool, files: bool, orgate: Option<&str>) -> Result<()> {
    let (basis, source) = match orgate {
        Some(url) => match basis_from_orgate(url)? {
            Some(b) => (Some(b), format!("{} の式", url.trim_end_matches('/'))),
            None => (None, format!("{} では審査料はかかりません", url.trim_end_matches('/'))),
        },
        None => (Some(FeeBasis::default()), "既定の式(ORGATE の既定の定数)".to_string()),
    };
    let mut all = Vec::new();
    for p in paths {
        all.extend(count_path(p)?);
    }
    if as_json {
        let out: Vec<Value> = all
            .iter()
            .map(|c| {
                json!({
                    "path": c.label,
                    "unpackedDir": c.unpacked_dir,
                    "method": c.count.method,
                    "packageBytes": (!c.unpacked_dir).then_some(c.count.package_bytes),
                    "unpackedBytes": c.count.unpacked_bytes,
                    "pieces": c.count.pieces,
                    "amountJpy": basis.as_ref().map(|b| b.amount_jpy(c.count.pieces)),
                    "basis": basis,
                    "files": c.count.files,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "計測方法: {}(語彙 {}、語彙ファイルの SHA-256 {})",
        rasq_review_pieces::METHOD,
        commas(rasq_review_pieces::VOCAB_LEN as u64),
        rasq_review_pieces::VOCAB_ZST_SHA256
    );
    for c in &all {
        println!();
        if c.unpacked_dir {
            println!("{}(展開した中身のディレクトリ。配布物にしたときと同じファイルなら同じ数)", c.label);
        } else {
            println!("{}(.rasqpkg {} バイト)", c.label, commas(c.count.package_bytes));
        }
        if files || c.count.files.len() <= 30 {
            println!("  {:>12}  {:>12}  ファイル", "ピース", "バイト");
            for f in &c.count.files {
                println!("  {:>12}  {:>12}  {}", commas(f.pieces), commas(f.bytes), f.path);
            }
        } else {
            println!("  ({} ファイル。--files で一覧)", c.count.files.len());
        }
        println!(
            "  合計: {} ピース / 数えたファイル {} 個・{} バイト",
            commas(c.count.pieces),
            c.count.files.len(),
            commas(c.count.unpacked_bytes)
        );
        match &basis {
            Some(b) => println!(
                "  審査料: ¥{}(税込)= {}  … {source}",
                commas(b.amount_jpy(c.count.pieces) as u64),
                formula_text(b, c.count.pieces)
            ),
            None => println!("  審査料: {source}"),
        }
    }
    if orgate.is_none() {
        println!("\n額の正は ORGATE の式です(定数は運営が変えることがあります)。--orgate <URL> でそのサーバーの式を読めます。申込みの画面にも、そのときの式と額が出ます。");
    }
    Ok(())
}

fn token(central: &str, token_env: &str) -> Result<String> {
    match std::env::var(token_env) {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        _ => crate::credentials::load()?
            .central
            .get(&crate::publish::host_key(central))
            .map(|c| c.token.clone())
            .ok_or_else(|| anyhow!("トークンがありません: 環境変数 {token_env}(Web コンソールの「CLI 用のトークン」)か、rasq token set --central {}", crate::publish::host_key(central))),
    }
}

fn check_url(central: &str) -> Result<String> {
    if !central.starts_with("http://") && !central.starts_with("https://") {
        bail!("--central は http(s):// で始まる URL です: {central}");
    }
    Ok(central.trim_end_matches('/').to_string())
}

fn call(req: ureq::Request, body: Option<Vec<u8>>) -> Result<(u16, Value)> {
    let res = match body {
        Some(b) => req.set("Content-Type", "application/zstd").send_bytes(&b),
        None => req.call(),
    };
    match res {
        Ok(r) => {
            let s = r.status();
            Ok((s, r.into_json().unwrap_or(Value::Null)))
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            Ok((code, serde_json::from_str(&text).unwrap_or(json!({"message": text}))))
        }
        Err(e) => Err(anyhow!("{e}")),
    }
}

fn message(v: &Value) -> String {
    v["message"].as_str().or(v["error"]["message"].as_str()).or(v["error"].as_str()).map(str::to_string).unwrap_or_else(|| v.to_string())
}

fn day(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.chars().take(10).collect())
}

fn yen(v: &Value) -> String {
    commas(v.as_u64().unwrap_or(0))
}

/// 申込み 1 つを人向けに。
pub fn describe_submission(s: &Value) -> Vec<String> {
    let mut out = vec![format!(
        "{} {}  状態 {}  審査料 {}{}",
        s["pluginId"].as_str().unwrap_or(""),
        s["version"].as_str().unwrap_or(""),
        s["status"].as_str().unwrap_or(""),
        s["feeStatus"].as_str().unwrap_or(""),
        match s["freeReason"].as_str() {
            Some(r) => format!("({r})"),
            None if s["amountJpy"].as_u64().unwrap_or(0) > 0 => format!(" ¥{}", yen(&s["amountJpy"])),
            None => String::new(),
        }
    )];
    out.push(format!("  申込み id: {}", s["id"].as_str().unwrap_or("")));
    if let Some(p) = s["feePieces"].as_u64() {
        out.push(format!(
            "  ピース数 {}(展開後 {} バイト)・式 {}",
            commas(p),
            yen(&s["unpackedBytes"]),
            s["feeFormula"].as_str().unwrap_or("")
        ));
    }
    if let Some(of) = s["resubmissionOf"].as_str() {
        out.push(format!("  差戻しの修正として出した申込み: {of}"));
    }
    let mut dates = Vec::new();
    for (k, label) in [("queuedAt", "受付"), ("startedAt", "着手"), ("decidedAt", "結果")] {
        if let Some(d) = day(&s[k]) {
            dates.push(format!("{label} {d}"));
        }
    }
    if !dates.is_empty() {
        out.push(format!("  {}", dates.join(" / ")));
    }
    if let Some(d) = day(&s["startDueAt"]).filter(|_| s["startedAt"].is_null() && s["status"] == "queued") {
        out.push(format!(
            "  着手の目安: {d} まで{}",
            if s["overdue"] == true { "(過ぎています: 取り消すと審査料は全額を返金)" } else { "" }
        ));
    }
    if let Some(d) = day(&s["resultDueAt"]).filter(|_| s["status"] == "in_review") {
        out.push(format!(
            "  結果通知の目安: {d} まで{}",
            if s["resultOverdue"] == true { "(過ぎています: 取り消すと審査料は全額を返金)" } else { "" }
        ));
    }
    if let Some(d) = day(&s["delayNoticedAt"]) {
        out.push(format!(
            "  遅延の連絡({d}): 新しい見込み {}。{}",
            s["delayEstimate"].as_str().unwrap_or("—"),
            s["delayNote"].as_str().unwrap_or("")
        ));
    }
    if s["freeResubmissionAvailable"] == true {
        out.push(format!(
            "  無料の再提出: {} まで(rasq submit --resubmission-of {})",
            day(&s["freeResubmissionUntil"]).unwrap_or_default(),
            s["id"].as_str().unwrap_or("")
        ));
    }
    if let Some(u) = s["payUrl"].as_str() {
        out.push(format!("  支払い: {u}"));
    }
    out
}

/// `rasq submit`
pub fn submit_cmd(central: &str, pkg: Option<PathBuf>, channel: &str, notes_file: Option<PathBuf>, resubmission_of: Option<String>, token_env: &str) -> Result<()> {
    let base = check_url(central)?;
    let pkg = match pkg {
        Some(p) => p,
        None => {
            let dist = std::env::current_dir()?.join("dist");
            let mut found: Vec<PathBuf> = std::fs::read_dir(&dist)
                .with_context(|| format!("{} がありません(配布物のパスを渡すか、rasq build を先に)", dist.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "rasqpkg"))
                .collect();
            if found.len() != 1 {
                bail!("dist/ の .rasqpkg が {} 個です。出す配布物のパスを渡してください", found.len());
            }
            found.remove(0)
        }
    };
    let bytes = std::fs::read(&pkg).with_context(|| format!("{} を読めません", pkg.display()))?;
    let mut sig_path = pkg.clone().into_os_string();
    sig_path.push(".sig.json");
    let sig = std::fs::read(&sig_path).ok();
    let notes = match notes_file {
        Some(f) => std::fs::read_to_string(&f).with_context(|| format!("{} を読めません", f.display()))?,
        None => String::new(),
    };
    // 申込みの前に、手元で数えた額を見せる(Central の額と同じ方法)。
    let local = rasq_review_pieces::count_package(&bytes).map_err(|e| anyhow!("{}: {e}", pkg.display()))?;
    println!("手元の数え: {} ピース(既定の式で ¥{})", commas(local.pieces), commas(FeeBasis::default().amount_jpy(local.pieces) as u64));
    let mut url = format!("{base}/api/v1/dev/plugins/publish?channel={}&notes={}", enc(channel), enc(&notes));
    if let Some(of) = &resubmission_of {
        url.push_str(&format!("&resubmissionOf={}", enc(of)));
    }
    let bearer = token(&base, token_env)?;
    let mut req = agent().post(&url).set("Authorization", &format!("Bearer {bearer}"));
    if let Some(sig) = sig {
        req = req.set("x-rasq-signature", &base64::engine::general_purpose::STANDARD.encode(sig));
    }
    let (status, v) = call(req, Some(bytes))?;
    if status != 200 {
        bail!("出せませんでした: HTTP {status} {}", message(&v));
    }
    println!("出しました: {} {}(審査待ち)", v["id"].as_str().unwrap_or(""), v["version"].as_str().unwrap_or(""));
    for line in describe_submission(&v["submission"]) {
        println!("{line}");
    }
    if v["submission"]["status"] == "awaiting_payment" {
        println!("審査料を払うまで審査は始まりません(上の「支払い」の画面で)。");
    }
    Ok(())
}

/// `rasq submissions`
pub fn submissions_cmd(central: &str, token_env: &str, as_json: bool) -> Result<()> {
    let base = check_url(central)?;
    let bearer = token(&base, token_env)?;
    let req = agent().get(&format!("{base}/api/v1/dev/review-submissions")).set("Authorization", &format!("Bearer {bearer}"));
    let (status, v) = call(req, None)?;
    if status != 200 {
        bail!("読めません: HTTP {status} {}", message(&v));
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let rows = v.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("申込みはまだありません。");
    }
    for s in rows {
        for line in describe_submission(&s) {
            println!("{line}");
        }
        println!();
    }
    Ok(())
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../rasq-review-pieces/tests/fixtures");

    #[test]
    fn fee_of_the_fixture_package_matches_the_expected_counts() {
        // Central(crates/rasq-review-pieces の写し)と同じ期待値。
        let expected: Value = serde_json::from_slice(&std::fs::read(format!("{FIXTURES}/expected.json")).unwrap()).unwrap();
        let c = count_path(Path::new(&format!("{FIXTURES}/sample.rasqpkg"))).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].count.pieces, expected["packages"]["sample.rasqpkg"]["pieces"].as_u64().unwrap());
        assert_eq!(
            i64::from(FeeBasis::default().amount_jpy(c[0].count.pieces)),
            expected["packages"]["sample.rasqpkg"]["amountJpy"].as_i64().unwrap()
        );
    }

    #[test]
    fn an_unpacked_dir_counts_the_same_as_the_package() {
        let pkg = std::fs::read(format!("{FIXTURES}/sample.rasqpkg")).unwrap();
        let files = rasq_review_pieces::package_files(&pkg, rasq_review_pieces::Limits::DEFAULT).unwrap();
        let dir = std::env::temp_dir().join(format!("rasq-fee-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for f in &files {
            let p = dir.join(&f.path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, &f.bytes).unwrap();
        }
        let from_dir = count_path(&dir).unwrap();
        let from_pkg = count_path(Path::new(&format!("{FIXTURES}/sample.rasqpkg"))).unwrap();
        assert!(from_dir[0].unpacked_dir);
        assert_eq!(from_dir[0].count.pieces, from_pkg[0].count.pieces);
        assert_eq!(from_dir[0].count.unpacked_bytes, from_pkg[0].count.unpacked_bytes);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn formula_text_and_commas() {
        assert_eq!(commas(1_876_000), "1,876,000");
        assert_eq!(commas(15), "15");
        assert_eq!(formula_text(&FeeBasis::default(), 4_650_000), "max(150, 10 円単位に四捨五入(100 × (4,650,000 ÷ 1,876,000) ^ 1.21))");
    }

    #[test]
    fn describes_targets_and_free_resubmission() {
        let s = json!({"id": "s1", "pluginId": "p", "version": "1.0.0", "status": "in_review", "feeStatus": "paid", "amountJpy": 300,
            "startedAt": "2026-10-01T00:00:00Z", "resultDueAt": "2026-10-31T00:00:00Z", "resultOverdue": true,
            "delayNoticedAt": "2026-10-20T00:00:00Z", "delayEstimate": "2026-11-10", "delayNote": "混んでいます"});
        let t = describe_submission(&s).join("\n");
        assert!(t.contains("結果通知の目安: 2026-10-31 まで(過ぎています"), "{t}");
        assert!(t.contains("新しい見込み 2026-11-10"), "{t}");
        let r = json!({"id": "s2", "pluginId": "p", "version": "1.0.0", "status": "returned", "feeStatus": "paid", "amountJpy": 300,
            "freeResubmissionAvailable": true, "freeResubmissionUntil": "2026-11-01T00:00:00Z"});
        assert!(describe_submission(&r).join("\n").contains("rasq submit --resubmission-of s2"));
    }
}
