//! アセットパック(docs/slots.md §2.10、契約 v0.11): wasm を持たない配布物。`packs/<name>/manifest.json`(kind = "assets")と画像。
//! ホストは画像を読むだけなので、ここでは「画像であること」と大きさだけを確かめる。

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

/// スキーマ → id / kind → 各画像(実在 / 形式 / 大きさ / 重複)→ 書き忘れ。
pub fn check(dir: &Path, entry: &PackEntry) -> Result<Checked> {
    let manifest_bytes = std::fs::read(dir.join("manifest.json")).with_context(|| format!("{}/manifest.json を読めません", dir.display()))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes).context("manifest.json が JSON ではありません")?;
    let mut errors = schema::violations(&schema::manifest_validator()?, &manifest).into_iter().map(|e| format!("schema: {e}")).collect::<Vec<_>>();
    let mut warnings = Vec::new();
    if manifest["kind"] != "assets" {
        errors.push(format!("packs/ の manifest は kind = \"assets\" でなければなりません: {}", manifest["kind"]));
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
    Ok(Checked { manifest, manifest_bytes, files, errors, warnings })
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

/// パック 1 つ = 配布物 1 つ(root.kind = assets、wasm 無し)。
pub fn package(c: &Checked) -> Result<Package> {
    let root = json!({"id": c.manifest["id"], "version": c.manifest["version"], "kind": "assets", "manifest": "manifest.json"});
    let mut entries = vec![Entry { path: "manifest.json".into(), bytes: c.manifest_bytes.clone() }];
    let mut assets = Vec::new();
    for (path, bytes) in &c.files {
        assets.push(json!({"path": path, "kind": "image", "sha512": pkg::hex(&Sha512::digest(bytes)), "sizeBytes": bytes.len()}));
        entries.push(Entry { path: path.clone(), bytes: bytes.clone() });
    }
    pkg::assemble_with(root, vec![], vec![], assets, vec![], entries)
}
