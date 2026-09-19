//! `.rasqpkg`(docs/cli.md §6、D-034 / D-244d): zstd 圧縮 tar。先頭が `bundle.json`(`rasq.bundle/1`、schema/bundle.schema.json)、
//! 最後が `SHA512SUMS`。中のパスは bundle.schema.json の `path` の文法(`..` / 先頭 `/` 不可)。

use crate::workspace::{Target, Workspace};
use crate::{embed, schema, wasm};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

pub const BUNDLE: &str = "bundle.json";
pub const SUMS: &str = "SHA512SUMS";
const ZSTD_LEVEL: i32 = 19;

/// 配布物の中の 1 ファイル。
pub struct Entry {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// 組み立て前の配布物 1 つ。
pub struct Package {
    /// `<root.id>-<version>.rasqpkg`
    pub file_name: String,
    pub bundle: Value,
    /// bundle.json / SHA512SUMS 以外。
    pub entries: Vec<Entry>,
}

fn file_json(path: &str, bytes: &[u8]) -> Value {
    json!({"path": path, "sha512": hex(&Sha512::digest(bytes)), "sizeBytes": bytes.len()})
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// パスの文法(bundle.schema.json `path`)。
fn check_path(p: &str) -> Result<()> {
    let ok = !p.is_empty()
        && !p.starts_with('/')
        && !p.split('/').any(|seg| seg.is_empty() || seg == "..")
        && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
        && !p.starts_with('.');
    if ok {
        Ok(())
    } else {
        Err(anyhow!("配布物内のパスとして使えません(英数字 _ . / - だけ、'..' と先頭 '/' は不可): {p:?}"))
    }
}

struct Built {
    id: String,
    version: String,
    kind: String,
    manifest_bytes: Vec<u8>,
    wasm_name: String,
    wasm: Vec<u8>,
}

fn read_target(root: &Path, t: &Target, profile: &str) -> Result<Built> {
    let manifest_bytes = std::fs::read(t.dir.join("manifest.json")).with_context(|| format!("{}/manifest.json を読めません", t.dir.display()))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let wasm_path = t.wasm_path(root, profile);
    let wasm = std::fs::read(&wasm_path).with_context(|| format!("{} がありません(rasq check を先に)", wasm_path.display()))?;
    Ok(Built {
        id: manifest["id"].as_str().unwrap_or("").to_string(),
        version: manifest["version"].as_str().unwrap_or("0.0.0").to_string(),
        kind: manifest["kind"].as_str().unwrap_or("").to_string(),
        manifest_bytes,
        wasm_name: wasm_path.file_name().unwrap().to_string_lossy().into_owned(),
        wasm,
    })
}

/// base/(`rasq init --base` が置いたもの)。
fn read_base(root: &Path) -> Result<Built> {
    let dir = root.join("base");
    let manifest_bytes = std::fs::read(dir.join("manifest.json")).context("base/manifest.json がありません(rasq init --base)")?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let wasm_path = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "wasm"))
        .ok_or_else(|| anyhow!("base/ に .wasm がありません"))?;
    Ok(Built {
        id: manifest["id"].as_str().unwrap_or("").to_string(),
        version: manifest["version"].as_str().unwrap_or("0.0.0").to_string(),
        kind: manifest["kind"].as_str().unwrap_or("").to_string(),
        manifest_bytes,
        wasm_name: wasm_path.file_name().unwrap().to_string_lossy().into_owned(),
        wasm: std::fs::read(&wasm_path)?,
    })
}

/// ワークスペースから配布物を計画する(docs/cli.md D-244c):
/// - `[sns]` → root = sns、children = plugins
/// - `[base] owned = true` → root = base/、children = plugins
/// - それ以外 → plugins ごとに root = その子の配布物、`requires` = base(あれば)
pub fn plan(root: &Path, ws: &Workspace, profile: &str) -> Result<Vec<Package>> {
    let targets = ws.targets(root);
    let (sns_t, plugin_ts): (Vec<&Target>, Vec<&Target>) = targets.iter().partition(|t| t.slot.is_none());
    let plugins: Vec<(Built, &Target)> = plugin_ts.iter().map(|t| Ok((read_target(root, t, profile)?, *t))).collect::<Result<_>>()?;
    let extras = extras(root, ws)?;

    let mut out = Vec::new();
    let owned_base = ws.base.as_ref().filter(|b| b.owned).map(|_| read_base(root)).transpose()?;
    let root_built = match (sns_t.first(), owned_base) {
        (Some(t), _) => Some(read_target(root, t, profile)?),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    if let Some(rb) = root_built {
        if rb.kind != "sns" {
            bail!("root {} の kind が sns ではありません: {}", rb.id, rb.kind);
        }
        let mut entries = vec![Entry { path: "manifest.json".into(), bytes: rb.manifest_bytes.clone() }, Entry { path: rb.wasm_name.clone(), bytes: rb.wasm.clone() }];
        let mut children = Vec::new();
        for (b, t) in &plugins {
            if !b.id.starts_with(&format!("{}.", rb.id)) {
                bail!("子 {} の id が root {} の配下(<root>.<name>)ではありません", b.id, rb.id);
            }
            let dir = format!("plugins/{}", t.package);
            let mp = format!("{dir}/manifest.json");
            let wp = format!("{dir}/{}", b.wasm_name);
            children.push(json!({"id": b.id, "version": b.version, "kind": b.kind, "manifest": mp, "wasm": file_json(&wp, &b.wasm), "template": t.slot}));
            entries.push(Entry { path: mp, bytes: b.manifest_bytes.clone() });
            entries.push(Entry { path: wp, bytes: b.wasm.clone() });
        }
        let root_json = json!({"id": rb.id, "version": rb.version, "kind": "sns", "manifest": "manifest.json", "wasm": file_json(&rb.wasm_name, &rb.wasm)});
        out.push(assemble(root_json, children, &extras, entries)?);
    } else {
        if plugins.is_empty() && ws.packs.is_empty() {
            bail!("配布するものがありません(rasq new <slot> <name>)");
        }
        let requires = ws.base.as_ref().map(|b| json!([{"id": b.id, "version": format!("^{}", b.version)}]));
        for (b, _t) in &plugins {
            let mut root_json = json!({"id": b.id, "version": b.version, "kind": b.kind, "manifest": "manifest.json", "wasm": file_json(&b.wasm_name, &b.wasm)});
            if let Some(r) = &requires {
                root_json["requires"] = r.clone();
            }
            let entries = vec![Entry { path: "manifest.json".into(), bytes: b.manifest_bytes.clone() }, Entry { path: b.wasm_name.clone(), bytes: b.wasm.clone() }];
            out.push(assemble(root_json, vec![], &extras, entries)?);
        }
    }
    Ok(out)
}

/// libs / assets / models(ワークスペース共通。配布物すべてに入る)。
struct Extras {
    libs: Vec<Value>,
    assets: Vec<Value>,
    models: Vec<Value>,
    entries: Vec<Entry>,
}

fn extras(root: &Path, ws: &Workspace) -> Result<Extras> {
    let mut x = Extras { libs: vec![], assets: vec![], models: vec![], entries: vec![] };
    let mut seen = BTreeMap::new();
    let mut add = |path: &str, bytes: Vec<u8>, entries: &mut Vec<Entry>| -> Result<()> {
        check_path(path)?;
        if seen.insert(path.to_string(), ()).is_some() {
            bail!("配布物内のパスが重複しています: {path}");
        }
        entries.push(Entry { path: path.into(), bytes });
        Ok(())
    };
    for l in &ws.libs {
        let bytes = std::fs::read(root.join(&l.path)).with_context(|| format!("lib {}: {} を読めません", l.name, l.path))?;
        let ins = wasm::inspect(&bytes).with_context(|| format!("lib {}", l.name))?;
        let missing: Vec<&String> = l.exports.iter().filter(|e| !ins.exports.contains(e)).collect();
        if !missing.is_empty() {
            bail!("lib {}: exports {:?} が wasm にありません(実際: {:?})", l.name, missing, ins.exports);
        }
        let path = format!("libs/{}", Path::new(&l.path).file_name().unwrap().to_string_lossy());
        x.libs.push(json!({"name": l.name, "wasm": file_json(&path, &bytes), "exports": l.exports}));
        add(&path, bytes, &mut x.entries)?;
    }
    for a in &ws.assets {
        let bytes = std::fs::read(root.join(&a.path)).with_context(|| format!("asset {} を読めません", a.path))?;
        x.assets.push(json!({"path": a.path, "kind": a.kind, "sha512": hex(&Sha512::digest(&bytes)), "sizeBytes": bytes.len()}));
        add(&a.path, bytes, &mut x.entries)?;
    }
    for m in &ws.models {
        let bytes = std::fs::read(root.join(&m.manifest)).with_context(|| format!("model {} を読めません", m.manifest))?;
        let doc: Value = serde_json::from_slice(&bytes).with_context(|| format!("model {} が JSON ではありません", m.manifest))?;
        let stem = Path::new(&m.manifest).file_name().unwrap().to_string_lossy().trim_end_matches(".model.json").replace(['.', '/'], "-");
        let mut entry = json!({"manifest": m.manifest, "lazy": m.lazy});
        if m.lazy {
            entry["artifactId"] = json!(format!("model-{stem}"));
        } else {
            // 本体(runtimes.*.file / tokenizer.file)も入れる。パスはワークスペース root 基準
            for (_, rt) in doc["runtimes"].as_object().into_iter().flatten() {
                for f in [rt["file"].as_str(), rt["tokenizer"]["file"].as_str()].into_iter().flatten() {
                    let b = std::fs::read(root.join(f)).with_context(|| format!("model {}: {f} がありません(lazy = true なら本体は入れない)", m.manifest))?;
                    add(f, b, &mut x.entries)?;
                }
            }
        }
        x.models.push(entry);
        add(&m.manifest, bytes, &mut x.entries)?;
    }
    Ok(x)
}

fn assemble(root_json: Value, children: Vec<Value>, extras: &Extras, mut entries: Vec<Entry>) -> Result<Package> {
    for e in &extras.entries {
        entries.push(Entry { path: e.path.clone(), bytes: e.bytes.clone() });
    }
    assemble_with(root_json, children, extras.libs.clone(), extras.assets.clone(), extras.models.clone(), entries)
}

/// 目次を組み立てて bundle.schema.json で確かめる。
pub fn assemble_with(root_json: Value, children: Vec<Value>, libs: Vec<Value>, assets: Vec<Value>, models: Vec<Value>, entries: Vec<Entry>) -> Result<Package> {
    let mut seen = BTreeMap::new();
    for e in &entries {
        check_path(&e.path)?;
        if seen.insert(&e.path, ()).is_some() {
            bail!("配布物内のパスが重複しています: {}", e.path);
        }
    }
    let bundle = json!({
        "schema": "rasq.bundle/1",
        "sdk": embed::SDK_VERSION,
        "createdAt": crate::credentials::now(),
        "root": root_json,
        "children": children,
        "libs": libs,
        "assets": assets,
        "models": models,
    });
    let v = schema::validator("bundle.schema.json")?;
    let errs = schema::violations(&v, &bundle);
    if !errs.is_empty() {
        bail!("bundle.json が bundle.schema.json に合いません:\n  {}", errs.join("\n  "));
    }
    let file_name = format!("{}-{}.rasqpkg", bundle["root"]["id"].as_str().unwrap(), bundle["root"]["version"].as_str().unwrap());
    Ok(Package { file_name, bundle, entries })
}

/// tar + zstd に固める。順序: bundle.json → entries → SHA512SUMS。
pub fn serialize(p: &Package) -> Result<Vec<u8>> {
    let mtime = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let enc = zstd::Encoder::new(Vec::new(), ZSTD_LEVEL)?;
    let mut tar = tar::Builder::new(enc);
    let bundle_bytes = serde_json::to_vec_pretty(&p.bundle)?;
    let mut sums = String::new();
    let mut append = |tar: &mut tar::Builder<zstd::Encoder<'static, Vec<u8>>>, path: &str, bytes: &[u8]| -> Result<()> {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(mtime);
        h.set_cksum();
        tar.append_data(&mut h, path, bytes).with_context(|| format!("tar に {path} を足せません"))?;
        sums.push_str(&format!("{}  {path}\n", hex(&Sha512::digest(bytes))));
        Ok(())
    };
    append(&mut tar, BUNDLE, &bundle_bytes)?;
    for e in &p.entries {
        append(&mut tar, &e.path, &e.bytes)?;
    }
    let sums_bytes = sums.into_bytes();
    let mut h = tar::Header::new_gnu();
    h.set_size(sums_bytes.len() as u64);
    h.set_mode(0o644);
    h.set_mtime(mtime);
    h.set_cksum();
    tar.append_data(&mut h, SUMS, &sums_bytes[..])?;
    let enc = tar.into_inner()?;
    Ok(enc.finish()?)
}

/// `.rasqpkg` を全部読む(bundle.json / SHA512SUMS も含む)。
pub fn extract(bytes: &[u8]) -> Result<Vec<Entry>> {
    let dec = zstd::Decoder::new(bytes).context("zstd として読めません(.rasqpkg ではない?)")?;
    let mut ar = tar::Archive::new(dec);
    let mut out = Vec::new();
    for e in ar.entries().context("tar として読めません")? {
        let mut e = e.context("zstd + tar として読めません(.rasqpkg ではない?)")?;
        let path = e.path()?.to_string_lossy().into_owned();
        check_path(&path)?;
        let mut b = Vec::new();
        e.read_to_end(&mut b)?;
        out.push(Entry { path, bytes: b });
    }
    if out.first().map(|e| e.path.as_str()) != Some(BUNDLE) {
        bail!("先頭が {BUNDLE} ではありません");
    }
    Ok(out)
}

pub struct Inspected {
    pub bundle: Value,
    pub entries: Vec<Entry>,
    /// (パス, 問題)
    pub problems: Vec<String>,
}

/// 目次と中身を突き合わせる: bundle.json のスキーマ、各ファイルの sha512 / sizeBytes、SHA512SUMS、余分なファイル。
pub fn inspect(bytes: &[u8]) -> Result<Inspected> {
    let entries = extract(bytes)?;
    let bundle: Value = serde_json::from_slice(&entries[0].bytes).context("bundle.json が JSON ではありません")?;
    let mut problems = Vec::new();
    let v = schema::validator("bundle.schema.json")?;
    problems.extend(schema::violations(&v, &bundle).into_iter().map(|e| format!("bundle.json: {e}")));
    let by_path: BTreeMap<&str, &Entry> = entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut listed: BTreeMap<String, ()> = BTreeMap::new();
    let mut check_file = |f: &Value, problems: &mut Vec<String>| {
        let Some(path) = f["path"].as_str() else { return };
        listed.insert(path.into(), ());
        match by_path.get(path) {
            None => problems.push(format!("{path}: 目次にあるのに中に無い")),
            Some(e) => {
                if let Some(s) = f["sha512"].as_str() {
                    if hex(&Sha512::digest(&e.bytes)) != s {
                        problems.push(format!("{path}: sha512 が目次と違う"));
                    }
                }
                if let Some(n) = f["sizeBytes"].as_u64() {
                    if e.bytes.len() as u64 != n {
                        problems.push(format!("{path}: sizeBytes が目次と違う"));
                    }
                }
            }
        }
    };
    check_file(&bundle["root"]["wasm"], &mut problems);
    for c in bundle["children"].as_array().into_iter().flatten() {
        check_file(&c["wasm"], &mut problems);
    }
    for l in bundle["libs"].as_array().into_iter().flatten() {
        check_file(&l["wasm"], &mut problems);
    }
    for a in bundle["assets"].as_array().into_iter().flatten() {
        check_file(a, &mut problems);
    }
    // manifest / model manifest は path だけ
    let mut plain: Vec<String> = vec![bundle["root"]["manifest"].as_str().unwrap_or("").into()];
    plain.extend(bundle["children"].as_array().into_iter().flatten().filter_map(|c| c["manifest"].as_str().map(String::from)));
    plain.extend(bundle["models"].as_array().into_iter().flatten().filter_map(|m| m["manifest"].as_str().map(String::from)));
    for p in plain {
        listed.insert(p.clone(), ());
        if !by_path.contains_key(p.as_str()) {
            problems.push(format!("{p}: 目次にあるのに中に無い"));
        }
    }
    // SHA512SUMS
    match by_path.get(SUMS) {
        None => problems.push(format!("{SUMS} が無い")),
        Some(s) => {
            for line in String::from_utf8_lossy(&s.bytes).lines() {
                if let Some((h, p)) = line.split_once("  ") {
                    match by_path.get(p) {
                        Some(e) if hex(&Sha512::digest(&e.bytes)) == h => {}
                        Some(_) => problems.push(format!("{p}: {SUMS} と違う")),
                        None => problems.push(format!("{p}: {SUMS} にあるのに中に無い")),
                    }
                }
            }
        }
    }
    for e in &entries {
        if e.path != BUNDLE && e.path != SUMS && !listed.contains_key(&e.path) {
            // モデル本体(runtimes.*.file)は目次に個別には載らないので警告にとどめる
            problems.push(format!("{}: 目次に載っていないファイル(モデル本体なら可)", e.path));
        }
    }
    Ok(Inspected { bundle, entries, problems })
}

/// 進捗表示用。
pub fn describe(p: &Package) -> String {
    let b = &p.bundle;
    let mut s = format!("{} (root {} {} {})", p.file_name, b["root"]["kind"].as_str().unwrap_or(""), b["root"]["id"].as_str().unwrap_or(""), b["root"]["version"].as_str().unwrap_or(""));
    let n = |k: &str| b[k].as_array().map(|a| a.len()).unwrap_or(0);
    let _ = write!(s, "  children {}  libs {}  assets {}  models {}", n("children"), n("libs"), n("assets"), n("models"));
    if let Some(r) = b["root"]["requires"].as_array() {
        let _ = write!(s, "  requires {}", r.iter().map(|x| format!("{} {}", x["id"].as_str().unwrap_or(""), x["version"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
    }
    s
}

use std::fmt::Write as _;
