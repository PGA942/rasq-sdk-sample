//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.7)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{rasq_plugin, rrl, serde_json::{json, Value}, unsupported};

/// 候補にする語(ここをローカル規則 / 辞書に差し替える)。
const TAGS: &[&str] = &["雑談", "RASQ", "写真", "音楽", "ゲーム"];
const DEFAULT_LIMIT: usize = 6;

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{recent: [string], query?, limit?}` → `{suggestions: [{text, reason?}]}`
        "trend.suggest" => {
            let recent: Vec<&str> = args["recent"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let query = args["query"].as_str().unwrap_or("").trim_start_matches('#').to_lowercase();
            let limit = args["limit"].as_u64().map(|n| n as usize).filter(|n| *n > 0).unwrap_or(DEFAULT_LIMIT);
            let picked: Vec<Value> = TAGS
                .iter()
                .filter(|t| query.is_empty() || t.to_lowercase().starts_with(&query))
                .filter(|t| !recent.iter().any(|r| r.eq_ignore_ascii_case(t)))
                .take(limit)
                .map(|t| json!({"text": t, "reason": "雛形の固定タグ"}))
                .collect();
            Ok(json!({"suggestions": picked}))
        }
        _ => Err(unsupported(fn_name)),
    }
}

fn manifest() -> Value {
    rasq_plugin_kit::serde_json::from_str(include_str!("../manifest.json")).expect("manifest.json")
}

fn root() -> Value {
    rrl::column("root", vec![rrl::text("desc", "{{display}}")])
}

rasq_plugin! { scope: "{{id}}:main", root: root, call: handle }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_declares_slot() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["id"], json!("{{id}}"));
        assert_eq!(m["slots"], json!(["{{slot}}"]));
        assert_eq!(call_native("nope", json!({})).unwrap_err()["code"], json!("C01"));
    }

    #[test]
    fn filters_recent_and_query() {
        let r = call_native("trend.suggest", json!({"recent": ["雑談"], "query": "#写", "limit": 3})).unwrap();
        assert_eq!(r["suggestions"][0]["text"], json!("写真"));
        assert_eq!(r["suggestions"].as_array().unwrap().len(), 1);
        let r = call_native("trend.suggest", json!({"recent": ["RASQ"]})).unwrap();
        assert!(r["suggestions"].as_array().unwrap().iter().all(|s| s["text"] != "RASQ"));
    }
}
