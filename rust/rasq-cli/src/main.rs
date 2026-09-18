//! `rasq` — RASQ プラグイン作者向け CLI(docs/cli.md、D-244)。
//!
//! 第一期の範囲: `init` / `new` / `check` / `test` / `build --raw` / `registry`。
//! `token` / `dev` / `.rasqpkg` / `publish` は docs/cli.md §9 の順で足す。

mod embed;
mod schema;
mod wasm;
mod workspace;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;
use sha2::{Digest, Sha512};
use std::path::{Path, PathBuf};
use std::process::Command;
use workspace::{Base, PluginEntry, Sns, Workspace};

#[derive(Parser)]
#[command(name = "rasq", version, about = "RASQ プラグイン作者向け CLI(rasq-sdk-sample docs/cli.md)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// ワークスペースを作る(土台の SNS プラグインを読む / 自分の SNS プラグインを作る / 単体)
    Init {
        /// 作る場所(既定: カレント)
        dir: Option<PathBuf>,
        /// 自分で SNS プラグインを作る(id を指定。土台は読まない)
        #[arg(long, value_name = "ID")]
        sns: Option<String>,
        /// 土台の SNS プラグイン(manifest.json と .wasm があるディレクトリ)。.rasqpkg は第 ④ 段
        #[arg(long, value_name = "DIR")]
        base: Option<PathBuf>,
        /// 土台が自分のもの(子を同梱する、D-244c)
        #[arg(long)]
        owned: bool,
        /// 子プラグインの id の接頭辞(土台が無い / 他人のものの場合)
        #[arg(long, default_value = "local.dev")]
        namespace: String,
    },
    /// スロットの雛形から子プラグインを作る(土台の配下にバンドル)
    New {
        /// スロット id(rasq registry で一覧)
        slot: String,
        /// プラグインの名前(ディレクトリ名 / crate 名)
        name: String,
        /// pluginId を直接指定(既定: <土台 or namespace>.<name>)
        #[arg(long)]
        id: Option<String>,
        /// 表示名(既定: name)
        #[arg(long)]
        display: Option<String>,
        /// planned のスロットも許す
        #[arg(long)]
        allow_planned: bool,
    },
    /// wasm32 ビルド → export → plugin.manifest 往復 → スキーマ / 登録簿 → cargo test
    Check {
        #[arg(long)]
        no_build: bool,
        #[arg(long)]
        no_test: bool,
    },
    /// cargo test(ホスト関数は kit の set_host_hook でモック)
    Test,
    /// デプロイ向けビルド。第一期は --raw(子ごとの .wasm を dist/ へ)だけ
    Build {
        /// 生の .wasm を dist/ に出す(今のホストが読める形。.rasqpkg は第 ④ 段)
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        no_build: bool,
    },
    /// 同梱の登録簿 / 雛形を表示
    Registry {
        #[arg(value_enum, default_value_t = What::Slots)]
        what: What,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum What {
    Slots,
    Templates,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Init { dir, sns, base, owned, namespace } => init(dir.unwrap_or_else(|| ".".into()), sns, base, owned, &namespace),
        Cmd::New { slot, name, id, display, allow_planned } => new(&slot, &name, id, display, allow_planned),
        Cmd::Check { no_build, no_test } => check(no_build, no_test),
        Cmd::Test => {
            let (root, _) = Workspace::find()?;
            cargo(&root, &["test"])
        }
        Cmd::Build { raw, no_build } => build(raw, no_build),
        Cmd::Registry { what } => registry(what),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn init(dir: PathBuf, sns: Option<String>, base: Option<PathBuf>, owned: bool, namespace: &str) -> Result<()> {
    if sns.is_some() && base.is_some() {
        bail!("--sns と --base は同時に使えません(SNS プラグインを作るときは土台を読まない)");
    }
    workspace::valid_plugin_id(namespace).context("--namespace")?;
    std::fs::create_dir_all(&dir)?;
    if dir.join(workspace::FILE).exists() {
        bail!("{} は既にワークスペースです", dir.display());
    }
    let mut ws = Workspace::new(embed::SDK_VERSION, namespace);
    embed::write_kit(&dir.join("kit"))?;
    std::fs::create_dir_all(dir.join("plugins"))?;
    std::fs::write(dir.join(".gitignore"), "/target\n/dist\n/base\n/.rasq\n")?;

    if let Some(id) = sns {
        workspace::valid_plugin_id(&id)?;
        let display = id.rsplit('.').next().unwrap_or("sns").to_string();
        let vars = [("id", id.as_str()), ("name", "sns"), ("display", display.as_str()), ("slot", "sns"), ("kit_path", "../kit")];
        embed::render_template("sns", &dir.join("sns"), &vars)?;
        ws.sns = Some(Sns { id: id.clone(), dir: "sns".into() });
        println!("SNS プラグイン {id} の雛形を sns/ に作りました");
    }
    if let Some(src) = base {
        let b = load_base(&src, &dir.join("base"))?;
        println!("土台 {} {} を base/ に置きました(owned = {owned})", b.id, b.version);
        ws.base = Some(Base { owned, ..b });
    }
    ws.save(&dir)?;
    println!("ワークスペースを作りました: {}\n  次: rasq new <slot> <name>(rasq registry でスロット一覧)", dir.join(workspace::FILE).display());
    Ok(())
}

/// `manifest.json` + `*.wasm` のディレクトリを base/ に写す(kind = sns だけ)。
fn load_base(src: &Path, dest: &Path) -> Result<Base> {
    let manifest_path = src.join("manifest.json");
    let m: Value = serde_json::from_slice(&std::fs::read(&manifest_path).with_context(|| format!("{} を読めません", manifest_path.display()))?)?;
    if m["kind"] != "sns" {
        bail!("土台は kind = sns のプラグインだけです: {}", m["kind"]);
    }
    let id = m["id"].as_str().ok_or_else(|| anyhow!("土台の manifest に id がありません"))?.to_string();
    let version = m["version"].as_str().unwrap_or("0.0.0").to_string();
    let wasm = std::fs::read_dir(src)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "wasm"))
        .ok_or_else(|| anyhow!("{} に .wasm がありません", src.display()))?;
    std::fs::create_dir_all(dest)?;
    std::fs::copy(&manifest_path, dest.join("manifest.json"))?;
    std::fs::copy(&wasm, dest.join(wasm.file_name().unwrap()))?;
    Ok(Base { id, version, source: format!("file:{}", src.display()), owned: false, account: None })
}

// ---------------------------------------------------------------------------
// new
// ---------------------------------------------------------------------------

fn new(slot: &str, name: &str, id: Option<String>, display: Option<String>, allow_planned: bool) -> Result<()> {
    let (root, mut ws) = Workspace::find()?;
    workspace::valid_name(name)?;
    if name == "sns" || name == "kit" || ws.plugin(name).is_some() {
        bail!("その名前は使えません(予約語か既にある): {name}");
    }
    let reg = embed::registry()?;
    let s = reg.iter().find(|s| s.id == slot).ok_or_else(|| anyhow!("slot {slot} は登録簿にありません(rasq registry)"))?;
    if s.status == "planned" && !allow_planned {
        bail!("slot {slot} は planned(まだホストが無い)。--allow-planned で作れますが rasq check は警告します");
    }
    if !embed::template_names().iter().any(|t| t == slot) {
        bail!("slot {slot} の雛形はまだありません(rasq registry templates)");
    }
    let id = id.unwrap_or_else(|| ws.child_id(name));
    workspace::valid_plugin_id(&id)?;
    let display = display.unwrap_or_else(|| name.to_string());
    let vars = [("id", id.as_str()), ("name", name), ("display", display.as_str()), ("slot", slot), ("kit_path", "../../kit")];
    let dest = root.join("plugins").join(name);
    if dest.exists() {
        bail!("{} が既にあります", dest.display());
    }
    let files = embed::render_template(slot, &dest, &vars)?;
    ws.plugins.push(PluginEntry { name: name.into(), slot: slot.into(), id: id.clone() });
    ws.save(&root)?;
    println!("plugins/{name}/ を作りました(id = {id}, slot = {slot}): {}", files.join(", "));
    println!("  次: src/lib.rs の handle を書く → rasq check");
    Ok(())
}

// ---------------------------------------------------------------------------
// check / build
// ---------------------------------------------------------------------------

fn cargo(root: &Path, args: &[&str]) -> Result<()> {
    println!("$ cargo {}", args.join(" "));
    let status = Command::new("cargo").args(args).current_dir(root).status().context("cargo を起動できません(Rust を入れてください)")?;
    if !status.success() {
        bail!("cargo {} が失敗しました({status})", args.join(" "));
    }
    Ok(())
}

fn build_wasm(root: &Path) -> Result<()> {
    cargo(root, &["build", "--release", "--target", "wasm32-unknown-unknown"])
        .context("wasm32 ビルドに失敗(rustup target add wasm32-unknown-unknown は済んでいますか)")
}

fn cargo_version(dir: &Path) -> Option<String> {
    let t: toml::Value = toml::from_str(&std::fs::read_to_string(dir.join("Cargo.toml")).ok()?).ok()?;
    t.get("package")?.get("version")?.as_str().map(String::from)
}

fn check(no_build: bool, no_test: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    let targets = ws.targets(&root);
    if targets.is_empty() {
        bail!("ビルド対象がありません(rasq new <slot> <name>)");
    }
    if !no_build {
        build_wasm(&root)?;
    }
    let validator = schema::manifest_validator()?;
    let reg = embed::registry()?;
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for t in &targets {
        let label = t.dir.strip_prefix(&root).unwrap_or(&t.dir).display().to_string();
        let mut errs: Vec<String> = Vec::new();
        let mut warns: Vec<String> = Vec::new();

        // 1. wasm の export / import
        let wasm_path = t.wasm_path(&root, "release");
        let bytes = match std::fs::read(&wasm_path) {
            Ok(b) => b,
            Err(e) => {
                report(&label, &[format!("{}: {e}", wasm_path.display())], &[]);
                errors += 1;
                continue;
            }
        };
        let manifest_file: Value = match std::fs::read(t.dir.join("manifest.json")).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice(&b)?)) {
            Ok(v) => v,
            Err(e) => {
                report(&label, &[format!("manifest.json: {e}")], &[]);
                errors += 1;
                continue;
            }
        };
        match wasm::inspect(&bytes) {
            Ok(i) => errs.extend(i.problems()),
            Err(e) => errs.push(format!("{e:#}")),
        }

        // 2. plugin.manifest の往復
        if errs.is_empty() {
            match wasm::manifest_from_wasm(&bytes) {
                Ok(from_wasm) => {
                    if from_wasm != manifest_file {
                        let mut keys: Vec<&String> = from_wasm.as_object().into_iter().flatten().chain(manifest_file.as_object().into_iter().flatten()).map(|(k, _)| k).collect();
                        keys.sort();
                        keys.dedup();
                        let diff: Vec<&str> = keys.into_iter().filter(|k| from_wasm.get(*k) != manifest_file.get(*k)).map(String::as_str).collect();
                        errs.push(format!("wasm の plugin.manifest と manifest.json が違います: {diff:?}"));
                    }
                }
                Err(e) => errs.push(format!("{e:#}")),
            }
        }

        // 3. スキーマ + 登録簿
        let f = schema::check_manifest(&validator, &reg, &manifest_file, t.slot.as_deref(), cargo_version(&t.dir).as_deref());
        errs.extend(f.errors);
        warns.extend(f.warnings);
        if manifest_file["id"].as_str() != Some(t.id.as_str()) {
            errs.push(format!("manifest.id {} が rasq.toml の id {} と違います", manifest_file["id"], t.id));
        }
        if t.slot.is_none() && manifest_file["kind"] != "sns" {
            errs.push("sns/ の manifest は kind = sns でなければなりません".into());
        }
        let size = bytes.len();
        report(&label, &errs, &warns);
        if errs.is_empty() {
            println!("  ok: {} ({} KB)", wasm_path.strip_prefix(&root).unwrap_or(&wasm_path).display(), size / 1024);
        }
        errors += errs.len();
        warnings += warns.len();
    }

    // 4. libs / assets / models の実在
    for (what, p) in ws.libs.iter().map(|l| ("lib", l.path.as_str())).chain(ws.assets.iter().map(|a| ("asset", a.path.as_str()))).chain(ws.models.iter().map(|m| ("model", m.manifest.as_str()))) {
        if !root.join(p).is_file() {
            println!("rasq.toml: {what} {p} がありません");
            errors += 1;
        }
    }

    println!("{} targets, {errors} errors, {warnings} warnings", targets.len());
    if errors > 0 {
        bail!("rasq check に失敗しました");
    }
    if !no_test {
        cargo(&root, &["test"])?;
    }
    Ok(())
}

fn report(label: &str, errs: &[String], warns: &[String]) {
    println!("{label}:");
    for w in warns {
        println!("  warn: {w}");
    }
    for e in errs {
        println!("  ERROR: {e}");
    }
}

fn build(raw: bool, no_build: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    if !raw {
        println!("note: .rasqpkg は第 ④ 段(docs/cli.md §6)。今は --raw と同じく生の .wasm を dist/ に出します");
    }
    if !no_build {
        build_wasm(&root)?;
    }
    let dist = root.join("dist");
    std::fs::create_dir_all(&dist)?;
    for t in ws.targets(&root) {
        let src = t.wasm_path(&root, "release");
        let bytes = std::fs::read(&src).with_context(|| format!("{} がありません(rasq check を先に)", src.display()))?;
        let out = dist.join(src.file_name().unwrap());
        std::fs::write(&out, &bytes)?;
        let digest = Sha512::digest(&bytes);
        println!("{}  {} bytes  sha512:{}", out.strip_prefix(&root).unwrap_or(&out).display(), bytes.len(), hex(&digest));
    }
    println!("rasq-client: 「WASMプラグイン試験 (dev)」→ .wasm のパスからインポート に上の絶対パス({})を入れる", dist.display());
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

fn registry(what: What) -> Result<()> {
    let templates = embed::template_names();
    match what {
        What::Templates => {
            println!("雛形(templates/、SDK {}):", embed::SDK_VERSION);
            for t in templates {
                println!("  {t}");
            }
        }
        What::Slots => {
            println!("{:<18} {:<9} {:<8} {:<28} {:<22} 雛形", "slot", "status", "tier", "kinds", "required");
            for s in embed::registry()? {
                println!(
                    "{:<18} {:<9} {:<8} {:<28} {:<22} {}",
                    s.id,
                    s.status,
                    s.tier,
                    s.kinds.join(","),
                    s.required.join(","),
                    if templates.contains(&s.id) { "✓" } else { "-" }
                );
            }
            println!("(sns プラグイン自体の雛形は rasq init --sns <id>)");
        }
    }
    Ok(())
}
