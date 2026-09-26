//! `rasq help <query…>` — 同梱の文書(docs/*.md, PROTOCOL.md)と登録簿(schema/slots.json)、
//! マニフェストのボタン属性(manifest.schema.json の actionDescriptor)を節ごとに索引して探す。
//!
//! 着せ替え(`manifest.theme`、docs/slots.md §2.12)のキーも同じように節にする。
//!
//! 探し方は rasq-client の RAi リファレンス(`NgramRaiEmbedder`)と同じ考え: 語の完全一致を
//! 最優先にし、それ以外は文字 2-gram の Dice 係数で見出しと本文を比べる。ネット無しで動く。
//! `--central <url>` を付けると、Central の公開 API(`GET /api/v1/plugins?q=`)でほかのプラグインの
//! 掲載(名前 / 概要 / 説明)も探す(掲載の説明 = そのプラグインの文書)。

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::HashSet;

use crate::embed;

/// 索引の 1 節。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 出どころ(`docs/slots.md` / `schema/slots.json` / `manifest.schema.json`)。
    pub source: String,
    /// 見出しの道(`§2.9 post.action › 初期状態`)。
    pub title: String,
    pub body: String,
}

/// 文書を節に割る: `#` / `##` / `###` の見出しごと。見出しの道は上位の見出しをつなぐ。
pub fn split_markdown(source: &str, text: &str) -> Vec<Entry> {
    let mut out = vec![];
    let mut path: Vec<(usize, String)> = vec![];
    let mut title = String::new();
    let mut body = String::new();
    let mut in_code = false;
    let flush = |title: &str, body: &mut String, out: &mut Vec<Entry>| {
        if !title.is_empty() && !body.trim().is_empty() {
            out.push(Entry {
                source: source.to_string(),
                title: title.to_string(),
                body: body.trim().to_string(),
            });
        }
        body.clear();
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        let level = line.chars().take_while(|c| *c == '#').count();
        if !in_code && (1..=3).contains(&level) && line.chars().nth(level) == Some(' ') {
            flush(&title, &mut body, &mut out);
            let text = line[level..].trim().to_string();
            path.retain(|(l, _)| *l < level);
            path.push((level, text));
            title = path
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
                .join(" › ");
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&title, &mut body, &mut out);
    out
}

/// 登録簿(slots.json)の 1 スロットを 1 節にする。
fn slot_entries(doc: &Value) -> Vec<Entry> {
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other["id"]
                    .as_str()
                    .or(other["name"].as_str())
                    .map(String::from)
                    .unwrap_or_else(|| other.to_string()),
            })
            .collect()
    };
    doc["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let mut b = String::new();
            let line = |b: &mut String, k: &str, v: String| {
                if !v.is_empty() {
                    b.push_str(&format!("{k}: {v}\n"));
                }
            };
            line(
                &mut b,
                "label",
                s["label"].as_str().unwrap_or("").to_string(),
            );
            line(
                &mut b,
                "status / tier",
                format!(
                    "{} / {}",
                    s["status"].as_str().unwrap_or(""),
                    s["tier"].as_str().unwrap_or("")
                ),
            );
            line(&mut b, "kinds", strs(&s["kinds"]).join(", "));
            line(
                &mut b,
                "calls(ホスト → プラグイン)",
                strs(&s["calls"]).join(", "),
            );
            line(
                &mut b,
                "hostFns(プラグイン → ホスト)",
                strs(&s["hostFns"]).join(", "),
            );
            line(
                &mut b,
                "permissions.required",
                strs(&s["permissions"]["required"]).join(", "),
            );
            line(
                &mut b,
                "permissions.optional",
                strs(&s["permissions"]["optional"]).join(", "),
            );
            line(
                &mut b,
                "surface",
                s["placement"]["surface"].as_str().unwrap_or("").to_string(),
            );
            for c in strs(&s["placement"]["constraints"]) {
                b.push_str(&format!("- {c}\n"));
            }
            if let Some(k) = s["actionKey"].as_str() {
                line(&mut b, "actionKey", k.to_string());
            }
            Entry {
                source: "schema/slots.json".into(),
                title: format!("slot {}", s["id"].as_str().unwrap_or("")),
                body: b,
            }
        })
        .collect()
}

/// マニフェストの `actions[]` の属性を 1 つずつ節にする(`rasq help auto` で説明が出る)。
fn action_attribute_entries(schema: &Value) -> Vec<Entry> {
    attribute_entries(&schema["$defs"]["actionDescriptor"], "manifest.actions[]")
}

/// 着せ替えの宣言(`manifest.theme`、docs/slots.md §2.12)のキーを 1 つずつ節にする(`rasq help opacity` など)。
fn theme_attribute_entries(schema: &Value) -> Vec<Entry> {
    let d = &schema["$defs"];
    let mut out = attribute_entries(&d["theme"], "manifest.theme");
    out.extend(attribute_entries(&d["themeSurface"], "manifest.theme.surfaces.<面>"));
    out.extend(attribute_entries(&d["themeLayer"], "manifest.theme.surfaces.<面>.layers[]"));
    out
}

/// `$defs` の 1 つ(`properties` を持つ object)の、説明のあるキーを節にする。見出しは `<prefix>.<key>`。
fn attribute_entries(def: &Value, prefix: &str) -> Vec<Entry> {
    def["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| {
            let desc = v["description"].as_str()?;
            let mut b = desc.to_string();
            if let Some(e) = v["enum"].as_array() {
                b.push_str(&format!(
                    "\nenum: {}",
                    e.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
            if !v["default"].is_null() {
                b.push_str(&format!("\ndefault: {}", v["default"]));
            }
            Some(Entry {
                source: "schema/manifest.schema.json".into(),
                title: format!("{prefix}.{k}"),
                body: b,
            })
        })
        .collect()
}

/// 同梱のすべてを索引にする。
pub fn index() -> Result<Vec<Entry>> {
    let mut out = vec![];
    for f in embed::DOCS.files() {
        let name = f.path().to_string_lossy().to_string();
        if !name.ends_with(".md") {
            continue;
        }
        let text = f
            .contents_utf8()
            .ok_or_else(|| anyhow!("docs/{name} が UTF-8 ではありません"))?;
        out.extend(split_markdown(&format!("docs/{name}"), text));
    }
    out.extend(split_markdown("PROTOCOL.md", embed::PROTOCOL));
    out.extend(slot_entries(&embed::schema_json("slots.json")?));
    let manifest = embed::schema_json("manifest.schema.json")?;
    out.extend(action_attribute_entries(&manifest));
    out.extend(theme_attribute_entries(&manifest));
    Ok(out)
}

fn norm(s: &str) -> String {
    s.to_lowercase()
}

fn bigrams(s: &str) -> HashSet<(char, char)> {
    let cs: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    cs.windows(2).map(|w| (w[0], w[1])).collect()
}

/// 文字 2-gram の Dice 係数(RAi リファレンスと同じ)。
pub fn dice(a: &str, b: &str) -> f64 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    2.0 * x.intersection(&y).count() as f64 / (x.len() + y.len()) as f64
}

/// 節の点数: 見出しの完全一致 / 部分一致 > 本文の語の一致 > 2-gram の近さ。
pub fn score(e: &Entry, query: &str) -> f64 {
    let q = norm(query);
    let title = norm(&e.title);
    let body = norm(&e.body);
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let mut s = 0.0f64;
    // 見出しの中の 1 語(関数名 / 属性名 / スロット名)と丸ごと一致 = 最優先。
    // `manifest.actions[].auto` の `auto`、`slot post.action` の `post.action` など。
    let words = title
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '_' | '-')))
        .map(|w| w.trim_matches('.'))
        .filter(|w| !w.is_empty());
    if title == q || words.clone().any(|w| w == q) {
        // 同点なら短い見出し(属性 / 関数そのものの節)を先に。長い見出しは本文の中で触れているだけのことが多い。
        s = s.max(1.0 - 0.001 * title.chars().count().min(200) as f64);
    }
    if title.contains(&q) {
        s = s.max(0.9);
    }
    if body.contains(&q) {
        s = s.max(0.75);
    }
    if !tokens.is_empty() {
        let hit = tokens
            .iter()
            .filter(|t| title.contains(*t) || body.contains(*t))
            .count();
        let in_title = tokens.iter().filter(|t| title.contains(*t)).count();
        s = s.max(
            0.7 * hit as f64 / tokens.len() as f64 + 0.15 * in_title as f64 / tokens.len() as f64,
        );
    }
    s.max(0.85 * dice(&title, &q))
}

/// 上位 `n` 件(点数 0.3 未満は捨てる)。
pub fn search(entries: &[Entry], query: &str, n: usize) -> Vec<(f64, Entry)> {
    let mut hits: Vec<(f64, Entry)> = entries
        .iter()
        .map(|e| (score(e, query), e.clone()))
        .filter(|(s, _)| *s >= 0.3)
        .collect();
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(n);
    hits
}

/// 本文を `max_lines` 行までにする(`--full` で全部)。
fn clip(body: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = body.lines().collect();
    if lines.len() <= max_lines {
        return body.to_string();
    }
    format!(
        "{}\n… ({} 行省略。--full で全部)",
        lines[..max_lines].join("\n"),
        lines.len() - max_lines
    )
}

pub fn run(
    query: &[String],
    n: usize,
    full: bool,
    central: Option<&str>,
    plugin: Option<&str>,
) -> Result<()> {
    if let (Some(base), Some(id)) = (central, plugin) {
        return show_plugin(base, id);
    }
    let q = query.join(" ");
    if q.trim().is_empty() {
        println!("使い方: rasq help <関数名 | スロット | 属性 | 探したい言葉…>   例: rasq help net.fetch / rasq help auto / rasq help 全画面 オン");
        println!("       rasq help --central <url> <言葉…>          Central の掲載(ほかのプラグイン)も探す");
        println!(
            "       rasq help --central <url> --plugin <id>    そのプラグインの掲載の説明を読む"
        );
        println!();
        println!("同梱の文書(SDK {}):", embed::SDK_VERSION);
        for f in embed::DOCS.files() {
            let name = f.path().to_string_lossy();
            if name.ends_with(".md") {
                let first = f
                    .contents_utf8()
                    .and_then(|t| t.lines().find(|l| l.starts_with("# ")))
                    .unwrap_or("");
                println!("  docs/{name:<22} {}", first.trim_start_matches("# "));
            }
        }
        println!("  PROTOCOL.md            wasm ABI / RRL / FOW / ホスト関数");
        println!("  schema/slots.json      登録簿(rasq registry slots)");
        return Ok(());
    }
    let entries = index()?;
    let hits = search(&entries, &q, n);
    if hits.is_empty() {
        println!("同梱の文書には見当たりません: {q}");
    }
    for (s, e) in &hits {
        println!("── {}  ({}、{:.2})", e.title, e.source, s);
        println!(
            "{}",
            if full {
                e.body.clone()
            } else {
                clip(&e.body, 24)
            }
        );
        println!();
    }
    if let Some(base) = central {
        search_central(base, &q)?;
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("rasq-cli/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Central の掲載を探す(公開 API、トークン不要)。
fn search_central(base: &str, q: &str) -> Result<()> {
    let base = base.trim_end_matches('/');
    let url = format!("{base}/api/v1/plugins?q={}", urlencode(q));
    let v: Value = agent()
        .get(&url)
        .call()
        .with_context(|| format!("Central に繋げません: {base}"))?
        .into_json()?;
    let items = v["items"].as_array().cloned().unwrap_or_default();
    println!("── Central の掲載 ({base}): {} 件", items.len());
    for p in items.iter().take(10) {
        println!(
            "  {:<32} {}  — {}",
            p["id"].as_str().unwrap_or(""),
            p["name"].as_str().unwrap_or(""),
            p["summary"].as_str().unwrap_or("")
        );
    }
    if !items.is_empty() {
        println!("  (説明を読む: rasq help --central {base} --plugin <id>)");
    }
    Ok(())
}

fn show_plugin(base: &str, id: &str) -> Result<()> {
    let base = base.trim_end_matches('/');
    let v: Value = agent()
        .get(&format!("{base}/api/v1/plugins/{}", urlencode(id)))
        .call()
        .with_context(|| format!("掲載を読めません: {id}"))?
        .into_json()?;
    println!(
        "{}  ({})  v{}",
        v["name"].as_str().unwrap_or(""),
        id,
        v["version"].as_str().unwrap_or("")
    );
    println!("{}", v["summary"].as_str().unwrap_or(""));
    println!();
    println!("{}", v["description"].as_str().unwrap_or("(説明なし)"));
    if let Some(perms) = v["safety"]["permissions"].as_array() {
        println!();
        println!("権限の理由:");
        for p in perms {
            println!(
                "  {}: {}",
                p["permission"].as_str().unwrap_or(""),
                p["reason"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_split_by_headings_with_a_path() {
        let e = split_markdown(
            "t.md",
            "# A\n\nintro\n\n## B\n\nbody b\n```\n# not a heading\n```\n### C\n\nbody c\n",
        );
        let titles: Vec<&str> = e.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["A", "A › B", "A › B › C"]);
        assert!(e[1].body.contains("# not a heading"));
    }

    #[test]
    fn function_names_and_attributes_are_found_first() {
        let entries = index().unwrap();
        let top = |q: &str| {
            search(&entries, q, 3)
                .into_iter()
                .map(|(_, e)| e.title)
                .collect::<Vec<_>>()
        };
        assert!(
            top("post.action").iter().any(|t| t.contains("post.action")),
            "{:?}",
            top("post.action")
        );
        assert_eq!(
            top("auto")[0],
            "manifest.actions[].auto",
            "{:?}",
            top("auto")
        );
        assert!(
            top("net.fetch").iter().any(|t| t.contains("net.fetch")),
            "{:?}",
            top("net.fetch")
        );
        assert!(
            top("全画面 初期状態")
                .iter()
                .any(|t| t.contains("2.11") || t.contains("viewer.overlay")),
            "{:?}",
            top("全画面 初期状態")
        );
        assert!(
            top("着せ替え").iter().any(|t| t.contains("2.12")),
            "{:?}",
            top("着せ替え")
        );
        assert!(
            top("opacity").iter().any(|t| t == "manifest.theme.surfaces.<面>.layers[].opacity"),
            "{:?}",
            top("opacity")
        );
    }

    #[test]
    fn dice_is_symmetric_and_bounded() {
        assert!((dice("abc", "abc") - 1.0).abs() < 1e-9);
        assert_eq!(dice("abc", "xyz"), 0.0);
        assert!(
            (dice("post.action", "post.actions") - dice("post.actions", "post.action")).abs()
                < 1e-9
        );
    }
}
