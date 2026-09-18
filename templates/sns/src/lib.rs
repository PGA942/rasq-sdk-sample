//! {{name}} — SNS プラグインの雛形(rasq-sdk-sample docs/sns-boundary.md)。
//!
//! `sns.*` に答えるだけの最小形(画面コンストラクタ無し = ホストが標準の投稿タイルで描く)。
//! 実サーバーへは `net.fetch`(ホスト関数)で行く。ホストは args に `host` / `token` を追い乗せする(暫定規約)。
use rasq_plugin_kit::{rasq_plugin, rrl, serde_json::{json, Value}, unsupported};

fn manifest() -> Value {
    rasq_plugin_kit::serde_json::from_str(include_str!("../manifest.json")).expect("manifest.json")
}

/// 最小の Post ワイヤ(`rasq.post/1`)。
fn wire_post(id: &str, body: &str) -> Value {
    json!({
        "id": id,
        "author": {"id": "me", "kind": "other", "instance": "example.test", "handle": "me", "displayName": "{{display}}"},
        "createdAt": "2026-09-18T00:00:00.000Z",
        "body": body,
        "bodyFormat": "plain",
        "counts": {"reply": 0, "renote": 0, "reaction": 0},
        "reactions": {},
    })
}

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // TODO: net.fetch で実サーバーの TL を取り、Post ワイヤに写す
        "sns.timeline" => Ok(json!({"page": {"items": [wire_post("p1", "{{display}} の最初の投稿")], "nextCursor": null}})),
        "sns.me" => Ok(json!({"user": wire_post("p1", "")["author"]})),
        "sns.actions" => Ok(json!({"actions": manifest()["actions"]})),
        "sns.performAction" => {
            let _post_id = args["postId"].as_str().unwrap_or("");
            Ok(json!({"result": {"done": true}}))
        }
        _ => Err(unsupported(fn_name)),
    }
}

fn root() -> Value {
    rrl::column("root", vec![rrl::text("desc", "{{display}}")])
}

rasq_plugin! { scope: "{{id}}:main", root: root, call: handle }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_sns() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["id"], json!("{{id}}"));
        assert_eq!(m["kind"], json!("sns"));
        assert_eq!(call_native("sns.thread", json!({})).unwrap_err()["code"], json!("C01"));
    }

    #[test]
    fn timeline_has_a_post() {
        let r = call_native("sns.timeline", json!({"kind": "home", "limit": 20})).unwrap();
        assert_eq!(r["page"]["items"][0]["id"], json!("p1"));
    }
}
