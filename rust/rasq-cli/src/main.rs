//! `rasq` — RASQ プラグイン作者向け CLI(docs/cli.md、D-244)。
//!
//! 第一期(①〜⑤): `init` / `new` / `check` / `test` / `call` / `build [--raw]` / `registry` / `token` / `dev` / `key` / `inspect` / `publish`。

mod credentials;
mod source;
mod embed;
mod help;
mod key;
mod pack;
mod pkg;
mod publish;
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
#[command(name = "rasq", version, about = "RASQ プラグイン作者向け CLI(rasq-sdk-sample docs/cli.md)", disable_help_subcommand = true)]
// `rasq help <言葉…>` は文書の検索(help.rs)。clap の help サブコマンドは外す(--help / -h は従来どおり)。
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
        /// 土台の SNS プラグイン(.rasqpkg、または manifest.json と .wasm があるディレクトリ)
        #[arg(long, value_name = "PKG|DIR")]
        base: Option<PathBuf>,
        /// 土台が自分のもの(子を同梱する、D-244c)
        #[arg(long)]
        owned: bool,
        /// 子プラグインの id の接頭辞(土台が無い / 他人のものの場合)
        #[arg(long, default_value = "local.dev")]
        namespace: String,
    },
    /// スロットの雛形から子プラグインを作る(土台の配下にバンドル)。assets = アセットパック、theme = 着せ替え(どちらも wasm 無し、packs/<name>/)
    New {
        /// スロット id(rasq registry で一覧)か assets / theme
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
    /// ビルド済みの wasm を wasmi で動かして call を 1 つ(以上)叩く。ホスト関数は --reply で答えを差し込める(無ければ E01)
    Call {
        /// プラグインの名前(plugins/<name>、SNS プラグインは sns)
        name: String,
        /// 呼ぶ fn(例 viewer.open)
        r#fn: String,
        /// args の JSON。@path でファイルから。値の中の "@b64:<path>" はそのファイルの base64 に置き換える
        #[arg(long, default_value = "{}")]
        args: String,
        /// ホスト関数の答え: fn=JSON(例 --reply 'fow.open={"opened":true}')。何度でも
        #[arg(long, value_name = "FN=JSON")]
        reply: Vec<String>,
        /// 続けて呼ぶ fn(同じインスタンスで)。`FN` なら args は {}、`FN=JSON` / `FN=@path` で args を渡せる。何度でも
        #[arg(long, value_name = "FN[=JSON|@path]")]
        then: Vec<String>,
        /// 長い文字列(base64 など)を縮めずに出す
        #[arg(long)]
        full: bool,
    },
    /// デプロイ向けビルド: dist/<id>-<version>.rasqpkg(+ 開発者署名 .sig.json)。--raw なら生の .wasm も
    Build {
        /// 生の .wasm も dist/ に出す(今のホストのパス読み込み / ストアの生 wasm 配布(D-168)向け)
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        no_build: bool,
        /// 開発者鍵で署名しない
        #[arg(long)]
        no_sign: bool,
        /// 審査用のソース dist/<id>-<version>.source.tar.zst も出す(配布はされず、審査だけが読む)
        #[arg(long)]
        with_source: bool,
    },
    /// 開発者鍵(ed25519)。サイドロード用の署名に使う。ストア署名は Central 側
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Central の管理 API へ公開する(掲載 → 版 → 配布物)。--dry-run で要求を dist/publish/ に書き出すだけ
    Publish {
        /// Central の URL(例 http://100.91.218.56:8080)
        #[arg(long)]
        central: String,
        #[arg(long, default_value = "stable", value_parser = ["stable", "beta"])]
        channel: String,
        /// リリースノート(Markdown)のファイル
        #[arg(long, value_name = "FILE")]
        notes_file: Option<PathBuf>,
        /// 要求を dist/publish/ に書くだけ(Central には触らない。トークン不要)
        #[arg(long)]
        dry_run: bool,
        /// 掲載(PUT /admin/plugins/{id})を送らない(既に掲載があるとき)
        #[arg(long)]
        skip_listing: bool,
        /// そのチャンネルの現在版にしない
        #[arg(long)]
        no_make_current: bool,
        /// dist/ を作り直さない(既にある .rasqpkg を送る)
        #[arg(long)]
        no_build: bool,
        /// トークンを読む環境変数(無ければ credentials.toml の [central."<host>"])
        #[arg(long, default_value = "RASQ_CENTRAL_TOKEN", value_name = "VAR")]
        token_env: String,
        /// 審査用のソースも送る(kind = source。審査から求められたら必須)
        #[arg(long)]
        with_source: bool,
    },
    /// .rasqpkg の中身を目次(bundle.json)と突き合わせ、.sig.json があれば署名も確かめる
    Inspect {
        pkg: PathBuf,
        /// 中身をこのディレクトリに展開する
        #[arg(long, value_name = "DIR")]
        extract: Option<PathBuf>,
    },
    /// 同梱の文書(docs/、PROTOCOL.md、登録簿、マニフェストの属性)から探す。--central でほかのプラグインの掲載も
    Help {
        /// 関数名 / スロット / 属性 / 探したい言葉(空なら文書の一覧)
        query: Vec<String>,
        /// 出す節の数
        #[arg(short = 'n', long, default_value_t = 5)]
        count: usize,
        /// 節を省略せずに全部出す
        #[arg(long)]
        full: bool,
        /// Central の URL(掲載 = ほかのプラグインの文書も探す。トークン不要)
        #[arg(long, value_name = "URL")]
        central: Option<String>,
        /// --central と一緒に: そのプラグインの掲載の説明を読む
        #[arg(long, value_name = "ID")]
        plugin: Option<String>,
    },
    /// 同梱の登録簿 / 雛形を表示
    Registry {
        #[arg(value_enum, default_value_t = What::Slots)]
        what: What,
    },
    /// Access Token(データを流すアカウント)。~/.config/rasq/credentials.toml(0600)に置き、リポジトリには accountKey だけ書く
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// 確認用ビルド: ビルド → 往復検査 → rasq-client の「WASMプラグイン試験 (dev)」に入れる絶対パスを出す(+ .rasq/dev.json)
    Dev {
        /// 変更のたびに再ビルドして出し直す(Ctrl-C で止める)
        #[arg(long)]
        watch: bool,
        /// debug プロファイルで組む(既定は release: wasm はインタープリタで動くので debug は大きく遅い)
        #[arg(long)]
        debug: bool,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// 鍵を作る(~/.config/rasq/dev-key.toml、0600)
    Init {
        /// 既にあっても作り直す
        #[arg(long)]
        force: bool,
    },
    /// 公開鍵(base64)と指紋
    Show,
}

#[derive(Subcommand)]
enum TokenCmd {
    /// トークンを保存する。端末なら非表示入力、パイプなら標準入力の 1 行、--from-env なら環境変数(argv では受け取らない)
    Set {
        /// kind:instance:handle(例 misskey:misskey.io:yamada)。--central なら Central の host[:port]
        account_key: String,
        /// Central(publish)のトークンとして保存する
        #[arg(long)]
        central: bool,
        /// トークンを環境変数から読む
        #[arg(long, value_name = "VAR")]
        from_env: Option<String>,
        /// 接続先(既定は accountKey の instance。テスト用サーバーに向けるとき)
        #[arg(long)]
        host: Option<String>,
        /// rasq.toml の [base]/[sns] の account に書かない
        #[arg(long)]
        no_workspace: bool,
    },
    /// 保存してあるアカウント(トークンは出さない)
    List,
    /// トークンを消す
    Remove { account_key: String },
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
        Cmd::Call { name, r#fn, args, reply, then, full } => call_cmd(&name, &r#fn, &args, &reply, &then, full),
        Cmd::Build { raw, no_build, no_sign, with_source } => build(raw, no_build, no_sign, with_source),
        Cmd::Key { cmd } => key_cmd(cmd),
        Cmd::Inspect { pkg, extract } => inspect(&pkg, extract.as_deref()),
        Cmd::Publish { central, channel, notes_file, dry_run, skip_listing, no_make_current, no_build, token_env, with_source } => publish_cmd(central, channel, notes_file, dry_run, skip_listing, !no_make_current, no_build, &token_env, with_source),
        Cmd::Help { query, count, full, central, plugin } => help::run(&query, count, full, central.as_deref(), plugin.as_deref()),
        Cmd::Registry { what } => registry(what),
        Cmd::Token { cmd } => token(cmd),
        Cmd::Dev { watch, debug } => dev(watch, debug),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn init(dir: PathBuf, sns: Option<String>, base: Option<PathBuf>, owned: bool, namespace: &str) -> Result<()> {
    if sns.is_some() && base.is_some() {
        bail!("--sns と --base は同時に使えません(SNS プラグインを作るときは土台を読まない)");
    }
    workspace::valid_namespace(namespace).context("--namespace")?;
    std::fs::create_dir_all(&dir)?;
    if dir.join(workspace::FILE).exists() {
        bail!("{} は既にワークスペースです", dir.display());
    }
    let mut ws = Workspace::new(embed::SDK_VERSION, namespace);
    embed::write_kit(&dir.join("kit"))?;
    std::fs::create_dir_all(dir.join("plugins"))?;
    // dist/ は無視しない(配布物をリポジトリに残す流儀 = plugin-sample)。署名だけは鍵ごとのものなので入れない。
    std::fs::write(dir.join(".gitignore"), "/target\n/base\n/.rasq\n*.sig.json\n")?;

    if let Some(id) = sns {
        workspace::valid_plugin_id(&id)?;
        let display = id.rsplit('.').next().unwrap_or("sns").to_string();
        let vars = [("id", id.as_str()), ("name", "sns"), ("display", display.as_str()), ("slot", "sns"), ("kit_path", "../kit")];
        embed::render_template("sns", &dir.join("sns"), &vars)?;
        ws.sns = Some(Sns { id: id.clone(), dir: "sns".into(), account: None });
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

/// 土台を base/ に写す(kind = sns だけ): `.rasqpkg` なら展開、ディレクトリなら `manifest.json` + `*.wasm`。
fn load_base(src: &Path, dest: &Path) -> Result<Base> {
    if src.is_file() {
        let bytes = std::fs::read(src).with_context(|| format!("{} を読めません", src.display()))?;
        let ins = pkg::inspect(&bytes)?;
        if !ins.problems.is_empty() {
            bail!("{} に問題があります:\n  {}", src.display(), ins.problems.join("\n  "));
        }
        let root = &ins.bundle["root"];
        if root["kind"] != "sns" {
            bail!("土台は root.kind = sns の配布物だけです: {}", root["kind"]);
        }
        let find = |p: &str| ins.entries.iter().find(|e| e.path == p).map(|e| e.bytes.clone()).ok_or_else(|| anyhow!("{p} が配布物にありません"));
        let manifest = find(root["manifest"].as_str().unwrap_or(""))?;
        let wasm_path = root["wasm"]["path"].as_str().unwrap_or("");
        let wasm = find(wasm_path)?;
        std::fs::create_dir_all(dest)?;
        std::fs::write(dest.join("manifest.json"), &manifest)?;
        std::fs::write(dest.join(Path::new(wasm_path).file_name().unwrap()), &wasm)?;
        std::fs::write(dest.join(pkg::BUNDLE), &ins.entries[0].bytes)?;
        return Ok(Base {
            id: root["id"].as_str().unwrap_or("").into(),
            version: root["version"].as_str().unwrap_or("0.0.0").into(),
            source: format!("file:{}", src.display()),
            owned: false,
            account: None,
        });
    }
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
    if name == "sns" || name == "kit" || ws.plugin(name).is_some() || ws.packs.iter().any(|p| p.name == name) {
        bail!("その名前は使えません(予約語か既にある): {name}");
    }
    if pack::PACK_KINDS.contains(&slot) {
        return new_pack(&root, ws, slot, name, id, display);
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

/// `rasq new assets <name>` / `rasq new theme <name>`: アセットパック / 着せ替え(wasm 無し、`packs/<name>/`)。
/// 土台に依らないので id は常に `<namespace>.<name>`。`kind` は雛形の名前でもある(templates/assets、templates/theme)。
fn new_pack(root: &Path, mut ws: Workspace, kind: &str, name: &str, id: Option<String>, display: Option<String>) -> Result<()> {
    let id = id.unwrap_or_else(|| format!("{}.{name}", ws.namespace()));
    workspace::valid_plugin_id(&id)?;
    let display = display.unwrap_or_else(|| name.to_string());
    let dest = root.join("packs").join(name);
    if dest.exists() {
        bail!("{} が既にあります", dest.display());
    }
    let vars = [("id", id.as_str()), ("name", name), ("display", display.as_str())];
    let files = embed::render_template(kind, &dest, &vars)?;
    ws.packs.push(workspace::PackEntry { name: name.into(), id: id.clone() });
    ws.save(root)?;
    println!("packs/{name}/ を作りました(id = {id}, kind = {kind}): {}", files.join(", "));
    if kind == "theme" {
        println!("  次: theme/ に画像を置いて manifest.json の assets(use = theme)に書き、theme.surfaces / icons から指す → rasq check");
    } else {
        println!("  次: stickers/ に画像を置いて manifest.json の assets に書く → rasq check");
    }
    Ok(())
}

/// アセットパック / 着せ替えの検査(wasm のビルドは要らない)。戻りは (errors, warnings)。
fn verify_packs(root: &Path, ws: &Workspace) -> Result<(usize, usize, Vec<pack::Checked>)> {
    let (mut errors, mut warnings, mut out) = (0, 0, Vec::new());
    for p in &ws.packs {
        let c = pack::check(&root.join("packs").join(&p.name), p)?;
        report(&format!("packs/{}", p.name), &c.errors, &c.warnings);
        if c.errors.is_empty() {
            println!("  ok: {} 枚 ({} KB)", c.files.len(), c.files.iter().map(|(_, b)| b.len()).sum::<usize>() / 1024);
        }
        errors += c.errors.len();
        warnings += c.warnings.len();
        out.push(c);
    }
    Ok((errors, warnings, out))
}

// ---------------------------------------------------------------------------
// check / build
// ---------------------------------------------------------------------------

/// `cargo` を起動する唯一の場所。引数は呼び出し元の固定のリテラル (`test` / `build --target …`) だけで、
/// rasq.toml / manifest / 他人の配布物から来た文字列は入れない。作業ディレクトリはワークスペースの root。
fn cargo(root: &Path, args: &[&str]) -> Result<()> {
    println!("$ cargo {}", args.join(" "));
    // nosemgrep: rasq-command-spawn — 上のとおり、コマンド名と引数は固定
    let status = Command::new("cargo").args(args).current_dir(root).status().context("cargo を起動できません(Rust を入れてください)")?;
    if !status.success() {
        bail!("cargo {} が失敗しました({status})", args.join(" "));
    }
    Ok(())
}

fn build_wasm(root: &Path, profile: &str) -> Result<()> {
    let mut args = vec!["build", "--target", "wasm32-unknown-unknown"];
    if profile == "release" {
        args.insert(1, "--release");
    }
    cargo(root, &args).context("wasm32 ビルドに失敗(rustup target add wasm32-unknown-unknown は済んでいますか)")
}

fn cargo_version(dir: &Path) -> Option<String> {
    let t: toml::Value = toml::from_str(&std::fs::read_to_string(dir.join("Cargo.toml")).ok()?).ok()?;
    t.get("package")?.get("version")?.as_str().map(String::from)
}

fn check(no_build: bool, no_test: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    let has_code = !ws.targets(&root).is_empty();
    if !has_code && ws.packs.is_empty() {
        bail!("ビルド対象がありません(rasq new <slot> <name>)");
    }
    if has_code && !no_build {
        build_wasm(&root, "release")?;
    }
    let mut v = if has_code { verify(&root, &ws, "release")? } else { Verified { targets: 0, errors: 0, warnings: 0 } };
    let (pe, pw, packs) = verify_packs(&root, &ws)?;
    v.targets += packs.len();
    v.errors += pe;
    v.warnings += pw;
    println!("{} targets, {} errors, {} warnings", v.targets, v.errors, v.warnings);
    if v.errors > 0 {
        bail!("rasq check に失敗しました");
    }
    if has_code && !no_test {
        cargo(&root, &["test"])?;
    }
    Ok(())
}

struct Verified {
    targets: usize,
    errors: usize,
    warnings: usize,
}

/// docs/cli.md §5 の 2〜5: export / import → plugin.manifest の往復 → スキーマ + 登録簿 → libs / assets / models の実在。
fn verify(root: &Path, ws: &Workspace, profile: &str) -> Result<Verified> {
    let targets = ws.targets(root);
    let validator = schema::manifest_validator()?;
    let reg = embed::registry()?;
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for t in &targets {
        let label = t.dir.strip_prefix(root).unwrap_or(&t.dir).display().to_string();
        let mut errs: Vec<String> = Vec::new();
        let mut warns: Vec<String> = Vec::new();

        // 1. wasm の export / import
        let wasm_path = t.wasm_path(root, profile);
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
        // 権限の理由 ([listing.safety])。審査 (rasq-judge の 1 段目) と同じ基準: 宣言した権限すべてに
        // 理由が要る。無いと自動では承認されない(人の列に回る)ので、出す前にここで分かるようにする。
        let declared: Vec<String> = manifest_file["permissions"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)).collect();
        if !declared.is_empty() {
            match ws.listing.as_ref().and_then(|l| l.safety.as_ref()) {
                None => warns.push(format!("[listing.safety] がありません: 権限 {} の理由を書いてください(無いと審査で自動承認されません)", declared.join(", "))),
                Some(s) => {
                    let (missing, stray) = s.gaps(&declared);
                    if !missing.is_empty() {
                        warns.push(format!("[listing.safety] に理由が無い権限: {}(無いと審査で自動承認されません)", missing.join(", ")));
                    }
                    if !stray.is_empty() {
                        warns.push(format!("[listing.safety] に、宣言していない権限の理由があります: {}", stray.join(", ")));
                    }
                }
            }
        }
        let size = bytes.len();
        report(&label, &errs, &warns);
        if errs.is_empty() {
            println!("  ok: {} ({} KB)", wasm_path.strip_prefix(root).unwrap_or(&wasm_path).display(), size / 1024);
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
    Ok(Verified { targets: targets.len(), errors, warnings })
}

/// `--args` / `--then FN=…` の値: JSON そのものか `@path`(ファイル)。中の `"@b64:<path>"` は展開する。
fn parse_call_args(args: &str, root: &Path) -> Result<Value> {
    let text = match args.strip_prefix('@') {
        Some(p) => std::fs::read_to_string(p).with_context(|| format!("{p} を読めません"))?,
        None => args.to_string(),
    };
    let mut v: Value = serde_json::from_str(&text).context("JSON ではありません")?;
    expand_b64(&mut v, root)?;
    Ok(v)
}

/// `"@b64:<path>"` をファイルの base64 に置き換える(画像を args に入れる用)。
fn expand_b64(v: &mut Value, root: &Path) -> Result<()> {
    match v {
        Value::String(s) => {
            if let Some(p) = s.strip_prefix("@b64:") {
                use base64::Engine;
                let bytes = std::fs::read(root.join(p)).or_else(|_| std::fs::read(p)).with_context(|| format!("{p} を読めません"))?;
                *s = base64::engine::general_purpose::STANDARD.encode(bytes);
            }
        }
        Value::Array(a) => {
            for x in a {
                expand_b64(x, root)?;
            }
        }
        Value::Object(o) => {
            for (_, x) in o.iter_mut() {
                expand_b64(x, root)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// 表示用: 長い文字列を縮める。
fn shorten(v: &Value) -> Value {
    match v {
        Value::String(s) if s.chars().count() > 160 => Value::String(format!("{}…({} 文字)", s.chars().take(60).collect::<String>(), s.chars().count())),
        Value::Array(a) => Value::Array(a.iter().map(shorten).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), shorten(x))).collect()),
        other => other.clone(),
    }
}

fn call_cmd(name: &str, fn_name: &str, args: &str, replies: &[String], then: &[String], full: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    let t = ws.targets(&root).into_iter().find(|t| t.package == name).ok_or_else(|| anyhow!("{name} はワークスペースにありません"))?;
    let wasm_path = t.wasm_path(&root, "release");
    let bytes = std::fs::read(&wasm_path).with_context(|| format!("{} がありません(rasq check / rasq dev を先に)", wasm_path.display()))?;
    let args = parse_call_args(args, &root).context("--args")?;
    let mut host = wasm::HostState::default();
    for r in replies {
        let (k, v) = r.split_once('=').ok_or_else(|| anyhow!("--reply は FN=JSON の形です: {r}"))?;
        host.replies.insert(k.to_string(), serde_json::from_str(v).with_context(|| format!("--reply {k} の値が JSON ではありません"))?);
    }
    let show = |v: &Value| serde_json::to_string_pretty(&if full { v.clone() } else { shorten(v) }).unwrap_or_default();
    let mut p = wasm::Plugin::load_with(&bytes, host)?;
    p.message("snapshot")?;
    let mut calls = vec![(fn_name.to_string(), args)];
    for t in then {
        let (f, a) = match t.split_once('=') {
            Some((f, a)) => (f.to_string(), parse_call_args(a, &root).with_context(|| format!("--then {f}"))?),
            None => (t.clone(), serde_json::json!({})),
        };
        calls.push((f, a));
    }
    let mut failed = false;
    for (i, (f, a)) in calls.iter().enumerate() {
        let started = std::time::Instant::now();
        let r = p.call(f, a, &format!("rasq-call-{i}"))?;
        println!("call {f} ({} ms) →", started.elapsed().as_millis());
        if r["ok"] == true {
            println!("{}", show(&r["value"]));
        } else {
            failed = true;
            println!("ERROR {}", show(&r["error"]));
        }
        for (hf, ha) in p.take_host_log() {
            println!("  host {hf} {}", show(&ha));
        }
        if r["value"]["patch"] == true {
            println!("  patch {}", show(&p.message("patch")?));
        }
    }
    if failed {
        bail!("call が失敗しました");
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

fn build(raw: bool, no_build: bool, no_sign: bool, with_source: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    build_in(&root, &ws, raw, no_build, no_sign, with_source).map(|_| ())
}

/// dist/ を作る。戻りは (ファイル名, bundle.json)。
fn build_in(root: &Path, ws: &Workspace, raw: bool, no_build: bool, no_sign: bool, with_source: bool) -> Result<Vec<(String, Value)>> {
    let root = root.to_path_buf();
    let has_code = !ws.targets(&root).is_empty();
    if !has_code && ws.packs.is_empty() {
        bail!("ビルド対象がありません(rasq new <slot> <name>)");
    }
    if has_code && !no_build {
        build_wasm(&root, "release")?;
    }
    let code_errors = if has_code { verify(&root, ws, "release")?.errors } else { 0 };
    let (pack_errors, _, packs) = verify_packs(&root, ws)?;
    if code_errors + pack_errors > 0 {
        bail!("{} errors(rasq check を通してから)", code_errors + pack_errors);
    }
    let dist = root.join("dist");
    std::fs::create_dir_all(&dist)?;

    if raw {
        for t in ws.targets(&root) {
            let src = t.wasm_path(&root, "release");
            let bytes = std::fs::read(&src)?;
            let out = dist.join(src.file_name().unwrap());
            std::fs::write(&out, &bytes)?;
            println!("{}  {} bytes  sha512:{}", out.strip_prefix(&root).unwrap_or(&out).display(), bytes.len(), hex(&Sha512::digest(&bytes)));
        }
    }

    let signer = if no_sign {
        None
    } else {
        match key::load()? {
            Some((k, sk)) => {
                println!("署名: 開発者鍵 {}(公開鍵の指紋 {})", key::path()?.display(), key::fingerprint(&k.public));
                Some(sk)
            }
            None => {
                println!("note: 開発者鍵が無いので署名しません(rasq key init で作る。--no-sign で黙らせる)");
                None
            }
        }
    };
    let mut built = Vec::new();
    let mut planned = if has_code { pkg::plan(&root, ws, "release")? } else { Vec::new() };
    for c in &packs {
        planned.push(pack::package(c)?);
    }
    for p in planned {
        let bytes = pkg::serialize(&p)?;
        let out = dist.join(&p.file_name);
        std::fs::write(&out, &bytes)?;
        let digest: [u8; 64] = Sha512::digest(&bytes).into();
        println!("{}", pkg::describe(&p));
        println!("  {}  {} bytes  sha512:{}", out.strip_prefix(&root).unwrap_or(&out).display(), bytes.len(), hex(&digest));
        if let Some(sk) = &signer {
            let sig = key::sign(sk, &digest);
            let sp = key::sig_path(&out);
            std::fs::write(&sp, serde_json::to_string_pretty(&sig)?)?;
            println!("  {}", sp.strip_prefix(&root).unwrap_or(&sp).display());
        }
        if with_source {
            // 審査用のソース (docs/cli.md §6)。配布物とは別のファイルで、publish は kind = source として送る。
            let src = source::archive(&root, ws, &p.bundle)?;
            let sp = dist.join(source::file_name(p.bundle["root"]["id"].as_str().unwrap_or("plugin"), p.bundle["root"]["version"].as_str().unwrap_or("0")));
            std::fs::write(&sp, &src)?;
            println!("  {}  {} bytes(審査用のソース。配布されない)", sp.strip_prefix(&root).unwrap_or(&sp).display(), src.len());
        }
        built.push((p.file_name.clone(), p.bundle.clone()));
    }
    if raw {
        println!("rasq-client: 「WASMプラグイン試験 (dev)」→ .wasm のパスからインポート に上の絶対パス({})を入れる", dist.display());
    } else {
        println!("rasq-client: 「WASMプラグイン試験 (dev)」→ パスからインポート に dist/ の .rasqpkg を入れるか、rasq publish でストアへ");
    }
    Ok(built)
}

#[allow(clippy::too_many_arguments)]
fn publish_cmd(central: String, channel: String, notes_file: Option<PathBuf>, dry_run: bool, skip_listing: bool, make_current: bool, no_build: bool, token_env: &str, with_source: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    if !central.starts_with("http://") && !central.starts_with("https://") {
        bail!("--central は http(s):// で始まる URL です: {central}");
    }
    let notes = match notes_file {
        Some(f) => std::fs::read_to_string(&f).with_context(|| format!("{} を読めません", f.display()))?,
        None => String::new(),
    };
    let packages = if no_build {
        // dist/ にあるものを plan の名前で拾う(build 済み前提)
        let dist = root.join("dist");
        let mut v = Vec::new();
        for p in pkg::plan(&root, &ws, "release")? {
            let bytes = std::fs::read(dist.join(&p.file_name)).with_context(|| format!("dist/{} がありません(rasq build を先に)", p.file_name))?;
            let ins = pkg::inspect(&bytes)?;
            v.push((p.file_name, ins.bundle));
        }
        v
    } else {
        build_in(&root, &ws, false, false, false, with_source)?
    };
    let token = if dry_run {
        None
    } else {
        match std::env::var(token_env) {
            Ok(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => credentials::load()?.central.get(&publish::host_key(&central)).map(|c| c.token.clone()),
        }
    };
    if !dry_run && token.is_none() {
        bail!("Central のトークンがありません: 環境変数 {token_env} か、rasq token set --central {}", publish::host_key(&central));
    }
    let opts = publish::Options { central: central.clone(), channel, notes, dry_run, skip_listing, make_current, token, with_source };
    let done = publish::run(&root, &ws, &packages, &opts)?;
    if dry_run {
        println!("dry-run: {} 件の配布物の要求を {} に書きました(Authorization は $RASQ_CENTRAL_TOKEN のまま)", done.len(), root.join("dist/publish").display());
    } else {
        for d in &done {
            println!("公開しました: {}/api/v1/plugins/{}(版 {}。掲載の status は in_review のまま = 審査待ち)", central.trim_end_matches('/'), d.id, d.version);
        }
    }
    Ok(())
}

fn key_cmd(cmd: KeyCmd) -> Result<()> {
    match cmd {
        KeyCmd::Init { force } => {
            let k = key::init(force)?;
            println!("作りました: {}\n  public = {}\n  指紋 = {}", key::path()?.display(), k.public, key::fingerprint(&k.public));
            println!("この公開鍵をホストに伝えると、サイドロード時に「開発者鍵」として検証される(WF P-23)。秘密は共有しない");
        }
        KeyCmd::Show => match key::load()? {
            Some((k, _)) => println!("{}\n  alg = {}\n  public = {}\n  指紋 = {}\n  created = {}", key::path()?.display(), k.alg, k.public, key::fingerprint(&k.public), k.created_at),
            None => bail!("鍵がありません(rasq key init)"),
        },
    }
    Ok(())
}

fn inspect(path: &Path, extract: Option<&Path>) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("{} を読めません", path.display()))?;
    let ins = pkg::inspect(&bytes)?;
    let b = &ins.bundle;
    println!("{}  {} bytes  sha512:{}", path.display(), bytes.len(), hex(&Sha512::digest(&bytes)));
    println!("bundle: {} sdk {} createdAt {}", b["schema"].as_str().unwrap_or("?"), b["sdk"].as_str().unwrap_or("?"), b["createdAt"].as_str().unwrap_or("?"));
    println!("root:   {} {} ({})", b["root"]["id"].as_str().unwrap_or(""), b["root"]["version"].as_str().unwrap_or(""), b["root"]["kind"].as_str().unwrap_or(""));
    if let Some(r) = b["root"]["requires"].as_array() {
        for x in r {
            println!("  requires {} {}", x["id"].as_str().unwrap_or(""), x["version"].as_str().unwrap_or(""));
        }
    }
    for c in b["children"].as_array().into_iter().flatten() {
        println!("child:  {} {} ({}) slot {}", c["id"].as_str().unwrap_or(""), c["version"].as_str().unwrap_or(""), c["kind"].as_str().unwrap_or(""), c["template"].as_str().unwrap_or("-"));
    }
    for l in b["libs"].as_array().into_iter().flatten() {
        println!("lib:    {} exports {:?}", l["name"].as_str().unwrap_or(""), l["exports"]);
    }
    for a in b["assets"].as_array().into_iter().flatten() {
        println!("asset:  {} ({})", a["path"].as_str().unwrap_or(""), a["kind"].as_str().unwrap_or(""));
    }
    for m in b["models"].as_array().into_iter().flatten() {
        println!("model:  {} lazy={}", m["manifest"].as_str().unwrap_or(""), m["lazy"]);
    }
    println!("files:");
    for e in &ins.entries {
        println!("  {:>9}  {}", e.bytes.len(), e.path);
    }
    let sp = key::sig_path(path);
    // 署名が合わないのは「中身が差し替わった」か「別の鍵」。表示だけして 0 で終わると、スクリプトや CI から見て通ってしまう。
    let mut sig_error = None;
    if sp.is_file() {
        let sig: Value = serde_json::from_slice(&std::fs::read(&sp)?)?;
        let digest: [u8; 64] = Sha512::digest(&bytes).into();
        match key::verify(&sig, &digest) {
            Ok(pk) => {
                // 公開鍵は .sig.json 自身に書いてある = 誰でも自分の鍵で署名し直せる。合っていることは「この鍵で署名された」までしか言わない。
                println!("署名: ok(ただし鍵は .sig.json の自己申告)");
                println!("  自己申告の開発者鍵: 指紋 {}  public {pk}", key::fingerprint(&pk));
                println!("  この指紋を開発者が登録している鍵(Central の掲載 / 開発者本人の告知)と見比べてください。違えば別人の署名です");
            }
            Err(e) => {
                println!("署名: NG — {e}");
                sig_error = Some(e);
            }
        }
    } else {
        println!("署名: なし({} が無い)", sp.file_name().unwrap().to_string_lossy());
    }
    if let Some(dir) = extract {
        for e in &ins.entries {
            let out = dir.join(&e.path);
            std::fs::create_dir_all(out.parent().unwrap())?;
            std::fs::write(&out, &e.bytes)?;
        }
        println!("展開: {}", dir.display());
    }
    if ins.problems.is_empty() {
        println!("目次と中身: ok");
    } else {
        for p in &ins.problems {
            println!("  ERROR: {p}");
        }
    }
    match (ins.problems.len(), sig_error) {
        (0, None) => Ok(()),
        (0, Some(e)) => bail!("署名の検証に失敗しました: {e}"),
        (n, None) => bail!("{n} 件の問題"),
        (n, Some(e)) => bail!("{n} 件の問題、署名の検証にも失敗しました: {e}"),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------
// token(docs/cli.md D-244e)
// ---------------------------------------------------------------------------

fn token(cmd: TokenCmd) -> Result<()> {
    match cmd {
        TokenCmd::Set { account_key, central: true, from_env, .. } => {
            let host = publish::host_key(&account_key);
            if host.is_empty() || host.contains(char::is_whitespace) {
                bail!("--central には host[:port] を渡します(例 100.91.218.56:8080)");
            }
            let tok = credentials::read_token(from_env.as_deref(), &format!("Central {host}(管理トークン)"))?;
            let mut c = credentials::load()?;
            c.central.insert(host.clone(), credentials::Central { token: tok.clone(), updated_at: credentials::now() });
            credentials::save(&c)?;
            println!("保存しました: central {host} → {}(token = {})", credentials::path()?.display(), credentials::mask(&tok));
        }
        TokenCmd::Set { account_key, central: false, from_env, host, no_workspace } => {
            let (kind, instance, handle) = credentials::parse_account_key(&account_key)?;
            let host = host.unwrap_or_else(|| instance.to_string());
            let tok = credentials::read_token(from_env.as_deref(), &format!("{handle}@{instance} ({kind})"))?;
            let mut c = credentials::load()?;
            c.accounts.insert(account_key.clone(), credentials::Account { host: host.clone(), token: tok.clone(), updated_at: credentials::now() });
            credentials::save(&c)?;
            println!("保存しました: {account_key} → {}(host = {host}, token = {})", credentials::path()?.display(), credentials::mask(&tok));
            if !no_workspace {
                if let Ok((root, mut ws)) = Workspace::find() {
                    if ws.set_account(&account_key) {
                        ws.save(&root)?;
                        println!("rasq.toml の account に {account_key} を書きました(トークンは書きません)");
                    } else {
                        println!("rasq.toml に [base] も [sns] も無いので account は書きません(スロットプラグイン単体)");
                    }
                }
            }
        }
        TokenCmd::List => {
            let c = credentials::load()?;
            println!("{}:", credentials::path()?.display());
            if c.accounts.is_empty() && c.central.is_empty() {
                println!("  (なし)");
            }
            for (k, a) in &c.accounts {
                println!("  {k:<40} host = {:<24} token = {:<14} {}", a.host, credentials::mask(&a.token), a.updated_at);
            }
            for (k, a) in &c.central {
                println!("  central {k:<32} token = {:<14} {}", credentials::mask(&a.token), a.updated_at);
            }
        }
        TokenCmd::Remove { account_key } => {
            let mut c = credentials::load()?;
            if c.accounts.remove(&account_key).is_none() && c.central.remove(&publish::host_key(&account_key)).is_none() {
                bail!("{account_key} は保存されていません");
            }
            credentials::save(&c)?;
            println!("消しました: {account_key}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dev(docs/cli.md D-244g)
// ---------------------------------------------------------------------------

fn dev(watch: bool, debug: bool) -> Result<()> {
    let (root, ws) = Workspace::find()?;
    if ws.targets(&root).is_empty() {
        bail!("ビルド対象がありません(rasq new <slot> <name>)");
    }
    let profile = if debug { "debug" } else { "release" };
    dev_once(&root, &ws, profile)?;
    if !watch {
        return Ok(());
    }
    println!("--watch: 変更を待っています(Ctrl-C で止める)");
    let mut last = snapshot_mtimes(&root, &ws);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let now = snapshot_mtimes(&root, &ws);
        if now != last {
            println!("\n変更を検知 → 再ビルド");
            // 保存直後のもう 1 回の変更を拾うため少し待つ
            std::thread::sleep(std::time::Duration::from_millis(300));
            if let Err(e) = dev_once(&root, &ws, profile) {
                eprintln!("error: {e:#}");
            }
            last = snapshot_mtimes(&root, &ws);
        }
    }
}

fn dev_once(root: &Path, ws: &Workspace, profile: &str) -> Result<()> {
    build_wasm(root, profile)?;
    let v = verify(root, ws, profile)?;
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());

    // データを流すアカウント(トークンは credentials.toml にあり、ここには参照だけ)
    let account = ws.account().map(String::from);
    let mut host = None;
    let mut token_ref = None;
    if let Some(k) = &account {
        match credentials::load()?.accounts.get(k) {
            Some(a) => {
                host = Some(a.host.clone());
                token_ref = Some(format!("credentials:{k}"));
            }
            None => println!("warn: rasq.toml の account {k} のトークンが無い(rasq token set {k})"),
        }
    }
    let mut plugins = Vec::new();
    let mut sns = None;
    for t in ws.targets(root) {
        let wasm = abs(&t.wasm_path(root, profile));
        match &t.slot {
            Some(slot) => plugins.push(serde_json::json!({"id": t.id, "slot": slot, "wasm": wasm})),
            None => sns = Some(serde_json::json!({"id": t.id, "wasm": wasm})),
        }
    }
    let base = ws.base.as_ref().map(|b| {
        let wasm = std::fs::read_dir(root.join("base")).ok().and_then(|d| d.filter_map(|e| e.ok().map(|e| e.path())).find(|p| p.extension().is_some_and(|x| x == "wasm")));
        serde_json::json!({"id": b.id, "version": b.version, "wasm": wasm.map(|p| abs(&p))})
    });
    let dev_json = serde_json::json!({
        "schema": "rasq.dev/1",
        "builtAt": credentials::now(),
        "profile": profile,
        "account": account,
        "host": host,
        "tokenRef": token_ref,
        "base": base,
        "sns": sns,
        "plugins": plugins,
    });
    std::fs::create_dir_all(root.join(".rasq"))?;
    std::fs::write(root.join(".rasq/dev.json"), serde_json::to_string_pretty(&dev_json)?)?;

    println!();
    if v.errors > 0 {
        println!("{} errors(上の ERROR を直してから読み込む)", v.errors);
    }
    println!("rasq-client 「WASMプラグイン試験 (dev)」→ .wasm のパスからインポート:");
    if let Some(s) = &dev_json["sns"].as_object() {
        println!("  {:<32} {}", s["id"].as_str().unwrap_or(""), s["wasm"].as_str().unwrap_or(""));
    }
    for p in dev_json["plugins"].as_array().into_iter().flatten() {
        println!("  {:<32} {}", p["id"].as_str().unwrap_or(""), p["wasm"].as_str().unwrap_or(""));
    }
    match (&account, &host) {
        (Some(k), Some(h)) => println!("データ: {k}(host = {h}、トークンは credentials.toml。第一期の dev 画面はホストの既存アカウントで動くため、この参照は .rasq/dev.json に書くだけ)"),
        (None, _) => println!("データ: アカウント未設定(rasq token set <kind:instance:handle> で土台に流すアカウントを決める)"),
        _ => {}
    }
    println!("→ {}", root.join(".rasq/dev.json").display());
    Ok(())
}

/// 監視対象(sns/ と plugins/*/ の src と manifest.json / Cargo.toml、kit/、rasq.toml)の mtime。
fn snapshot_mtimes(root: &Path, ws: &Workspace) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = ws.targets(root).into_iter().map(|t| t.dir).collect();
    dirs.push(root.join("kit"));
    for d in dirs {
        walk(&d, &mut out);
    }
    if let Ok(m) = std::fs::metadata(root.join(workspace::FILE)).and_then(|m| m.modified()) {
        out.push((root.join(workspace::FILE), m));
    }
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
            out.push((p, m));
        }
    }
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
