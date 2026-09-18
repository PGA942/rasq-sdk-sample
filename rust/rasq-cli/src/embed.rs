//! バイナリに同梱するもの: `schema/*.json`(登録簿とスキーマ)、`templates/<slot>/`(雛形)、`rust/rasq-plugin-kit/`(kit)。
//! ネット無しで `init / new / check / build` が完結する(docs/cli.md D-244a)。

use anyhow::{anyhow, Context, Result};
use include_dir::{include_dir, Dir};
use serde_json::Value;
use std::path::Path;

pub static SCHEMA: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../schema");
pub static TEMPLATES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../templates");
pub static KIT: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../rasq-plugin-kit");

/// 同梱 SDK の版(kit と雛形の版でもある)。
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn schema_json(name: &str) -> Result<Value> {
    let f = SCHEMA.get_file(name).ok_or_else(|| anyhow!("同梱スキーマがありません: {name}"))?;
    serde_json::from_slice(f.contents()).with_context(|| format!("schema/{name} を読めません"))
}

/// 登録簿(`schema/slots.json`)の 1 スロット。
#[derive(Debug, Clone)]
pub struct Slot {
    pub id: String,
    pub status: String,
    pub tier: String,
    pub kinds: Vec<String>,
    pub required: Vec<String>,
}

pub fn registry() -> Result<Vec<Slot>> {
    let doc = schema_json("slots.json")?;
    let strs = |v: &Value| v.as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect::<Vec<_>>();
    Ok(doc["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Slot {
            id: s["id"].as_str().unwrap_or("").to_string(),
            status: s["status"].as_str().unwrap_or("").to_string(),
            tier: s["tier"].as_str().unwrap_or("").to_string(),
            kinds: strs(&s["kinds"]),
            required: strs(&s["permissions"]["required"]),
        })
        .collect())
}

/// 雛形の名前一覧(`templates/` 直下のディレクトリ)。
pub fn template_names() -> Vec<String> {
    let mut v: Vec<String> = TEMPLATES.dirs().map(|d| d.path().file_name().unwrap().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// 雛形を `dest` に展開する。`{{key}}` を `vars` で置き換える(テンプレートエンジンは使わない)。
pub fn render_template(name: &str, dest: &Path, vars: &[(&str, &str)]) -> Result<Vec<String>> {
    let dir = TEMPLATES.get_dir(name).ok_or_else(|| anyhow!("雛形がありません: {name}(rasq registry templates で一覧)"))?;
    let mut written = Vec::new();
    write_dir(dir, dir.path(), dest, vars, &mut written)?;
    Ok(written)
}

fn write_dir(dir: &Dir<'_>, base: &Path, dest: &Path, vars: &[(&str, &str)], written: &mut Vec<String>) -> Result<()> {
    for f in dir.files() {
        let rel = f.path().strip_prefix(base).unwrap();
        let out = dest.join(rel);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut text = String::from_utf8(f.contents().to_vec()).with_context(|| format!("雛形 {} が UTF-8 ではありません", f.path().display()))?;
        for (k, v) in vars {
            text = text.replace(&format!("{{{{{k}}}}}"), v);
        }
        std::fs::write(&out, text).with_context(|| format!("{} を書けません", out.display()))?;
        written.push(rel.to_string_lossy().into_owned());
    }
    for d in dir.dirs() {
        write_dir(d, base, dest, vars, written)?;
    }
    Ok(())
}

/// kit(`rasq-plugin-kit`)をワークスペースへ写す(D-244i)。
pub fn write_kit(dest: &Path) -> Result<()> {
    for rel in ["Cargo.toml", "src/lib.rs"] {
        let f = KIT.get_file(rel).ok_or_else(|| anyhow!("同梱 kit に {rel} がありません"))?;
        let out = dest.join(rel);
        std::fs::create_dir_all(out.parent().unwrap())?;
        std::fs::write(&out, f.contents()).with_context(|| format!("{} を書けません", out.display()))?;
    }
    Ok(())
}
