//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.5)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{rasq_plugin, rrl, serde_json::{json, Map, Value}, unsupported};

/// 置き換える語(ここを自分の処理に差し替える)。
const FROM: &str = "こんにちは";
const TO: &str = "こんにちは👋";

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{posts: [Post]}` → `{posts: {postId: {body?, cw?}}}`(変わった投稿だけ)
        "post.transform" => Ok(transform(&args)),
        _ => Err(unsupported(fn_name)),
    }
}

fn transform(args: &Value) -> Value {
    let mut out = Map::new();
    for post in args["posts"].as_array().into_iter().flatten() {
        let Some(id) = post["id"].as_str() else { continue };
        let mut patch = Map::new();
        for field in ["body", "cw"] {
            if let Some(text) = post[field].as_str() {
                if text.contains(FROM) {
                    patch.insert(field.to_string(), json!(text.replace(FROM, TO)));
                }
            }
        }
        if !patch.is_empty() {
            out.insert(id.to_string(), Value::Object(patch));
        }
    }
    json!({ "posts": out })
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
    fn replaces_word_and_omits_unchanged() {
        let r = call_native("post.transform", json!({"posts": [
            {"id": "a", "body": "こんにちは 世界", "cw": null},
            {"id": "b", "body": "無関係"},
        ]})).unwrap();
        assert_eq!(r["posts"]["a"], json!({"body": "こんにちは👋 世界"}));
        assert!(r["posts"].get("b").is_none());
        assert_eq!(call_native("post.transform", Value::Null).unwrap(), json!({"posts": {}}));
    }
}
