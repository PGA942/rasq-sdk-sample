//! {{name}} — `{{slot}}` の雛形(rasq-sdk-sample docs/slots.md §2.4)。
//!
//! `manifest.json` が `plugin.manifest` の答え。`handle` の match にこのスロットの call を書く。
use rasq_plugin_kit::{host, rasq_plugin, rrl, serde_json::{json, Value}, unsupported};

fn handle(fn_name: &str, args: Value) -> Result<Value, Value> {
    match fn_name {
        "plugin.manifest" => Ok(manifest()),
        "lifecycle" => Ok(json!({})),
        // `{token, url, mimeType, width, height, bytesB64?}` → `{root?: Stack}`(画像の画素系)
        "viewer.open" => {
            let (w, h) = (args["width"].as_f64().unwrap_or(0.0), args["height"].as_f64().unwrap_or(0.0));
            if w <= 0.0 || h <= 0.0 {
                return Ok(json!({}));
            }
            Ok(json!({"root": overlay(w, h)}))
        }
        "viewer.close" => Ok(json!({})),
        "mark.tap" => {
            host::toast("枠をタップしました");
            Ok(json!({}))
        }
        _ => Err(unsupported(fn_name)),
    }
}

/// 画像の中央に 1 つ枠(`Box`)を重ねる。`label` は合わせる先の一覧に出る名前。
fn overlay(w: f64, h: f64) -> Value {
    let mark = rrl::boxed(
        "mark",
        json!({"width": w / 2.0, "height": h / 2.0, "borderColor": "#00e5ff", "borderWidth": 3, "radius": 8, "label": "中央"}),
        vec![],
    );
    rrl::sized_stack("overlay", w, h, vec![rrl::with_on(rrl::positioned(mark, w / 4.0, h / 4.0), "tap", "mark.tap")])
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
    fn overlay_is_centered_box() {
        let r = call_native("viewer.open", json!({"token": "t", "url": "u", "mimeType": "image/png", "width": 400, "height": 200})).unwrap();
        let root = &r["root"];
        assert_eq!((root["t"].as_str(), root["p"]["width"].as_f64()), (Some("Stack"), Some(400.0)));
        let mark = &root["c"][0];
        assert_eq!((mark["p"]["left"].as_f64(), mark["p"]["top"].as_f64()), (Some(100.0), Some(50.0)));
        assert_eq!(mark["on"]["tap"], json!("mark.tap"));
        assert_eq!(call_native("viewer.open", json!({"width": 0})).unwrap(), json!({}));
    }
}
