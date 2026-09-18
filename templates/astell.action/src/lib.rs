//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.1)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{err, fow_spec, host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported, Surface};
use std::sync::Mutex;

const TEXTS: &[&str] = &["👏👏👏", "いいね!"];

static PANEL: Surface = Surface::new("astell-panel");
/// 開いているルームの token(閉じていれば空)。
static TOKEN: Mutex<String> = Mutex::new(String::new());

fn panel_root(room_title: &str) -> Value {
    let mut c = vec![rrl::text("hdr", format!("{{display}} — {room_title}"))];
    c.extend(TEXTS.iter().enumerate().map(|(i, t)| rrl::button(&format!("b{i}"), t, &format!("text.{i}"))));
    rrl::column("root", c)
}

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{token, room, me, peers, shares}` → FOW を開く
        "astell.open" => {
            let token = args["token"].as_str().unwrap_or("");
            if token.is_empty() {
                return Err(err("D01", "token required"));
            }
            let title = args["room"]["title"].as_str().unwrap_or("ルーム");
            let opened = PANEL.open(fow_spec("{{display}}", 0.4), panel_root(title), Some("panelClosed"), json!({}));
            *TOKEN.lock().unwrap() = if opened { token.to_string() } else { String::new() };
            Ok(json!({}))
        }
        // `{token, type, ...}`: share / transcript / peer-joined / peer-left / closed
        "astell.event" => {
            if args["type"] == "closed" {
                let mut t = TOKEN.lock().unwrap();
                if !t.is_empty() {
                    PANEL.close(json!({}));
                    t.clear();
                }
            }
            Ok(json!({}))
        }
        f if f.starts_with("text.") => {
            let i: usize = f["text.".len()..].parse().map_err(|_| unsupported(f))?;
            let t = TEXTS.get(i).ok_or_else(|| unsupported(f))?;
            host::astell_share("text", json!({"text": t}));
            Ok(json!({}))
        }
        "panelClosed" => {
            TOKEN.lock().unwrap().clear();
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
    fn open_share_close() {
        set_host_hook(|f, _| if f == "fow.open" { json!({"opened": true}) } else { json!({}) });
        take_host_log();
        call_native("astell.open", json!({"token": "r1", "room": {"title": "雑談"}, "me": {}, "peers": [], "shares": []})).unwrap();
        assert_eq!(take_host_log()[0].0, "fow.open");
        call_native("text.1", json!({})).unwrap();
        let log = take_host_log();
        assert_eq!(log[0].1, json!({"kind": "text", "payload": {"text": "いいね!"}}));
        call_native("astell.event", json!({"token": "r1", "type": "closed"})).unwrap();
        assert_eq!(take_host_log()[0].0, "fow.close");
        assert_eq!(call_native("astell.open", json!({})).unwrap_err()["code"], json!("D01"));
    }
}
