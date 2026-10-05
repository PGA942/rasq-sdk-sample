//! 配布物 `.rspkg`(旧 `.rasqpkg`。docs/cli.md §6、D-034 / D-244d): zstd 圧縮 tar。先頭が `bundle.json`(`rasq.bundle/1`、schema/bundle.schema.json)、
//! 最後が `SHA512SUMS`。中のパスは bundle.schema.json の `path` の文法(`..` / 先頭 `/` 不可)。

use crate::workspace::{Target, Workspace};
use crate::{embed, schema, wasm};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// 配布物の拡張子。既定は `.rspkg`。`--ext rasqpkg` で旧来の `.rasqpkg` で出す(中身は同じ。署名は `<pkg>.sig.json`)。
pub const EXT: &str = "rspkg";
/// 旧来の拡張子。読むほう(inspect / fee / submit / publish / init --base)はどちらも受け付ける。
pub const LEGACY_EXT: &str = "rasqpkg";
/// 受け付ける拡張子(先が既定)。
pub const EXTS: [&str; 2] = [EXT, LEGACY_EXT];

/// 拡張子が `.rspkg` か `.rasqpkg`。
pub fn is_package(p: &Path) -> bool {
    p.extension().and_then(|x| x.to_str()).is_some_and(|x| EXTS.contains(&x))
}

/// ファイル名から配布物の拡張子を外す(`x-1.0.0.rspkg` / `x-1.0.0.rasqpkg` → `x-1.0.0`)。
pub fn strip_ext(name: &str) -> &str {
    EXTS.iter().find_map(|e| name.strip_suffix(e).and_then(|s| s.strip_suffix('.'))).unwrap_or(name)
}

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
    /// `<root.id>-<version>`(ファイル名は [`Package::file_name`] で拡張子を付ける)
    pub stem: String,
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
        let mut root_json = json!({"id": rb.id, "version": rb.version, "kind": "sns", "manifest": "manifest.json", "wasm": file_json(&rb.wasm_name, &rb.wasm)});
        attach_safety(ws, &mut root_json);
        out.push(assemble(root_json, children, &extras, entries)?);
    } else {
        if plugins.is_empty() && ws.packs.is_empty() {
            bail!("配布するものがありません(rasq new <slot> <name>)");
        }
        let requires = ws.base.as_ref().map(|b| json!([{"id": b.id, "version": format!("^{}", b.version)}]));
        for (b, _t) in &plugins {
            let mut root_json = json!({"id": b.id, "version": b.version, "kind": b.kind, "manifest": "manifest.json", "wasm": file_json(&b.wasm_name, &b.wasm)});
            attach_safety(ws, &mut root_json);
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

/// `[listing.safety]` を目次の `root.safety` に入れる。別ファイルにしないのは、目次に無い
/// ファイルを拒否する既存の検査(アプリのインストール、`rasq inspect`)を壊さないため。
/// 目次は開発者の署名の範囲に入る。
fn attach_safety(ws: &Workspace, root_json: &mut Value) {
    if let Some(s) = ws.listing.as_ref().and_then(|l| l.safety.as_ref()) {
        root_json["safety"] = s.to_json();
    }
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
    let stem = format!("{}-{}", bundle["root"]["id"].as_str().unwrap(), bundle["root"]["version"].as_str().unwrap());
    Ok(Package { stem, bundle, entries })
}

impl Package {
    /// `<root.id>-<version>.<ext>`(`ext` は [`EXTS`] のどれか)。
    pub fn file_name(&self, ext: &str) -> String {
        format!("{}.{ext}", self.stem)
    }
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

/// 展開の上限(zstd 爆弾の対策)。配布物(`.rspkg` / `.rasqpkg`)は他人のもので、`inspect` / `init --base` は中身を全部メモリに読む。
/// 圧縮率に上限は無い(0 が続くだけの 1 GiB は数十 KiB に縮む)ので、展開した量で切る。
/// モデル本体(onnx / gguf、lazy = false)を同梱できるので 1 ファイルは大きめに取ってある。
#[derive(Clone, Copy)]
pub struct ExtractLimits {
    /// 中身の合計(tar のヘッダが申告する大きさの和)
    pub total_bytes: u64,
    /// 1 ファイル
    pub entry_bytes: u64,
    /// エントリの数
    pub entries: usize,
    /// zstd を解いた tar の流れそのもの(ヘッダ / 詰め物 / GNU の長い名前 / PAX を含む)。
    /// 申告の検査より先に効く最後の線で、長い名前のエントリ(tar crate が丸ごとメモリに読む)もこれで止まる。
    pub stream_bytes: u64,
}

impl ExtractLimits {
    /// 合計 1 GiB / 1 ファイル 512 MiB / 4096 エントリ。流れは合計 + 64 MiB
    /// (ヘッダと詰め物は 1 エントリ高々 1 KiB = 4096 エントリで 4 MiB)。
    pub const DEFAULT: ExtractLimits = ExtractLimits {
        total_bytes: 1024 * 1024 * 1024,
        entry_bytes: 512 * 1024 * 1024,
        entries: 4096,
        stream_bytes: 1024 * 1024 * 1024 + 64 * 1024 * 1024,
    };
}

/// 読んだ量が `left` を超えたらエラーにする Read。
struct Capped<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            // ちょうど上限で終わる入力は通す。まだ続くならそこで止める。
            let mut one = [0u8; 1];
            return match self.inner.read(&mut one)? {
                0 => Ok(0),
                _ => Err(std::io::Error::other("展開後の大きさが上限を超えました(zstd 爆弾?)")),
            };
        }
        let n = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let got = self.inner.read(&mut buf[..n])?;
        self.left -= got as u64;
        Ok(got)
    }
}

/// 配布物(`.rspkg` / `.rasqpkg`)を全部読む(bundle.json / SHA512SUMS も含む)。上限は [`ExtractLimits::DEFAULT`]。
pub fn extract(bytes: &[u8]) -> Result<Vec<Entry>> {
    extract_with(bytes, &ExtractLimits::DEFAULT)
}

pub fn extract_with(bytes: &[u8], limits: &ExtractLimits) -> Result<Vec<Entry>> {
    let dec = zstd::Decoder::new(bytes).context("zstd として読めません(.rspkg / .rasqpkg ではない?)")?;
    let mut ar = tar::Archive::new(Capped { inner: dec, left: limits.stream_bytes });
    let mut out = Vec::new();
    let mut total: u64 = 0;
    for e in ar.entries().context("tar として読めません")? {
        let mut e = e.context("zstd + tar として読めません(.rspkg / .rasqpkg ではない? 展開後の大きさが上限を超えた?)")?;
        let path = e.path()?.to_string_lossy().into_owned();
        check_path(&path)?;
        if out.len() >= limits.entries {
            bail!("エントリが多すぎます(上限 {} 個)", limits.entries);
        }
        // 読む前に申告の大きさで切る(読んでから数えると、その 1 ファイルぶんはメモリに載ってしまう)。
        let size = e.size();
        if size > limits.entry_bytes {
            bail!("{path}: 展開後 {size} バイトは 1 ファイルの上限 {} バイトを超えます", limits.entry_bytes);
        }
        total = total.saturating_add(size);
        if total > limits.total_bytes {
            bail!("展開後の合計が上限 {} バイトを超えます({path} まで {total} バイト)", limits.total_bytes);
        }
        let mut b = Vec::new();
        (&mut e).take(size).read_to_end(&mut b).with_context(|| format!("{path} を読めません"))?;
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
    // モデル本体(runtimes.*.file / tokenizer.file)は目次に個別には載らない。model manifest から引いて「載っているもの」に数える
    // (lazy = true なら本体は入っていなくてよい。中身の一致は SHA512SUMS で見る)。
    for m in bundle["models"].as_array().into_iter().flatten() {
        let Some(path) = m["manifest"].as_str() else { continue };
        let Some(e) = by_path.get(path) else { continue };
        let Ok(doc) = serde_json::from_slice::<Value>(&e.bytes) else {
            problems.push(format!("{path}: model manifest が JSON ではありません"));
            continue;
        };
        for (_, rt) in doc["runtimes"].as_object().into_iter().flatten() {
            for f in [rt["file"].as_str(), rt["tokenizer"]["file"].as_str()].into_iter().flatten() {
                listed.insert(f.to_string(), ());
                if m["lazy"].as_bool() != Some(true) && !by_path.contains_key(f) {
                    problems.push(format!("{f}: model manifest にあるのに中に無い(lazy ではない)"));
                }
            }
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
            problems.push(format!("{}: 目次に載っていないファイル", e.path));
        }
    }
    Ok(Inspected { bundle, entries, problems })
}

/// 進捗表示用。
pub fn describe(p: &Package, ext: &str) -> String {
    let b = &p.bundle;
    let mut s = format!("{} (root {} {} {})", p.file_name(ext), b["root"]["kind"].as_str().unwrap_or(""), b["root"]["id"].as_str().unwrap_or(""), b["root"]["version"].as_str().unwrap_or(""));
    let n = |k: &str| b[k].as_array().map(|a| a.len()).unwrap_or(0);
    let _ = write!(s, "  children {}  libs {}  assets {}  models {}", n("children"), n("libs"), n("assets"), n("models"));
    if let Some(r) = b["root"]["requires"].as_array() {
        let _ = write!(s, "  requires {}", r.iter().map(|x| format!("{} {}", x["id"].as_str().unwrap_or(""), x["version"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
    }
    s
}

use std::fmt::Write as _;

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, bytes) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, path, *bytes).unwrap();
        }
        zstd::encode_all(&tar.into_inner().unwrap()[..], 3).unwrap()
    }

    const SMALL: ExtractLimits = ExtractLimits { total_bytes: 4096, entry_bytes: 2048, entries: 4, stream_bytes: 64 * 1024 };

    #[test]
    fn both_extensions_are_packages() {
        assert!(is_package(Path::new("dist/a.b-1.0.0.rspkg")));
        assert!(is_package(Path::new("dist/a.b-1.0.0.rasqpkg")));
        assert!(!is_package(Path::new("dist/a.b-1.0.0.rspkg.sig.json")));
        assert!(!is_package(Path::new("dist/a.b-1.0.0.source.tar.zst")));
        assert_eq!(strip_ext("a.b-1.0.0.rspkg"), "a.b-1.0.0");
        assert_eq!(strip_ext("a.b-1.0.0.rasqpkg"), "a.b-1.0.0");
        assert_eq!(strip_ext("a.b-1.0.0.wasm"), "a.b-1.0.0.wasm");
    }

    #[test]
    fn extract_within_the_limits() {
        let bytes = pack(&[(BUNDLE, b"{}"), ("a.wasm", &[1u8; 2048]), ("b/c.json", &[2u8; 2000])]);
        let out = extract_with(&bytes, &SMALL).unwrap();
        assert_eq!(out.iter().map(|e| (e.path.as_str(), e.bytes.len())).collect::<Vec<_>>(), [(BUNDLE, 2), ("a.wasm", 2048), ("b/c.json", 2000)]);
    }

    #[test]
    fn extract_refuses_too_many_entries() {
        let files: Vec<(String, Vec<u8>)> = (0..5).map(|i| (if i == 0 { BUNDLE.to_string() } else { format!("f{i}") }, vec![0u8; 1])).collect();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
        let err = extract_with(&pack(&refs), &SMALL).err().unwrap().to_string();
        assert!(err.contains("エントリが多すぎます"), "{err}");
    }

    #[test]
    fn extract_refuses_a_file_over_the_per_file_cap() {
        let big = vec![0u8; 2049];
        let err = extract_with(&pack(&[(BUNDLE, b"{}"), ("model.onnx", &big)]), &SMALL).err().unwrap().to_string();
        assert!(err.contains("1 ファイルの上限"), "{err}");
    }

    #[test]
    fn extract_refuses_a_total_over_the_cap() {
        let part = vec![0u8; 2000];
        let err = extract_with(&pack(&[(BUNDLE, b"{}"), ("a", &part), ("b", &part), ("c", &part)]), &SMALL).err().unwrap().to_string();
        assert!(err.contains("合計が上限"), "{err}");
    }

    #[test]
    fn extract_stops_a_stream_over_the_cap() {
        // 申告の検査が効かない部分(GNU の長い名前は tar crate がそのままメモリに読む)も流れの上限で止まる。
        let long = format!("{}/x", "a".repeat(8000));
        let limits = ExtractLimits { stream_bytes: 4096, ..SMALL };
        let err = extract_with(&pack(&[(BUNDLE, b"{}"), (&long, b"1")]), &limits).err().unwrap();
        assert!(format!("{err:#}").contains("上限を超えました"), "{err:#}");
    }

    #[test]
    fn extract_refuses_a_zstd_bomb_before_reading_it() {
        // 既定の上限で: 0 が 512 MiB + 1 続くエントリは圧縮すると 1 MiB を切る。申告の大きさで、読む前に切れる。
        let n = ExtractLimits::DEFAULT.entry_bytes + 1;
        let mut tar = tar::Builder::new(zstd::Encoder::new(Vec::new(), 1).unwrap());
        for (path, size) in [(BUNDLE, 2u64), ("zeros.bin", n)] {
            let mut h = tar::Header::new_gnu();
            h.set_size(size);
            h.set_mode(0o644);
            h.set_cksum();
            let data: Box<dyn Read> = if size == 2 { Box::new(&b"{}"[..]) } else { Box::new(std::io::repeat(0).take(size)) };
            tar.append_data(&mut h, path, data).unwrap();
        }
        let bomb = tar.into_inner().unwrap().finish().unwrap();
        assert!(bomb.len() < 1024 * 1024, "{}", bomb.len());
        let err = extract(&bomb).err().unwrap().to_string();
        assert!(err.contains("1 ファイルの上限"), "{err}");
    }
}
