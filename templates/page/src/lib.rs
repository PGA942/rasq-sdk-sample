//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.13、契約 v0.18)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`page.open` でページの RRL を返し、変化は `page.render` で差し替える。
//! shared のセッション(`page.open` の `session`)があればサーバー変数 `count` を `shared.op incr` で増やし、
//! 結果は `page.event {type: "var"}` で届く(自分の書き込みも)。セッションが無ければ端末の中だけで数える。
use rasq_plugin_kit::{err, host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported};
use std::sync::Mutex;

#[derive(Default)]
struct Page {
    /// 開いているページの token(閉じていれば空)。
    token: String,
    /// shared のセッションに入っているか。
    shared: bool,
    count: i64,
}

static PAGE: Mutex<Page> = Mutex::new(Page { token: String::new(), shared: false, count: 0 });

fn page_root(p: &Page) -> Value {
    let mode = if p.shared { "みんなで数えています" } else { "この端末だけで数えています" };
    rrl::column(
        "root",
        vec![
            rrl::styled_text("count", p.count.to_string(), 40.0, true, None),
            rrl::text("mode", mode),
            rrl::button("inc", "+1", if p.shared { "count.shared" } else { "count.local" }),
        ],
    )
}

fn render(p: &Page) {
    host::page_render(&p.token, page_root(p), None);
}

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{token, surface, context, me, session?, size}` → `{root, title?}`
        "page.open" => {
            let token = args["token"].as_str().unwrap_or("");
            if token.is_empty() {
                return Err(err("D01", "token required"));
            }
            let mut p = PAGE.lock().unwrap();
            let session = &args["session"];
            *p = Page { token: token.to_string(), shared: session.is_object(), count: session["vars"]["count"]["value"].as_i64().unwrap_or(0) };
            Ok(json!({"root": page_root(&p)}))
        }
        // `{token, type, ...}`: var / call / joined / left / conflict / closed / resized
        "page.event" => {
            let mut p = PAGE.lock().unwrap();
            if p.token.is_empty() || args["token"] != json!(p.token) {
                return Ok(json!({})); // 古いページへの通知
            }
            match args["type"].as_str().unwrap_or("") {
                "var" | "conflict" if args["key"] == "count" => {
                    p.count = args["value"].as_i64().unwrap_or(0);
                    render(&p);
                }
                "closed" => {
                    p.shared = false;
                    render(&p);
                }
                _ => {}
            }
            Ok(json!({}))
        }
        // RRL の on.tap: `{token, nodeId, kind, ...payload}`(payload は平らに展開)
        "count.shared" => {
            let p = PAGE.lock().unwrap();
            host::shared_op(&p.token, "count", "incr", json!(1));
            Ok(json!({}))
        }
        "count.local" => {
            let mut p = PAGE.lock().unwrap();
            p.count += 1;
            render(&p);
            Ok(json!({}))
        }
        "page.close" => {
            *PAGE.lock().unwrap() = Page::default();
            Ok(json!({}))
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
    use rasq_plugin_kit::{set_host_hook, take_host_log};

    fn open(session: Value) -> Value {
        let mut a = json!({"token": "p1", "surface": "page", "context": {}, "me": {"accountId": "a1", "handle": "me", "displayName": "わたし"}, "size": {"width": 400, "height": 800}});
        if !session.is_null() {
            a["session"] = session;
        }
        call_native("page.open", a).unwrap()
    }

    #[test]
    fn manifest_declares_slot() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["id"], json!("{{id}}"));
        assert_eq!(m["slots"], json!(["{{slot}}"]));
        assert_eq!(m["page"]["mode"], json!("shared"));
        assert_eq!(call_native("nope", json!({})).unwrap_err()["code"], json!("C01"));
        assert_eq!(call_native("page.open", json!({})).unwrap_err()["code"], json!("D01"));
    }

    #[test]
    fn shared_and_solo() {
        set_host_hook(|_, _| json!({}));
        take_host_log();
        // shared: 押すと shared.op、数は page.event var で変わる
        let r = open(json!({"id": "s1", "scope": "room:r1", "role": "member", "participants": [], "vars": {"count": {"value": 4, "rev": 2, "by": "a2"}}}));
        assert_eq!(r["root"]["c"][0]["p"]["text"], json!("4"));
        call_native("count.shared", json!({"token": "p1", "nodeId": "x", "kind": "tap"})).unwrap();
        assert_eq!(take_host_log()[0], ("shared.op".to_string(), json!({"token": "p1", "key": "count", "op": "incr", "value": 1})));
        call_native("page.event", json!({"token": "p1", "type": "var", "key": "count", "value": 5, "rev": 3, "by": "a1"})).unwrap();
        let log = take_host_log();
        assert_eq!(log[0].0, "page.render");
        assert_eq!(log[0].1["root"]["c"][0]["p"]["text"], json!("5"));
        // 古い token は無視
        call_native("page.event", json!({"token": "old", "type": "var", "key": "count", "value": 9})).unwrap();
        assert!(take_host_log().is_empty());
        // solo(セッション無し): 端末の中だけ
        let r = open(Value::Null);
        assert_eq!(r["root"]["c"][2]["on"]["tap"], json!("count.local"));
        call_native("count.local", json!({})).unwrap();
        assert_eq!(take_host_log()[0].1["root"]["c"][0]["p"]["text"], json!("1"));
        call_native("page.close", json!({"token": "p1", "reason": "back"})).unwrap();
    }
}
