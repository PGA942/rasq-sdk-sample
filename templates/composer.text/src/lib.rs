//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.3)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{fow_spec, host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported, Surface};
use std::sync::Mutex;

/// 候補にする語(ここを辞書 / 履歴に差し替える)。
const TAGS: &[&str] = &["雑談", "RASQ", "写真", "音楽", "ゲーム"];

static PANEL: Surface = Surface::new("suggest");

#[derive(Default)]
struct State {
    token: String,
    start: usize,
    end: usize,
    candidates: Vec<String>,
    open: bool,
}

static STATE: Mutex<State> = Mutex::new(State { token: String::new(), start: 0, end: 0, candidates: Vec::new(), open: false });

fn close_if_open() {
    let mut s = STATE.lock().unwrap();
    if s.open {
        PANEL.close(json!({}));
    }
    *s = State::default();
}

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{token, text, selectionStart, selectionEnd, trigger?}`。trigger = カーソル直前のトークン(UTF-16 単位)
        "text.changed" => {
            let token = args["token"].as_str().unwrap_or("").to_string();
            let trigger = args["trigger"].as_str().unwrap_or("");
            let cursor = args["selectionStart"].as_u64().unwrap_or(0) as usize;
            let Some(query) = trigger.strip_prefix('#').filter(|_| !token.is_empty()) else {
                close_if_open();
                return Ok(json!({}));
            };
            let candidates: Vec<String> = TAGS.iter().filter(|t| t.starts_with(query)).map(|t| t.to_string()).collect();
            if candidates.is_empty() {
                close_if_open();
                return Ok(json!({}));
            }
            let tiles = candidates.iter().enumerate().map(|(i, t)| rrl::list_tile(&format!("c{i}"), &format!("#{t}"), None, Some(&format!("pick.{i}")))).collect();
            let opened = PANEL.open(fow_spec("候補", 0.35), rrl::column("root", tiles), Some("panelClosed"), json!({}));
            let start = cursor.saturating_sub(trigger.encode_utf16().count());
            *STATE.lock().unwrap() = State { token, start, end: cursor, candidates, open: opened };
            Ok(json!({}))
        }
        f if f.starts_with("pick.") => {
            let i: usize = f["pick.".len()..].parse().map_err(|_| unsupported(f))?;
            let picked = {
                let s = STATE.lock().unwrap();
                s.candidates.get(i).map(|t| (s.token.clone(), s.start, s.end, t.clone()))
            };
            if let Some((token, start, end, tag)) = picked {
                host::composer_text_replace(&token, start, end, &format!("#{tag} "));
                close_if_open();
                host::toast(&format!("#{tag} を挿入しました"));
            }
            Ok(json!({}))
        }
        "panelClosed" => {
            *STATE.lock().unwrap() = State::default();
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
    fn hashtag_trigger_opens_and_pick_replaces() {
        set_host_hook(|f, _| if f == "fow.open" { json!({"opened": true}) } else { json!({}) });
        take_host_log();
        call_native("text.changed", json!({"token": "t", "text": "きょうは #写", "selectionStart": 6, "selectionEnd": 6, "trigger": "#写"})).unwrap();
        let log = take_host_log();
        assert_eq!(log[0].0, "fow.open");
        assert_eq!(log[0].1["root"]["c"][0]["p"]["title"], json!("#写真"));
        call_native("pick.0", json!({})).unwrap();
        let log = take_host_log();
        let rep = log.iter().find(|(f, _)| f == "composer.text.replace").unwrap();
        assert_eq!(rep.1, json!({"token": "t", "start": 4, "end": 6, "text": "#写真 "}));
        assert!(log.iter().any(|(f, _)| f == "fow.close"));
    }
}
