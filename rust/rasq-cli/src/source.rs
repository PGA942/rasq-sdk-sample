//! 審査用のソース(`rasq build --with-source`、docs/cli.md §6)。
//!
//! `dist/<root.id>-<root.version>.source.tar.zst`: ワークスペースのうち**自分で書いた部分**
//! (`rasq.toml` / `Cargo.toml` / `Cargo.lock` と、ビルド対象のディレクトリ)を zstd 圧縮の
//! tar に入れる。配布はされない — Central は `kind = source` の配布物として持ち、公開の
//! manifest / artifacts には出さず、審査 (rasq-judge の 3 段目) だけが読む。
//!
//! 先頭のエントリ `SOURCE.json` (`rasq.source/1`) に、どの wasm のソースかを書く:
//! 各 wasm の sha512(配布物の目次と同じ値)と、ツールチェーン、ビルドコマンド、
//! ファイル一覧のハッシュ。**再現ビルドの検証はしていない** — ソースとバイナリの一致は
//! 「開発者がこれから作ったと言っているもの」であって証明ではない。後から再現ビルドで
//! 検証する道を残すために、ツールチェーンとコマンドを記録しておく。

use crate::pkg::hex;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::path::{Path, PathBuf};

pub const SUFFIX: &str = ".source.tar.zst";
pub const MANIFEST: &str = "SOURCE.json";
const ZSTD_LEVEL: i32 = 19;
/// 展開後の合計(バイト)。ソースは wasm より小さいはずで、これを超えるなら生成物が混ざっている。
const MAX_UNPACKED: u64 = 32 * 1024 * 1024;
const MAX_FILES: usize = 4000;
/// 1 ファイルの上限。ソースの 1 ファイルがこれを超えることはまず無い(超えるのはデータや生成物)。
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// 入れないディレクトリ名(どの深さでも)。
const SKIP_DIRS: &[&str] = &["target", "dist", "node_modules", ".git", ".idea", ".vscode"];

pub fn file_name(id: &str, version: &str) -> String {
    format!("{id}-{version}{SUFFIX}")
}

/// 集めるディレクトリ: ビルド対象(sns / plugins/<name>)と、root のファイル。
fn roots(root: &Path, ws: &crate::workspace::Workspace) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ws.targets(root).into_iter().map(|t| t.dir).collect();
    if ws.base.as_ref().is_some_and(|b| b.owned) {
        v.push(root.join("base"));
    }
    for l in &ws.libs {
        // wasm 化したライブラリは、ソースを持っていればそのディレクトリごと。
        let p = root.join(&l.path);
        if let Some(dir) = p.parent().filter(|d| *d != root) {
            v.push(dir.to_path_buf());
        }
    }
    v.sort();
    v.dedup();
    v
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).with_context(|| format!("{} を読めません", dir.display()))?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue; // 外を指すかもしれない。ソースに symlink は要らない
        }
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk(&p, out)?;
        } else if ft.is_file() {
            if name.starts_with('.') {
                continue;
            }
            out.push(p);
        }
    }
    Ok(())
}

/// ソースの tar.zst を組む。`bundle` は配布物の目次(root / children の wasm の sha512 を写す)。
pub fn archive(root: &Path, ws: &crate::workspace::Workspace, bundle: &Value) -> Result<Vec<u8>> {
    let id = bundle["root"]["id"].as_str().unwrap_or("");
    let version = bundle["root"]["version"].as_str().unwrap_or("");
    let mut files: Vec<PathBuf> = Vec::new();
    for f in ["rasq.toml", "Cargo.toml", "Cargo.lock"] {
        let p = root.join(f);
        if p.is_file() {
            files.push(p);
        }
    }
    for dir in roots(root, ws) {
        if dir.is_dir() {
            walk(&dir, &mut files)?;
        }
    }
    files.sort();
    files.dedup();
    if files.len() > MAX_FILES {
        bail!("ソースのファイルが多すぎます({} > {MAX_FILES})。生成物が混ざっていませんか", files.len());
    }

    let mtime = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut listing = Vec::new();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total: u64 = 0;
    for p in &files {
        let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(p).with_context(|| format!("{} を読めません", p.display()))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            bail!("{rel}: {} bytes(1 ファイル {} MiB まで。ソースではないものでは?)", bytes.len(), MAX_FILE_BYTES / 1024 / 1024);
        }
        total += bytes.len() as u64;
        if total > MAX_UNPACKED {
            bail!("ソースの合計が {} MiB を超えました。target/ や dist/ 以外の生成物を消してください", MAX_UNPACKED / 1024 / 1024);
        }
        listing.push(json!({"path": rel, "sha512": hex(&Sha512::digest(&bytes)), "sizeBytes": bytes.len()}));
        entries.push((rel, bytes));
    }
    let tree = {
        let mut h = Sha512::new();
        for l in &listing {
            h.update(format!("{}  {}\n", l["sha512"].as_str().unwrap_or(""), l["path"].as_str().unwrap_or("")));
        }
        hex(&h.finalize())
    };
    let mut wasm = Vec::new();
    if let Some(w) = bundle["root"].get("wasm") {
        wasm.push(json!({"id": id, "path": w["path"], "sha512": w["sha512"]}));
    }
    for c in bundle["children"].as_array().into_iter().flatten() {
        wasm.push(json!({"id": c["id"], "path": c["wasm"]["path"], "sha512": c["wasm"]["sha512"]}));
    }
    let manifest = json!({
        "schema": "rasq.source/1",
        "pluginId": id,
        "version": version,
        "sdk": ws.sdk,
        "toolchain": toolchain(),
        "buildCommand": "rasq build",
        "wasm": wasm,
        "treeSha512": tree,
        "files": listing,
    });

    let enc = zstd::Encoder::new(Vec::new(), ZSTD_LEVEL)?;
    let mut tar = tar::Builder::new(enc);
    let append = |tar: &mut tar::Builder<zstd::Encoder<'static, Vec<u8>>>, path: &str, bytes: &[u8]| -> Result<()> {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(mtime);
        h.set_cksum();
        tar.append_data(&mut h, path, bytes).with_context(|| format!("tar に {path} を足せません"))?;
        Ok(())
    };
    append(&mut tar, MANIFEST, &serde_json::to_vec_pretty(&manifest)?)?;
    for (path, bytes) in &entries {
        append(&mut tar, path, bytes)?;
    }
    let enc = tar.into_inner()?;
    Ok(enc.finish()?)
}

/// `rustc --version` と `cargo --version`(取れなければ空)。再現ビルドの手掛かりとして記録するだけ。
fn toolchain() -> Value {
    let run = |cmd: &str| {
        // nosemgrep: rasq-command-spawn — 引数は固定 ("--version")。cmd は下の "rustc" / "cargo" だけで、外の文字列は入らない
        std::process::Command::new(cmd)
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    json!({"rustc": run("rustc"), "cargo": run("cargo"), "target": "wasm32-unknown-unknown"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn unpack(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let dec = zstd::Decoder::new(bytes).unwrap();
        let mut ar = tar::Archive::new(dec);
        let mut out = Vec::new();
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            let path = e.path().unwrap().to_string_lossy().into_owned();
            let mut b = Vec::new();
            e.read_to_end(&mut b).unwrap();
            out.push((path, b));
        }
        out
    }

    #[test]
    fn archive_has_the_source_and_not_the_build_output() {
        let tmp = std::env::temp_dir().join(format!("rasq-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("plugins/hello/src")).unwrap();
        std::fs::create_dir_all(tmp.join("plugins/hello/target/wasm32")).unwrap();
        std::fs::create_dir_all(tmp.join("dist")).unwrap();
        std::fs::write(tmp.join("rasq.toml"), "schema = \"rasq.workspace/1\"\nsdk = \"0.1.0\"\n[[plugins]]\nname = \"hello\"\nslot = \"post.action\"\nid = \"dev.test.hello\"\n").unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[workspace]\nmembers = [\"plugins/hello\"]\n").unwrap();
        std::fs::write(tmp.join("plugins/hello/Cargo.toml"), "[package]\nname = \"hello\"\n").unwrap();
        std::fs::write(tmp.join("plugins/hello/src/lib.rs"), "fn main() {}\n").unwrap();
        std::fs::write(tmp.join("plugins/hello/.env"), "SECRET=1\n").unwrap(); // 隠しファイルは入らない
        std::fs::write(tmp.join("plugins/hello/target/wasm32/hello.wasm"), b"\0asm").unwrap(); // 生成物は入らない
        std::fs::write(tmp.join("dist/x.rasqpkg"), b"zz").unwrap();
        let ws: crate::workspace::Workspace = toml::from_str(&std::fs::read_to_string(tmp.join("rasq.toml")).unwrap()).unwrap();
        let bundle = json!({"root": {"id": "dev.test.hello", "version": "0.1.0", "wasm": {"path": "hello.wasm", "sha512": "ab"}}, "children": []});
        let bytes = archive(&tmp, &ws, &bundle).unwrap();
        let entries = unpack(&bytes);
        let paths: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths[0], MANIFEST);
        assert!(paths.contains(&"rasq.toml") && paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"plugins/hello/src/lib.rs") && paths.contains(&"plugins/hello/Cargo.toml"));
        assert!(!paths.iter().any(|p| p.contains("target/") || p.starts_with("dist/") || p.ends_with(".env")), "{paths:?}");
        let m: Value = serde_json::from_slice(&entries[0].1).unwrap();
        assert_eq!(m["schema"], "rasq.source/1");
        assert_eq!(m["pluginId"], "dev.test.hello");
        assert_eq!(m["wasm"][0]["sha512"], "ab");
        assert_eq!(m["files"].as_array().unwrap().len(), paths.len() - 1);
        assert_eq!(m["treeSha512"].as_str().unwrap().len(), 128);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
