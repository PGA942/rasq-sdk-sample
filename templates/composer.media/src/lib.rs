//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.2)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{err, host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported};
use std::sync::Mutex;

/// 編集中の 1 枚(`media.edit` 〜 `media.commit` / `media.cancel`)。
#[derive(Default, Clone)]
struct Session {
    token: String,
    name: String,
    mime: String,
    bytes_b64: String,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// `media.catalog` / `media.edit` の答え(静的な一覧)。ジャンル id は契約の既定(tone / process / transform / add / draw)。
fn catalog() -> Value {
    json!({"genres": [{"id": "process", "actions": [{"id": "sample", "label": "サンプル処理", "icon": "auto_fix_high"}]}]})
}

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        "media.catalog" => Ok(catalog()),
        // `{token, index, name, mimeType, width?, height?, bytesB64}` → `{genres}`(エディター形)
        "media.edit" => {
            let mime = args["mimeType"].as_str().unwrap_or("");
            if mime != "image/png" && mime != "image/jpeg" {
                return Err(err("C01", format!("unsupported mimeType: {mime}")));
            }
            let s = Session {
                token: args["token"].as_str().unwrap_or("").to_string(),
                name: args["name"].as_str().unwrap_or("image").to_string(),
                mime: mime.to_string(),
                bytes_b64: args["bytesB64"].as_str().ok_or_else(|| err("D01", "bytesB64 required"))?.to_string(),
            };
            host::composer_media_preview(&s.token, &s.bytes_b64, &s.mime, None);
            *SESSION.lock().unwrap() = Some(s);
            Ok(catalog())
        }
        // 第 2 階層のボタン。ここに処理を書き、結果を `composer.media.preview` で見せる
        "media.action" => {
            let s = SESSION.lock().unwrap().clone().ok_or_else(|| err("D01", "no session"))?;
            match args["action"].as_str().unwrap_or("") {
                "sample" => {
                    // TODO: s.bytes_b64 を復号して処理し、結果をプレビューへ
                    host::composer_media_preview(&s.token, &s.bytes_b64, &s.mime, None);
                    host::toast("サンプル処理(未実装)を適用しました");
                    Ok(json!({}))
                }
                other => Err(err("D01", format!("unknown action: {other}"))),
            }
        }
        // 適用: 本サイズの結果を戻りで返す
        "media.commit" => {
            let s = SESSION.lock().unwrap().take().ok_or_else(|| err("D01", "no session"))?;
            Ok(json!({"bytesB64": s.bytes_b64, "mimeType": s.mime, "name": s.name}))
        }
        "media.cancel" => {
            *SESSION.lock().unwrap() = None;
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
    use rasq_plugin_kit::take_host_log;

    #[test]
    fn manifest_declares_slot() {
        let m = call_native("plugin.manifest", json!({})).unwrap();
        assert_eq!(m["id"], json!("{{id}}"));
        assert_eq!(m["slots"], json!(["{{slot}}"]));
        assert_eq!(call_native("nope", json!({})).unwrap_err()["code"], json!("C01"));
    }

    #[test]
    fn edit_preview_commit() {
        take_host_log();
        let r = call_native("media.edit", json!({"token": "t1", "name": "a.png", "mimeType": "image/png", "bytesB64": "AAAA"})).unwrap();
        assert_eq!(r, catalog());
        let log = take_host_log();
        assert_eq!(log[0].0, "composer.media.preview");
        assert_eq!(log[0].1["token"], json!("t1"));
        call_native("media.action", json!({"token": "t1", "genre": "process", "action": "sample"})).unwrap();
        let out = call_native("media.commit", json!({"token": "t1"})).unwrap();
        assert_eq!(out["name"], json!("a.png"));
        assert_eq!(call_native("media.commit", json!({"token": "t1"})).unwrap_err()["code"], json!("D01"));
        assert_eq!(call_native("media.edit", json!({"mimeType": "image/gif", "bytesB64": "x"})).unwrap_err()["code"], json!("C01"));
    }
}
