//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.6)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{err, rasq_plugin, rrl, serde_json::{json, Value}, unsupported};

const TL_ID: &str = "length";
/// パラメーター名(= RRL ノードの key、`$.settings.<key>`)。
const PARAM_DESC: &str = "desc";

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        "tl.timelines" => Ok(json!({"timelines": [{"id": TL_ID, "label": "本文の長さ順", "base": "home"}]})),
        "tl.params" => {
            check_id(&args)?;
            let mut sw = rrl::switch_tile(PARAM_DESC, "長い順", false, "settings.set");
            sw["p"]["value"] = json!(format!("$.settings.{PARAM_DESC}"));
            Ok(json!({"root": rrl::column("params", vec![sw]), "defaults": {PARAM_DESC: false}}))
        }
        "tl.sort" => {
            check_id(&args)?;
            let desc = args["params"][PARAM_DESC].as_bool().unwrap_or(false);
            let mut posts: Vec<(usize, &str)> = args["posts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| Some((p["body"].as_str().map(|b| b.chars().count()).unwrap_or(0), p["id"].as_str()?)))
                .collect();
            posts.sort_by_key(|(n, _)| *n);
            if desc {
                posts.reverse();
            }
            Ok(json!({"order": posts.iter().map(|(_, id)| *id).collect::<Vec<_>>()}))
        }
        _ => Err(unsupported(fn_name)),
    }
}

fn check_id(args: &Value) -> Result<(), Value> {
    if args["id"].as_str() == Some(TL_ID) { Ok(()) } else { Err(err("C01", format!("unknown timeline: {}", args["id"]))) }
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
    fn sorts_by_length() {
        let posts = json!([{"id": "a", "body": "みっつ"}, {"id": "b", "body": "い"}, {"id": "c", "body": "ふた"}]);
        let r = call_native("tl.sort", json!({"id": TL_ID, "posts": posts, "params": {}})).unwrap();
        assert_eq!(r["order"], json!(["b", "c", "a"]));
        let r = call_native("tl.sort", json!({"id": TL_ID, "posts": posts, "params": {"desc": true}})).unwrap();
        assert_eq!(r["order"], json!(["a", "c", "b"]));
        assert_eq!(call_native("tl.sort", json!({"id": "x"})).unwrap_err()["code"], json!("C01"));
        assert_eq!(call_native("tl.params", json!({"id": TL_ID})).unwrap()["defaults"], json!({"desc": false}));
    }
}
