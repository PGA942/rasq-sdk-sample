//! マニフェストの検査(docs/cli.md §5 の 4): `manifest.schema.json` + 登録簿の突き合わせ(`schema/validate.py` と同じ規則)。

use crate::embed::{self, Slot};
use anyhow::{Context, Result};
use jsonschema::{Resource, Validator};
use serde_json::Value;

/// 同梱スキーマをすべて `https://rasq.io/schema/<name>` と `$id` で登録して、`manifest.schema.json` の検証器を作る。
pub fn manifest_validator() -> Result<Validator> {
    validator("manifest.schema.json")
}

/// 同梱スキーマ `name` の検証器(相対 $ref は同梱の他スキーマで解決)。
pub fn validator(name: &str) -> Result<Validator> {
    let mut docs: Vec<(String, Value)> = Vec::new();
    for f in embed::SCHEMA.files() {
        let name = f.path().file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".schema.json") {
            docs.push((name.clone(), serde_json::from_slice(f.contents()).with_context(|| format!("schema/{name}"))?));
        }
    }
    // 相対 $ref は参照元の $id を基底に解決される。基底(https://rasq.io/schema/ など)がスキーマごとに違っても
    // 引けるよう、全スキーマを全基底の下に登録する(validate.py の registry() と同じ)。
    let mut bases: Vec<String> = docs.iter().filter_map(|(_, d)| d["$id"].as_str()).map(|id| format!("{}/", id.rsplit_once('/').map(|(b, _)| b).unwrap_or(id))).collect();
    bases.push("https://rasq.io/schema/".into());
    bases.sort();
    bases.dedup();
    let mut opts = jsonschema::options();
    for (name, doc) in &docs {
        let res = Resource::from_contents(doc.clone()).map_err(|e| anyhow::anyhow!("schema/{name}: {e}"))?;
        for b in &bases {
            opts = opts.with_resource(format!("{b}{name}"), res.clone());
        }
        if let Some(id) = doc["$id"].as_str() {
            opts = opts.with_resource(id.to_string(), res);
        }
    }
    let schema = embed::schema_json(name)?;
    opts.build(&schema).map_err(|e| anyhow::anyhow!("{name} を組めません: {e}"))
}

/// スキーマ違反を文字列で(先頭 8 件)。
pub fn violations(v: &Validator, doc: &Value) -> Vec<String> {
    v.iter_errors(doc)
        .take(8)
        .map(|e| {
            let path = e.instance_path.to_string();
            format!("{}: {}", if path.is_empty() { "(root)".into() } else { path }, e)
        })
        .collect()
}

pub struct Findings {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// スキーマ + 登録簿。`declared_slot` = rasq.toml の slot(sns は None)。
pub fn check_manifest(v: &Validator, registry: &[Slot], m: &Value, declared_slot: Option<&str>, cargo_version: Option<&str>) -> Findings {
    let mut f = Findings { errors: vec![], warnings: vec![] };
    for e in v.iter_errors(m).take(8) {
        let path = e.instance_path.to_string();
        f.errors.push(format!("schema: {}: {}", if path.is_empty() { "(root)".into() } else { path }, e));
    }
    let kind = m["kind"].as_str().unwrap_or("");
    let perms: Vec<&str> = m["permissions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let slots: Vec<&str> = m["slots"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    for sid in &slots {
        let Some(s) = registry.iter().find(|s| s.id == *sid) else {
            f.errors.push(format!("slot {sid} は登録簿にありません"));
            continue;
        };
        if s.status == "planned" {
            f.warnings.push(format!("slot {sid} は planned(まだホストが無い)"));
        }
        if !s.kinds.iter().any(|k| k == kind) {
            f.errors.push(format!("kind {kind} は slot {sid} では使えません({:?})", s.kinds));
        }
        let missing: Vec<&str> = s.required.iter().map(String::as_str).filter(|p| !perms.contains(p)).collect();
        if !missing.is_empty() {
            f.errors.push(format!("slot {sid} に必要な権限が足りません: {missing:?}"));
        }
    }
    if let Some(d) = declared_slot {
        if !slots.contains(&d) {
            f.errors.push(format!("rasq.toml の slot {d} が manifest.slots {slots:?} にありません"));
        }
    }
    for a in m["actions"].as_array().into_iter().flatten() {
        if a["id"].as_str().is_some_and(|id| id.contains(':')) {
            f.errors.push(format!("action id に ':' は使えません: {}", a["id"]));
        }
        if let Some(slot) = a["slot"].as_str() {
            if !slots.contains(&slot) {
                f.errors.push(format!("action {} の slot {slot} が manifest.slots {slots:?} にありません", a["id"]));
            }
        }
        if !a["output"].is_null() && a["slot"] != "composer.media" {
            f.errors.push(format!("action {} の output は slot = \"composer.media\" の宣言にだけ書けます", a["id"]));
        }
        if !a["accepts"].is_null() && a["slot"] != "composer.media" {
            f.errors.push(format!("action {} の accepts は slot = \"composer.media\" の宣言にだけ書けます", a["id"]));
        }
        // ボタンの共通属性(docs/slots.md §2.11 v0.14): auto(初期状態 = temp)と placement。
        let slot = a["slot"].as_str().unwrap_or("post.action");
        if a["auto"] == true {
            if a["destructive"] == true || a["autoHookForbidden"] == true {
                f.errors.push(format!("action {} は destructive / autoHookForbidden なので auto: true(自動で動く)にできません", a["id"]));
            }
            if slot == "post.action" && !perms.contains(&"auto_hook") {
                f.errors.push(format!("action {} の auto: true(タイルごとに自動で実行)には permissions に auto_hook が要ります", a["id"]));
            }
        }
        // composer.media で初期状態を持つのはフィルターだけ。replace / sticker には true でも false でも書けない。
        if !a["auto"].is_null() && slot == "composer.media" && a["output"] != "filter" {
            f.errors.push(format!("action {} の auto は output = \"filter\" の宣言にだけ書けます(replace / sticker は状態を持たない)", a["id"]));
        }
        if a["placement"] == "manual" && a["tags"].is_null() && a["genreTag"].is_null() {
            f.warnings.push(format!("action {} は placement: manual で tags が無いので、検索でタグでは見つかりません(label だけ)", a["id"]));
        }
    }
    // auto_hook を持つのに auto: true の post.action が無い = 過剰宣言(審査の許容の枠内で数えられる)。
    if perms.contains(&"auto_hook")
        && !m["actions"].as_array().into_iter().flatten().any(|a| a["auto"] == true && a["slot"].as_str().unwrap_or("post.action") == "post.action")
    {
        f.warnings.push("permissions に auto_hook がありますが、auto: true の post.action のボタンがありません(過剰宣言として数えられます)".into());
    }
    // 加工画面の下段バーのボタン宣言(docs/slots.md §2.2 v0.10)。無くても動くが、既定の見た目で画像にだけ出る。
    if slots.contains(&"composer.media") && !m["actions"].as_array().into_iter().flatten().any(|a| a["slot"] == "composer.media") {
        f.warnings.push("加工画面の下段バーのボタン宣言(actions[].slot = \"composer.media\")がありません(ホストが name と既定アイコンで、画像にだけ出す)".into());
    }
    // 全画面アクションバーのボタン宣言(docs/slots.md §2.4 v0.9)。無くても動くが、既定の見た目になる。
    if slots.contains(&"viewer.overlay") && !m["actions"].as_array().into_iter().flatten().any(|a| a["slot"] == "viewer.overlay") {
        f.warnings.push("全画面アクションバーのボタン宣言(actions[].slot = \"viewer.overlay\")がありません(ホストが name と既定アイコンで出す)".into());
    }
    // ページ型(docs/slots.md §2.13 v0.18)。page が要ること / shared なら shared.session / astell は shared だけ、はスキーマが見る。
    if m["page"]["mode"] == "solo" {
        if perms.contains(&"shared.session") {
            f.warnings.push("page.mode = solo なのに permissions に shared.session があります(solo はセッションに入らないので過剰宣言)".into());
        }
        if !m["page"]["maxParticipants"].is_null() {
            f.warnings.push("page.maxParticipants は shared のときだけ意味があります".into());
        }
    }
    if let (Some(cv), Some(mv)) = (cargo_version, m["version"].as_str()) {
        if cv != mv {
            f.warnings.push(format!("Cargo.toml の version {cv} と manifest の version {mv} が違います"));
        }
    }
    f
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(m: Value) -> Findings {
        let v = manifest_validator().unwrap();
        let reg = crate::embed::registry().unwrap();
        check_manifest(&v, &reg, &m, None, None)
    }

    fn post_action(action: Value, perms: &[&str]) -> Value {
        json!({
            "schema": "rasq.plugin-manifest/1", "id": "sample.t", "name": "t", "version": "0.1.0", "kind": "action",
            "slots": ["post.action"], "permissions": perms, "actions": [action]
        })
    }

    #[test]
    fn auto_on_post_action_needs_auto_hook_and_is_refused_for_destructive_buttons() {
        // 権限が無い → error(スキーマと rasq check の両方が言う)。
        let f = check(post_action(json!({"id": "a", "label": "a", "tags": ["Copy"], "auto": true}), &["fow.widget"]));
        assert!(f.errors.iter().any(|e| e.contains("auto_hook")), "{:?}", f.errors);
        // 権限があれば通る。
        let f = check(post_action(json!({"id": "a", "label": "a", "tags": ["Copy"], "auto": true}), &["fow.widget", "auto_hook"]));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        // 壊す操作は自動で動かせない。
        let f = check(post_action(json!({"id": "a", "label": "a", "tags": ["Copy"], "auto": true, "destructive": true}), &["fow.widget", "auto_hook"]));
        assert!(f.errors.iter().any(|e| e.contains("destructive")), "{:?}", f.errors);
    }

    #[test]
    fn auto_hook_without_an_auto_button_is_an_overdeclaration_warning() {
        let f = check(post_action(json!({"id": "a", "label": "a", "tags": ["Copy"]}), &["fow.widget", "auto_hook"]));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.iter().any(|w| w.contains("auto_hook")), "{:?}", f.warnings);
    }

    #[test]
    fn manual_placement_allows_omitting_tags_but_warns() {
        let f = check(post_action(json!({"id": "a", "label": "a", "placement": "manual"}), &["fow.widget"]));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.iter().any(|w| w.contains("manual")), "{:?}", f.warnings);
        // tags も placement も無いのは従来どおりスキーマ違反。
        let f = check(post_action(json!({"id": "a", "label": "a"}), &["fow.widget"]));
        assert!(!f.errors.is_empty());
    }

    #[test]
    fn composer_media_auto_is_only_for_filters() {
        let base = |action: Value| {
            json!({
                "schema": "rasq.plugin-manifest/1", "id": "sample.t", "name": "t", "version": "0.1.0", "kind": "action",
                "slots": ["composer.media"], "permissions": ["fow.widget"], "actions": [action]
            })
        };
        let f = check(base(json!({"id": "g", "slot": "composer.media", "label": "g", "tags": ["Filter"], "output": "filter", "auto": true})));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        let f = check(base(json!({"id": "g", "slot": "composer.media", "label": "g", "tags": ["Filter"], "auto": true})));
        assert!(f.errors.iter().any(|e| e.contains("filter")), "{:?}", f.errors);
        // false でも同じ(状態を持たないボタンに初期状態は書けない)。
        let f = check(base(json!({"id": "g", "slot": "composer.media", "label": "g", "tags": ["Filter"], "auto": false})));
        assert!(f.errors.iter().any(|e| e.contains("filter")), "{:?}", f.errors);
    }

    #[test]
    fn page_mode_rules() {
        let v = manifest_validator().unwrap();
        let reg = crate::embed::registry().unwrap();
        let base = |perms: Value, page: Value| {
            json!({
                "schema": "rasq.plugin-manifest/1", "id": "sample.t", "name": "t", "version": "0.1.0", "kind": "view",
                "slots": ["page"], "permissions": perms, "page": page
            })
        };
        let ok = check_manifest(&v, &reg, &base(json!(["render", "shared.session"]), json!({"mode": "shared", "surfaces": ["page", "astell"]})), Some("page"), None);
        assert!(ok.errors.is_empty() && ok.warnings.is_empty(), "{:?} {:?}", ok.errors, ok.warnings);
        // shared には shared.session が要る
        let f = check_manifest(&v, &reg, &base(json!(["render"]), json!({"mode": "shared"})), Some("page"), None);
        assert!(!f.errors.is_empty());
        // astell は shared だけ
        let f = check_manifest(&v, &reg, &base(json!(["render"]), json!({"mode": "solo", "surfaces": ["astell"]})), Some("page"), None);
        assert!(!f.errors.is_empty());
        // v0.20: swing は shared でも書ける(v0.19 の solo だけの決まりを外した)
        let f = check_manifest(&v, &reg, &base(json!(["render", "shared.session"]), json!({"mode": "shared", "surfaces": ["page", "swing"]})), Some("page"), None);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        // slots に page があれば page が要る
        let mut m = base(json!(["render"]), json!({"mode": "solo"}));
        m.as_object_mut().unwrap().remove("page");
        assert!(!check_manifest(&v, &reg, &m, Some("page"), None).errors.is_empty());
        // solo + shared.session は警告
        let f = check_manifest(&v, &reg, &base(json!(["render", "shared.session"]), json!({"mode": "solo"})), Some("page"), None);
        assert!(f.errors.is_empty() && f.warnings.iter().any(|w| w.contains("shared.session")), "{:?} {:?}", f.errors, f.warnings);
    }

    #[test]
    fn bundled_templates_pass_the_check() {
        let v = manifest_validator().unwrap();
        let reg = crate::embed::registry().unwrap();
        for name in ["post.action", "viewer.overlay", "composer.media", "page"] {
            let raw = crate::embed::TEMPLATES.get_file(format!("{name}/manifest.json")).unwrap().contents_utf8().unwrap();
            let m: Value = serde_json::from_str(&raw.replace("{{id}}", "sample.t").replace("{{display}}", "t")).unwrap();
            let f = check_manifest(&v, &reg, &m, Some(name), None);
            assert!(f.errors.is_empty(), "{name}: {:?}", f.errors);
        }
    }
}
