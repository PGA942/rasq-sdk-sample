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
    }
    // 加工画面の下段バーのボタン宣言(docs/slots.md §2.2 v0.10)。無くても動くが、既定の見た目で画像にだけ出る。
    if slots.contains(&"composer.media") && !m["actions"].as_array().into_iter().flatten().any(|a| a["slot"] == "composer.media") {
        f.warnings.push("加工画面の下段バーのボタン宣言(actions[].slot = \"composer.media\")がありません(ホストが name と既定アイコンで、画像にだけ出す)".into());
    }
    // 全画面アクションバーのボタン宣言(docs/slots.md §2.4 v0.9)。無くても動くが、既定の見た目になる。
    if slots.contains(&"viewer.overlay") && !m["actions"].as_array().into_iter().flatten().any(|a| a["slot"] == "viewer.overlay") {
        f.warnings.push("全画面アクションバーのボタン宣言(actions[].slot = \"viewer.overlay\")がありません(ホストが name と既定アイコンで出す)".into());
    }
    if let (Some(cv), Some(mv)) = (cargo_version, m["version"].as_str()) {
        if cv != mv {
            f.warnings.push(format!("Cargo.toml の version {cv} と manifest の version {mv} が違います"));
        }
    }
    f
}
