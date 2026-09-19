//! `rasq publish`(docs/cli.md §7、D-244h): Central の管理 API にそのまま流す。
//!
//! 1. `GET  /api/v1/plugins/{id}/manifest`(公開中の版。無ければ新規)→ `permissionsAdded` の差分
//! 2. `PUT  /api/v1/admin/plugins/{id}`(掲載、`--skip-listing` で省略)
//! 3. `POST /api/v1/admin/plugins/{id}/versions`(版 + マニフェスト)
//! 4. `PUT  /api/v1/admin/plugins/{id}/versions/{v}/artifacts/bundle?kind=bundle`(.rasqpkg。応答の sha512 を照合)
//! 5. lazy モデルごとに `PUT …/artifacts/{artifactId}?kind=model&lazy=true`
//!
//! `--dry-run` は要求を `dist/publish/<pkg>/NN-<label>.json`(+ `.bin`)に書くだけ(審査側に渡す束)。
//! トークンは `RASQ_CENTRAL_TOKEN`(`--token-env` で変更)か credentials.toml の `[central."<host>"]`。

use crate::workspace::Workspace;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::path::{Path, PathBuf};

pub struct Options {
    pub central: String,
    pub channel: String,
    pub notes: String,
    pub dry_run: bool,
    pub skip_listing: bool,
    pub make_current: bool,
    pub token: Option<String>,
}

/// Central の URL から credentials.toml のキー(`host[:port]`)。
pub fn host_key(url: &str) -> String {
    let s = url.trim_end_matches('/');
    let s = s.split_once("://").map(|(_, r)| r).unwrap_or(s);
    s.split('/').next().unwrap_or(s).to_string()
}

struct Request {
    label: String,
    method: &'static str,
    path: String,
    query: Vec<(String, String)>,
    body: Body,
}

enum Body {
    None,
    Json(Value),
    Bytes(Vec<u8>, &'static str),
}

struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
    dry_dir: Option<PathBuf>,
    seq: usize,
}

impl Client {
    fn url(&self, r: &Request) -> String {
        let mut u = format!("{}{}", self.base, r.path);
        if !r.query.is_empty() {
            u.push('?');
            u.push_str(&r.query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
        }
        u
    }

    /// 実行(または dry-run の書き出し)。戻りは (status, body JSON か Null)。
    fn send(&mut self, r: Request, admin: bool) -> Result<(u16, Value)> {
        self.seq += 1;
        let url = self.url(&r);
        if let Some(dir) = &self.dry_dir {
            let stem = format!("{:02}-{}", self.seq, r.label);
            let mut doc = json!({"method": r.method, "url": url, "headers": {}});
            if admin {
                doc["headers"]["Authorization"] = json!("Bearer $RASQ_CENTRAL_TOKEN");
            }
            match &r.body {
                Body::None => {}
                Body::Json(v) => {
                    doc["headers"]["Content-Type"] = json!("application/json");
                    doc["body"] = v.clone();
                }
                Body::Bytes(b, ct) => {
                    doc["headers"]["Content-Type"] = json!(ct);
                    doc["bodyFile"] = json!(format!("{stem}.bin"));
                    doc["bodySha512"] = json!(hex(&Sha512::digest(b)));
                    std::fs::write(dir.join(format!("{stem}.bin")), b)?;
                }
            }
            std::fs::write(dir.join(format!("{stem}.json")), serde_json::to_string_pretty(&doc)?)?;
            println!("  [dry-run] {} {} → {}/{stem}.json", r.method, url, dir.display());
            return Ok((0, Value::Null));
        }
        let mut req = self.agent.request(r.method, &url);
        if admin {
            let t = self.token.as_deref().ok_or_else(|| anyhow!("Central のトークンがありません(RASQ_CENTRAL_TOKEN か rasq token set --central <host>)"))?;
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let res = match r.body {
            Body::None => req.call(),
            Body::Json(v) => req.send_json(v),
            Body::Bytes(b, ct) => req.set("Content-Type", ct).send_bytes(&b),
        };
        match res {
            Ok(res) => {
                let status = res.status();
                let text = res.into_string().unwrap_or_default();
                Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
            }
            Err(ureq::Error::Status(code, res)) => {
                let text = res.into_string().unwrap_or_default();
                Ok((code, serde_json::from_str(&text).unwrap_or(json!({"message": text}))))
            }
            Err(e) => Err(anyhow!("{} {url}: {e}", r.method)),
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn fail(what: &str, status: u16, body: &Value) -> anyhow::Error {
    anyhow!("{what}: HTTP {status} {}", body["message"].as_str().or(body["error"].as_str()).unwrap_or(&body.to_string()))
}

/// 掲載(UpsertPlugin)。`[listing]` があればそれを、無ければ manifest から。
fn listing(ws: &Workspace, manifest: &Value) -> Value {
    let l = ws.listing.as_ref();
    let desc = manifest["description"].as_str().unwrap_or("");
    let summary = l.and_then(|l| l.summary.clone()).unwrap_or_else(|| desc.chars().take(80).collect());
    let mut sns: Vec<String> = l.and_then(|l| l.sns.clone()).unwrap_or_default();
    if sns.is_empty() {
        sns = manifest["supportedSns"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
    }
    let mut v = json!({
        "name": l.and_then(|l| l.name.clone()).unwrap_or_else(|| manifest["name"].as_str().unwrap_or("").to_string()),
        "summary": summary,
        "description": l.and_then(|l| l.description.clone()).unwrap_or_else(|| desc.to_string()),
        "sns": sns,
        "hasAds": l.map(|l| l.has_ads).unwrap_or(false),
        "price": l.and_then(|l| l.price).unwrap_or(0.0),
    });
    if let Some(c) = l.and_then(|l| l.category.clone()) {
        v["category"] = json!(c);
    }
    // アセットパックは同じワークスペースのプラグインと掲載を分ける: 名前 / 説明は自分の manifest、カテゴリは assets。
    if manifest["kind"] == "assets" {
        v["name"] = manifest["name"].clone();
        v["summary"] = json!(desc.chars().take(80).collect::<String>());
        v["description"] = json!(desc);
        v["category"] = json!("assets");
    }
    if let Some(u) = l.and_then(|l| l.icon_url.clone()) {
        v["iconUrl"] = json!(u);
    }
    if let Some(d) = l.and_then(|l| l.developer.clone()) {
        v["developer"] = json!({"name": d});
    }
    v
}

pub struct Published {
    pub id: String,
    pub version: String,
}

/// dist/ の配布物(plan の順)を Central に投げる。
pub fn run(root: &Path, ws: &Workspace, packages: &[(String, Value)], opts: &Options) -> Result<Vec<Published>> {
    let base = opts.central.trim_end_matches('/').to_string();
    let dry_dir = if opts.dry_run {
        let d = root.join("dist/publish");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d)?;
        Some(d)
    } else {
        None
    };
    let top_dry = dry_dir.clone();
    let mut c = Client { base: base.clone(), token: opts.token.clone(), agent: ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(600)).build(), dry_dir, seq: 0 };
    let mut out = Vec::new();

    for (file_name, bundle) in packages {
        let id = bundle["root"]["id"].as_str().unwrap_or("").to_string();
        let version = bundle["root"]["version"].as_str().unwrap_or("").to_string();
        let pkg_path = root.join("dist").join(file_name);
        let bytes = std::fs::read(&pkg_path).with_context(|| format!("{} がありません(rasq build を先に)", pkg_path.display()))?;
        let manifest_rel = bundle["root"]["manifest"].as_str().unwrap_or("manifest.json");
        let manifest: Value = {
            let ins = crate::pkg::inspect(&bytes)?;
            if !ins.problems.is_empty() {
                bail!("{}: {}", file_name, ins.problems.join("; "));
            }
            let m = ins.entries.iter().find(|e| e.path == manifest_rel).ok_or_else(|| anyhow!("{file_name} に {manifest_rel} がありません"))?;
            serde_json::from_slice(&m.bytes)?
        };
        println!("{file_name} → {base} ({id} {version}, channel {})", opts.channel);
        if let Some(d) = &top_dry {
            let sub = d.join(file_name.trim_end_matches(".rasqpkg"));
            std::fs::create_dir_all(&sub)?;
            c.dry_dir = Some(sub);
            c.seq = 0;
        }

        // 1. 公開中の版(権限の差分)
        let (st, prev) = c.send(Request { label: "get-manifest".into(), method: "GET", path: format!("/api/v1/plugins/{id}/manifest"), query: vec![], body: Body::None }, false)?;
        let new_perms: Vec<&str> = manifest["permissions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let permissions_added: Vec<&str> = if st == 200 {
            let old: Vec<&str> = prev["manifest"]["permissions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let added: Vec<&str> = new_perms.iter().copied().filter(|p| !old.contains(p)).collect();
            println!("  公開中: {}(権限の増加: {:?})", prev["version"].as_str().unwrap_or("?"), added);
            added
        } else {
            if st != 0 {
                println!("  公開中の版なし(HTTP {st})→ 新規");
            }
            vec![]
        };

        // 2. 掲載
        if !opts.skip_listing {
            let (st, body) = c.send(Request { label: "upsert-plugin".into(), method: "PUT", path: format!("/api/v1/admin/plugins/{id}"), query: vec![], body: Body::Json(listing(ws, &manifest)) }, true)?;
            if st != 0 && !(200..300).contains(&st) {
                return Err(fail("掲載(PUT /admin/plugins/{id})", st, &body));
            }
            if st != 0 {
                println!("  掲載: ok");
            }
        }

        // 3. 版
        let body = json!({"version": version, "channel": opts.channel, "manifest": manifest, "notes": opts.notes, "permissionsAdded": permissions_added, "makeCurrent": opts.make_current});
        let (st, res) = c.send(Request { label: "publish-version".into(), method: "POST", path: format!("/api/v1/admin/plugins/{id}/versions"), query: vec![], body: Body::Json(body) }, true)?;
        if st != 0 && !(200..300).contains(&st) {
            return Err(fail("版(POST /admin/plugins/{id}/versions)", st, &res));
        }
        if st != 0 {
            println!("  版 {version}: ok");
        }

        // 4. 配布物
        let local_sha = hex(&Sha512::digest(&bytes));
        let (st, res) = c.send(
            Request { label: "upload-bundle".into(), method: "PUT", path: format!("/api/v1/admin/plugins/{id}/versions/{version}/artifacts/bundle"), query: vec![("kind".into(), "bundle".into())], body: Body::Bytes(bytes.clone(), "application/zstd") },
            true,
        )?;
        if st != 0 {
            if !(200..300).contains(&st) {
                return Err(fail("配布物(PUT …/artifacts/bundle)", st, &res));
            }
            let remote_sha = res["artifact"]["sha512"].as_str().unwrap_or("");
            if remote_sha != local_sha {
                bail!("配布物の sha512 が Central の応答と違います(手元 {local_sha}、Central {remote_sha})");
            }
            println!("  配布物 bundle: ok({} bytes、sha512 一致、ストア署名 {}…、ストア公開鍵 {})", bytes.len(), res["artifact"]["signature"].as_str().unwrap_or("").chars().take(12).collect::<String>(), res["storePublicKey"].as_str().unwrap_or("?"));
        }

        // 5. lazy モデル
        for m in bundle["models"].as_array().into_iter().flatten().filter(|m| m["lazy"] == true) {
            let Some(aid) = m["artifactId"].as_str() else { continue };
            let mpath = m["manifest"].as_str().unwrap_or("");
            let doc: Value = serde_json::from_slice(&std::fs::read(root.join(mpath)).with_context(|| format!("{mpath} を読めません"))?)?;
            let Some(file) = doc["runtimes"].as_object().and_then(|r| r.values().find_map(|rt| rt["file"].as_str())) else {
                println!("  model {aid}: runtimes.*.file が無いので送りません");
                continue;
            };
            let fb = match std::fs::read(root.join(file)) {
                Ok(b) => b,
                Err(_) => {
                    println!("  model {aid}: {file} が手元に無いので送りません(別途 PUT …/artifacts/{aid}?kind=model&lazy=true)");
                    continue;
                }
            };
            let (st, res) = c.send(
                Request { label: format!("upload-{aid}"), method: "PUT", path: format!("/api/v1/admin/plugins/{id}/versions/{version}/artifacts/{aid}"), query: vec![("kind".into(), "model".into()), ("lazy".into(), "true".into())], body: Body::Bytes(fb, "application/octet-stream") },
                true,
            )?;
            if st != 0 && !(200..300).contains(&st) {
                return Err(fail(&format!("model {aid}"), st, &res));
            }
            if st != 0 {
                println!("  model {aid}: ok");
            }
        }
        out.push(Published { id, version });
    }
    Ok(out)
}
