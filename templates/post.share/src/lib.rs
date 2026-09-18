//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.8)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{err, fow_spec, host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported, Surface};
use std::sync::Mutex;

static PANEL: Surface = Surface::new("share-panel");
/// 開いている共有の (token, link)。閉じていれば None。
static CURRENT: Mutex<Option<(String, String)>> = Mutex::new(None);

fn close_and_drop() {
    if CURRENT.lock().unwrap().take().is_some() {
        PANEL.close(json!({}));
    }
}

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{token, post, account, link}` → 共有 FOW のインライン子として FOW を開く
        "post.share" => {
            let token = args["token"].as_str().unwrap_or("");
            if token.is_empty() {
                return Err(err("D01", "token required"));
            }
            let link = args["link"].as_str().unwrap_or("").to_string();
            let body = args["post"]["body"].as_str().unwrap_or("");
            let text = format!("「{}」\n{link}", body.chars().take(40).collect::<String>());
            let root = rrl::column("root", vec![rrl::text("preview", text.clone()), rrl::button("copy", "文面をコピー", "share.copy"), rrl::button("compose", "コンポーザーへ", "share.composer")]);
            let opened = PANEL.open(fow_spec("{{display}}", 0.4), root, Some("panelClosed"), json!({}));
            *CURRENT.lock().unwrap() = if opened { Some((token.to_string(), text)) } else { None };
            Ok(json!({}))
        }
        "share.copy" => {
            if let Some((_, text)) = CURRENT.lock().unwrap().clone() {
                host::clipboard_set(&text);
                host::toast("文面をコピーしました");
            }
            Ok(json!({}))
        }
        "share.composer" => {
            if let Some((token, text)) = CURRENT.lock().unwrap().clone() {
                host::composer_open(&token, &text, None, None, None);
            }
            Ok(json!({}))
        }
        "post.share.close" => {
            close_and_drop();
            Ok(json!({}))
        }
        "panelClosed" => {
            *CURRENT.lock().unwrap() = None;
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

    #[test]
    fn manifest_declares_slot() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["id"], json!("{{id}}"));
        assert_eq!(m["slots"], json!(["{{slot}}"]));
        assert_eq!(call_native("nope", json!({})).unwrap_err()["code"], json!("C01"));
    }

    #[test]
    fn share_then_copy() {
        set_host_hook(|f, _| if f == "fow.open" { json!({"opened": true}) } else { json!({}) });
        take_host_log();
        call_native("post.share", json!({"token": "s1", "post": {"body": "本文"}, "account": {}, "link": "https://example.com/p/1"})).unwrap();
        assert_eq!(take_host_log()[0].0, "fow.open");
        call_native("share.copy", json!({})).unwrap();
        let log = take_host_log();
        assert_eq!(log[0].1["text"], json!("「本文」\nhttps://example.com/p/1"));
        call_native("post.share.close", json!({"token": "s1"})).unwrap();
        assert_eq!(take_host_log()[0].0, "fow.close");
    }
}
