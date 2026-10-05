//! アセットパック(docs/slots.md §2.10、契約 v0.11)と着せ替え(§2.12、v0.16): wasm を持たない配布物。
//! `packs/<name>/manifest.json`(kind = "assets" / "theme")と画像。
//! ホストは画像と宣言を読むだけなので、ここでは「画像であること」と大きさ、着せ替えなら宣言の参照先があることを確かめる。

use crate::pkg::{self, Entry, Package};
use crate::schema;
use crate::workspace::PackEntry;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::path::Path;

/// 1 枚の上限。
pub const MAX_FILE: usize = 2 * 1024 * 1024;
/// パック全体の上限。
pub const MAX_TOTAL: usize = 16 * 1024 * 1024;

/// packs/ に置ける kind(wasm を持たない配布物)。
pub const PACK_KINDS: [&str; 2] = ["assets", "theme"];

/// マジックナンバーから画像の形式。
pub fn image_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

fn ext_kind(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => Some("png"),
        Some("jpg") | Some("jpeg") => Some("jpeg"),
        Some("webp") => Some("webp"),
        Some("gif") => Some("gif"),
        _ => None,
    }
}

pub struct Checked {
    pub manifest: Value,
    pub manifest_bytes: Vec<u8>,
    /// (パックからの相対パス, 中身)
    pub files: Vec<(String, Vec<u8>)>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// スキーマ → id / kind → 各画像(実在 / 形式 / 大きさ / 重複)→ 書き忘れ → (着せ替えなら)宣言の参照先。
pub fn check(dir: &Path, entry: &PackEntry) -> Result<Checked> {
    let manifest_bytes = std::fs::read(dir.join("manifest.json")).with_context(|| format!("{}/manifest.json を読めません", dir.display()))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes).context("manifest.json が JSON ではありません")?;
    let mut errors = schema::violations(&schema::manifest_validator()?, &manifest).into_iter().map(|e| format!("schema: {e}")).collect::<Vec<_>>();
    let mut warnings = Vec::new();
    if !manifest["kind"].as_str().is_some_and(|k| PACK_KINDS.contains(&k)) {
        errors.push(format!("packs/ の manifest は kind = \"assets\" か \"theme\" でなければなりません: {}", manifest["kind"]));
    }
    if manifest["id"].as_str() != Some(entry.id.as_str()) {
        errors.push(format!("manifest.id {} が rasq.toml の id {} と違います", manifest["id"], entry.id));
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for a in manifest["assets"].as_array().into_iter().flatten() {
        let (Some(id), Some(path)) = (a["id"].as_str(), a["path"].as_str()) else { continue };
        if !ids.insert(id.to_string()) {
            errors.push(format!("asset id が重複しています: {id}"));
        }
        if files.iter().any(|(p, _)| p == path) {
            errors.push(format!("同じファイルを 2 回書いています: {path}"));
            continue;
        }
        if path.split('/').any(|s| s == ".." || s.is_empty()) {
            errors.push(format!("{path}: パックの外は指せません"));
            continue;
        }
        let bytes = match std::fs::read(dir.join(path)) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{path}: {e}"));
                continue;
            }
        };
        match (image_kind(&bytes), ext_kind(path)) {
            (None, _) => errors.push(format!("{path}: PNG / JPEG / WebP / GIF ではありません(中身で判定)")),
            (Some(k), Some(e)) if k != e => errors.push(format!("{path}: 拡張子は {e} なのに中身は {k}")),
            _ => {}
        }
        if bytes.len() > MAX_FILE {
            errors.push(format!("{path}: {} bytes(1 枚 {} MiB まで)", bytes.len(), MAX_FILE / 1024 / 1024));
        }
        total += bytes.len();
        files.push((path.to_string(), bytes));
    }
    if total > MAX_TOTAL {
        errors.push(format!("合計 {total} bytes(パック全体で {} MiB まで)", MAX_TOTAL / 1024 / 1024));
    }
    // 書き忘れ: ディレクトリにある画像で assets に無いもの。
    let mut on_disk = Vec::new();
    walk(dir, dir, &mut on_disk);
    for p in on_disk {
        if ext_kind(&p).is_some() && !files.iter().any(|(f, _)| *f == p) {
            warnings.push(format!("{p}: manifest.assets に書かれていない画像(配布物には入りません)"));
        }
    }
    if manifest["kind"] == "theme" {
        let (e, w) = theme_problems(&manifest);
        errors.extend(e);
        warnings.extend(w);
    }
    Ok(Checked { manifest, manifest_bytes, files, errors, warnings })
}

/// 着せ替え(docs/slots.md §2.12)の整合。スキーマでは書けない「id の参照先がある」と、グラデーション(v0.17)の
/// 「stops が colors と同じ数で昇順」を見る(schema/validate.py の theme_problems と同じ規則)。色の形 / 面のキー /
/// layers の数 / asset と gradient のどちらか 1 つ はスキーマが見る。戻りは (errors, warnings)。
pub fn theme_problems(m: &Value) -> (Vec<String>, Vec<String>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    let theme_ids: std::collections::BTreeSet<&str> =
        m["assets"].as_array().into_iter().flatten().filter(|a| a["use"] == "theme").filter_map(|a| a["id"].as_str()).collect();
    let mut used = std::collections::BTreeSet::new();
    for (name, surface) in m["theme"]["surfaces"].as_object().into_iter().flatten() {
        for (i, layer) in surface["layers"].as_array().into_iter().flatten().enumerate() {
            let at = format!("theme.surfaces.{name}.layers[{i}]");
            if let Some(id) = layer["asset"].as_str() {
                used.insert(id);
                if !theme_ids.contains(id) {
                    errors.push(format!("{at}.asset {id:?} が assets(use = theme)にありません"));
                }
            }
            // グラデーションは参照先を持たない。stops は colors と同じ数、昇順(同じ値の並びは可)。
            let g = &layer["gradient"];
            if g.is_object() {
                if let Some(stops) = g["stops"].as_array() {
                    let colors = g["colors"].as_array().map_or(0, Vec::len);
                    if stops.len() != colors {
                        errors.push(format!("{at}.gradient.stops が {} 個、colors が {colors} 個(同じ数にする)", stops.len()));
                    }
                    let xs: Vec<f64> = stops.iter().filter_map(Value::as_f64).collect();
                    if xs.windows(2).any(|w| w[0] > w[1]) {
                        errors.push(format!("{at}.gradient.stops が昇順ではありません: {xs:?}"));
                    }
                }
                for k in ["fit", "repeat"] {
                    if !layer[k].is_null() {
                        warnings.push(format!("{at}.{k} はグラデーションでは使われません(画像のときだけ)"));
                    }
                }
            }
        }
    }
    for (icon, r) in m["theme"]["icons"].as_object().into_iter().flatten() {
        let Some(id) = r.as_str().and_then(|r| r.strip_prefix("asset:")) else { continue };
        used.insert(id);
        if !theme_ids.contains(id) {
            errors.push(format!("theme.icons.{icon} の \"asset:{id}\" が assets(use = theme)にありません"));
        }
    }
    for id in theme_ids.difference(&used) {
        warnings.push(format!("asset {id} は theme のどこからも指されていません(配布物には入るが使われない)"));
    }
    (errors, warnings)
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(base, &p, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// パック 1 つ = 配布物 1 つ(root.kind = assets / theme = manifest の kind、wasm 無し)。
pub fn package(c: &Checked) -> Result<Package> {
    let root = json!({"id": c.manifest["id"], "version": c.manifest["version"], "kind": c.manifest["kind"], "manifest": "manifest.json"});
    let mut entries = vec![Entry { path: "manifest.json".into(), bytes: c.manifest_bytes.clone() }];
    let mut assets = Vec::new();
    for (path, bytes) in &c.files {
        assets.push(json!({"path": path, "kind": "image", "sha512": pkg::hex(&Sha512::digest(bytes)), "sizeBytes": bytes.len()}));
        entries.push(Entry { path: path.clone(), bytes: bytes.clone() });
    }
    pkg::assemble_with(root, vec![], vec![], assets, vec![], entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed;

    /// 雛形を一時ディレクトリに展開する(`rasq new assets|theme` と同じ)。
    fn scratch(tag: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("rasq-pack-test-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn from_template(kind: &str) -> (std::path::PathBuf, PackEntry) {
        let dir = scratch(kind);
        embed::render_template(kind, &dir, &[("id", "sample.p"), ("name", "p"), ("display", "p")]).unwrap();
        (dir, PackEntry { name: "p".into(), id: "sample.p".into() })
    }

    fn edit(dir: &Path, f: impl FnOnce(&mut Value)) {
        let path = dir.join("manifest.json");
        let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        f(&mut m);
        std::fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    }

    #[test]
    fn bundled_pack_templates_pass_the_check_and_package_without_wasm() {
        for kind in PACK_KINDS {
            let (dir, entry) = from_template(kind);
            let c = check(&dir, &entry).unwrap();
            assert!(c.errors.is_empty(), "{kind}: {:?}", c.errors);
            assert!(c.warnings.is_empty(), "{kind}: {:?}", c.warnings);
            assert_eq!(c.files.len(), 1, "{kind}");
            let p = package(&c).unwrap();
            assert_eq!(p.bundle["root"]["kind"], kind);
            assert!(p.bundle["root"]["wasm"].is_null());
            assert_eq!(p.bundle["assets"][0]["kind"], "image");
            assert_eq!(p.file_name(crate::pkg::EXT), "sample.p-0.1.0.rspkg");
            assert_eq!(p.file_name(crate::pkg::LEGACY_EXT), "sample.p-0.1.0.rasqpkg");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn theme_references_must_point_at_theme_assets() {
        let (dir, entry) = from_template("theme");
        edit(&dir, |m| {
            m["theme"]["surfaces"]["postTile"]["layers"] = json!([{ "asset": "nope", "position": "above", "repeat": "repeat" }]);
            m["theme"]["icons"]["favorite_border"] = json!("asset:heart");
        });
        let c = check(&dir, &entry).unwrap();
        assert!(c.errors.iter().any(|e| e.contains("theme.surfaces.postTile.layers[0].asset \"nope\"")), "{:?}", c.errors);
        assert!(c.errors.iter().any(|e| e.contains("theme.icons.favorite_border")), "{:?}", c.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gradient_layers_need_no_asset_but_stops_must_match_colors() {
        let (dir, entry) = from_template("theme");
        // 画像を指さないグラデーションだけのレイヤー(と帯の height、面の onColor)は通る。
        edit(&dir, |m| {
            m["theme"]["surfaces"]["postTile"]["layers"] =
                json!([{ "gradient": { "colors": ["#1F1E33", "#001F1E33"] }, "alignment": "topCenter", "height": 28 }]);
            m["theme"]["surfaces"]["fow"]["onColor"] = json!("#FFFFFF");
            m["theme"]["surfaces"]["fow"]["layers"] =
                json!([{ "gradient": { "colors": ["#02775C", "#178178", "#FFFFFF"], "stops": [0, 0.5, 0.5], "begin": "topLeft", "end": "bottomRight" } }]);
        });
        let c = check(&dir, &entry).unwrap();
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        // stops の数が違う / 昇順でない = error。fit / repeat はグラデーションでは使われない = warning。
        edit(&dir, |m| {
            m["theme"]["surfaces"]["fow"]["layers"] = json!([
                { "gradient": { "colors": ["#02775C", "#178178", "#FFFFFF"], "stops": [0, 1] } },
                { "gradient": { "colors": ["#02775C", "#178178"], "stops": [0.8, 0.2] }, "fit": "cover" }
            ]);
        });
        let c = check(&dir, &entry).unwrap();
        assert!(c.errors.iter().any(|e| e.contains("layers[0].gradient.stops が 2 個、colors が 3 個")), "{:?}", c.errors);
        assert!(c.errors.iter().any(|e| e.contains("layers[1].gradient.stops が昇順ではありません")), "{:?}", c.errors);
        assert!(c.warnings.iter().any(|w| w.contains("layers[1].fit")), "{:?}", c.warnings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_layer_is_either_an_asset_or_a_gradient() {
        let (dir, entry) = from_template("theme");
        let g = json!({ "colors": ["#000000", "#FFFFFF"] });
        for (layer, bad) in [
            (json!({ "asset": "background", "gradient": g }), "両方"),
            (json!({ "position": "below" }), "どちらも無し"),
            (json!({ "gradient": { "colors": ["#000000"] } }), "色 1 つ"),
            (json!({ "gradient": g, "height": 401 }), "height が 400 超"),
        ] {
            edit(&dir, |m| m["theme"]["surfaces"]["app"]["layers"] = json!([layer]));
            let c = check(&dir, &entry).unwrap();
            assert!(c.errors.iter().any(|e| e.starts_with("schema:")), "{bad}: {:?}", c.errors);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_shape_is_checked_by_the_schema() {
        let (dir, entry) = from_template("theme");
        edit(&dir, |m| {
            m["theme"]["colors"]["dark"]["primary"] = json!("#123"); // 3 桁は不可
            m["theme"]["surfaces"]["header"] = json!({}); // 今の面は 4 つだけ
            m["slots"] = json!(["post.action"]); // コードに関わる宣言は書けない
        });
        let c = check(&dir, &entry).unwrap();
        let all = c.errors.join("\n");
        assert!(all.contains("#123"), "{all}");
        assert!(all.contains("header"), "{all}");
        assert!(all.contains("/slots"), "{all}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_colour_only_theme_needs_no_images_but_unused_images_warn() {
        let (dir, entry) = from_template("theme");
        edit(&dir, |m| {
            m["theme"]["surfaces"]["app"]["layers"] = json!([]);
        });
        let c = check(&dir, &entry).unwrap();
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(c.warnings.iter().any(|w| w.contains("background")), "{:?}", c.warnings);
        // 画像ゼロ(色だけ)も配布物になる。
        edit(&dir, |m| m["assets"] = json!([]));
        std::fs::remove_file(dir.join("theme/background.png")).unwrap();
        let c = check(&dir, &entry).unwrap();
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let p = package(&c).unwrap();
        assert_eq!(p.bundle["assets"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_is_refused_outside_a_theme_pack_and_packs_refuse_code_kinds() {
        let (dir, entry) = from_template("assets");
        edit(&dir, |m| m["theme"] = json!({}));
        let c = check(&dir, &entry).unwrap();
        assert!(!c.errors.is_empty());
        edit(&dir, |m| {
            m.as_object_mut().unwrap().remove("theme");
            m["kind"] = json!("action");
        });
        let c = check(&dir, &entry).unwrap();
        assert!(c.errors.iter().any(|e| e.contains("\"theme\"")), "{:?}", c.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
