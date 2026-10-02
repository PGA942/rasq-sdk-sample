//! Access Token の置き場(docs/cli.md D-244e): `~/.config/rasq/credentials.toml`(0600)。
//! リポジトリには accountKey しか書かない。トークンは argv で受け取らない(シェル履歴に残るため)。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// accountKey(`kind:instance:handle`)→ 資格情報。
    #[serde(default)]
    pub accounts: BTreeMap<String, Account>,
    /// Central のホスト(`host[:port]`)→ トークン(⑤ publish 用)。
    #[serde(default)]
    pub central: BTreeMap<String, Central>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    /// 接続先(accountKey の instance。`--host` で上書き可 = テスト用サーバー)。
    pub host: String,
    pub token: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Central {
    pub token: String,
    pub updated_at: String,
}

pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("RASQ_CREDENTIALS") {
        return Ok(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| anyhow!("HOME が分かりません(RASQ_CREDENTIALS でファイルを指定できます)"))?;
    Ok(base.join("rasq").join("credentials.toml"))
}

pub fn load() -> Result<Credentials> {
    let p = path()?;
    if !p.is_file() {
        return Ok(Credentials::default());
    }
    let text = std::fs::read_to_string(&p).with_context(|| format!("{} を読めません", p.display()))?;
    toml::from_str(&text).with_context(|| format!("{} の形が違います", p.display()))
}

pub fn save(c: &Credentials) -> Result<()> {
    let p = path()?;
    // 0700 のディレクトリに、0600 で作った一時ファイルから原子的に置き換える(書いてから chmod しない、#5)。
    crate::private_file::write(&p, toml::to_string_pretty(c)?.as_bytes())
}

/// accountKey = `kind:instance:handle`(schema/action-ref.schema.json)。戻りは (kind, instance, handle)。
pub fn parse_account_key(key: &str) -> Result<(&str, &str, &str)> {
    let parts: Vec<&str> = key.split(':').collect();
    let ok = parts.len() == 3
        && !parts[0].is_empty()
        && parts[0].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && parts[1..].iter().all(|p| !p.is_empty() && !p.chars().any(char::is_whitespace));
    if !ok {
        bail!("accountKey は kind:instance:handle の形です(例 misskey:misskey.io:yamada): {key:?}");
    }
    Ok((parts[0], parts[1], parts[2]))
}

/// トークンの受け取り: `--from-env VAR` → 環境変数、端末なら非表示入力、それ以外は標準入力の 1 行。
pub fn read_token(from_env: Option<&str>, what: &str) -> Result<String> {
    let token = if let Some(var) = from_env {
        std::env::var(var).with_context(|| format!("環境変数 {var} がありません"))?
    } else if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("{what} のトークン(表示されません): ")).context("入力を読めません")?
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).context("標準入力を読めません")?;
        line
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        bail!("トークンが空です");
    }
    Ok(token)
}

pub fn now() -> String {
    // 依存を増やさない粗い ISO-8601(秒精度、UTC)。
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// 端末に出す用: 先頭 4 文字だけ。
pub fn mask(token: &str) -> String {
    let head: String = token.chars().take(4).collect();
    // nosemgrep: rasq-secret-in-output — 出すのは先頭 4 文字と文字数だけ (これが伏字にする側)
    format!("{head}…({} 文字)", token.chars().count())
}
