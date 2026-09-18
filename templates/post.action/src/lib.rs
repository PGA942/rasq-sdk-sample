//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.9)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{err, host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported};

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{token, actionId, post, account}` → `{result?: {done?, message?}}`(message はトーストに出る)
        "post.action" => {
            let post = args.get("post").filter(|p| p.is_object()).ok_or_else(|| err("D01", "post required"))?;
            match args["actionId"].as_str().unwrap_or("") {
                "hello" => {
                    let body = post["body"].as_str().unwrap_or("");
                    host::clipboard_set(body);
                    Ok(json!({"result": {"done": true, "message": format!("{} 文字をコピーしました", body.chars().count())}}))
                }
                other => Err(err("D01", format!("unknown actionId: {other}"))),
            }
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
    use rasq_plugin_kit::take_host_log;

    #[test]
    fn manifest_declares_slot() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["id"], json!("{{id}}"));
        assert_eq!(m["slots"], json!(["{{slot}}"]));
        assert_eq!(call_native("nope", json!({})).unwrap_err()["code"], json!("C01"));
    }

    #[test]
    fn manifest_declares_actions_with_tags() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["actions"][0]["id"], json!("hello"));
        assert!(m["actions"][0]["tags"].as_array().unwrap().len() >= 1);
    }

    #[test]
    fn hello_copies_body() {
        take_host_log();
        let r = call_native("post.action", json!({"token": "t", "actionId": "hello", "post": {"body": "abc"}, "account": {}})).unwrap();
        assert_eq!(r["result"]["done"], json!(true));
        assert_eq!(take_host_log()[0].1["text"], json!("abc"));
        assert_eq!(call_native("post.action", json!({"actionId": "x", "post": {}})).unwrap_err()["code"], json!("D01"));
        assert_eq!(call_native("post.action", json!({"actionId": "hello"})).unwrap_err()["code"], json!("D01"));
    }
}
