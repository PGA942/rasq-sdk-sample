//! ワークスペース(`rasq.toml`、docs/cli.md §2)。root の `Cargo.toml` は CLI が rasq.toml から作り直す。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE: &str = "rasq.toml";
pub const SCHEMA: &str = "rasq.workspace/1";

#[derive(Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub schema: String,
    pub sdk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>,
    /// 土台の SNS プラグイン(無ければ `sns` = 自分が SNS プラグイン、またはスロットプラグイン単体)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Base>,
    /// 自分が作る SNS プラグイン(`rasq init --sns`)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sns: Option<Sns>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginEntry>,
    /// アセットパック(`packs/<name>/`、wasm 無し。docs/slots.md §2.10)。パックごとに配布物 1 つ。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<PackEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libs: Vec<Lib>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Asset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Model>,
    /// ストアの掲載(Central の UpsertPlugin)。無ければ manifest の name / description / supportedSns から。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<Listing>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Listing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub has_ads: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Author {
    /// 子プラグインの id の接頭辞(土台が他人のものか無いとき)。
    pub namespace: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Base {
    pub id: String,
    pub version: String,
    pub source: String,
    /// true なら子を同梱して 1 つの .rasqpkg にする(D-244c)。
    #[serde(default)]
    pub owned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sns {
    pub id: String,
    pub dir: String,
    /// `sns.login` の確認に使うアカウント(トークンは credentials.toml)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginEntry {
    pub name: String,
    pub slot: String,
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackEntry {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Lib {
    pub name: String,
    pub path: String,
    pub exports: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Model {
    pub manifest: String,
    #[serde(default)]
    pub lazy: bool,
}

/// ビルド対象 1 つ(sns か plugins/<name>)。
pub struct Target {
    pub dir: PathBuf,
    /// Cargo のパッケージ名(= ディレクトリ名)。
    pub package: String,
    pub id: String,
    /// 宣言したスロット(sns は None)。
    pub slot: Option<String>,
}

impl Target {
    /// `<target>/wasm32-unknown-unknown/<profile>/<crate>.wasm`。`<target>` は cargo と同じ決め方
    /// (`CARGO_TARGET_DIR` があればそれ。相対ならワークスペースからの相対。無ければ `target/`)。
    pub fn wasm_path(&self, root: &Path, profile: &str) -> PathBuf {
        cargo_target_dir(root).join("wasm32-unknown-unknown").join(profile).join(format!("{}.wasm", self.package.replace('-', "_")))
    }
}

/// cargo が使う target ディレクトリ(`CARGO_TARGET_DIR` を尊重する。CI や共有ビルドでリポジトリの外に置くため)。
pub fn cargo_target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        Some(v) => {
            let p = PathBuf::from(v);
            if p.is_absolute() { p } else { root.join(p) }
        }
        None => root.join("target"),
    }
}

impl Workspace {
    pub fn new(sdk: &str, namespace: &str) -> Self {
        Self {
            schema: SCHEMA.into(),
            sdk: sdk.into(),
            author: Some(Author { namespace: namespace.into() }),
            base: None,
            sns: None,
            plugins: vec![],
            packs: vec![],
            libs: vec![],
            assets: vec![],
            models: vec![],
            listing: None,
        }
    }

    /// カレントから上へ `rasq.toml` を探す。
    pub fn find() -> Result<(PathBuf, Self)> {
        let mut dir = std::env::current_dir()?;
        loop {
            let p = dir.join(FILE);
            if p.is_file() {
                return Ok((dir.clone(), Self::load(&dir)?));
            }
            if !dir.pop() {
                bail!("{FILE} が見つかりません(rasq init で作る)");
            }
        }
    }

    pub fn load(root: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(root.join(FILE)).with_context(|| format!("{} を読めません", root.join(FILE).display()))?;
        let ws: Self = toml::from_str(&text).context("rasq.toml の形が違います")?;
        if ws.schema != SCHEMA {
            bail!("rasq.toml の schema が {} ではありません: {}", SCHEMA, ws.schema);
        }
        Ok(ws)
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("rasq.toml を書き出せません")?;
        std::fs::write(root.join(FILE), text)?;
        self.sync_cargo_toml(root)
    }

    /// root の `Cargo.toml`(仮想ワークスペース)を rasq.toml から作り直す。
    pub fn sync_cargo_toml(&self, root: &Path) -> Result<()> {
        let mut members: Vec<String> = self.plugins.iter().map(|p| format!("plugins/{}", p.name)).collect();
        if let Some(s) = &self.sns {
            members.insert(0, s.dir.clone());
        }
        let members = members.iter().map(|m| format!("    \"{m}\",")).collect::<Vec<_>>().join("\n");
        let text = format!(
            "# rasq CLI が rasq.toml から作り直すファイル(手で直しても rasq new で戻る)。\n[workspace]\nresolver = \"2\"\nmembers = [\n{members}\n]\n\n[profile.release]\nopt-level = \"s\"\nlto = true\ncodegen-units = 1\npanic = \"abort\"\nstrip = true\n"
        );
        std::fs::write(root.join("Cargo.toml"), text)?;
        Ok(())
    }

    pub fn namespace(&self) -> &str {
        self.author.as_ref().map(|a| a.namespace.as_str()).unwrap_or("local.dev")
    }

    /// `rasq new` が付ける子の id(docs/cli.md D-244c)。
    pub fn child_id(&self, name: &str) -> String {
        if let Some(s) = &self.sns {
            return format!("{}.{name}", s.id);
        }
        match &self.base {
            Some(b) if b.owned => format!("{}.{name}", b.id),
            _ => format!("{}.{name}", self.namespace()),
        }
    }

    pub fn targets(&self, root: &Path) -> Vec<Target> {
        let mut v = Vec::new();
        if let Some(s) = &self.sns {
            v.push(Target { dir: root.join(&s.dir), package: s.dir.clone(), id: s.id.clone(), slot: None });
        }
        for p in &self.plugins {
            v.push(Target { dir: root.join("plugins").join(&p.name), package: p.name.clone(), id: p.id.clone(), slot: Some(p.slot.clone()) });
        }
        v
    }

    /// データを流すアカウント(`[base].account` か `[sns].account`)。
    pub fn account(&self) -> Option<&str> {
        self.base.as_ref().and_then(|b| b.account.as_deref()).or_else(|| self.sns.as_ref().and_then(|s| s.account.as_deref()))
    }

    /// `rasq token set` が書く先。土台も sns も無ければ None(スロットプラグイン単体)。
    pub fn set_account(&mut self, key: &str) -> bool {
        if let Some(b) = &mut self.base {
            b.account = Some(key.into());
            true
        } else if let Some(s) = &mut self.sns {
            s.account = Some(key.into());
            true
        } else {
            false
        }
    }

    pub fn plugin(&self, name: &str) -> Option<&PluginEntry> {
        self.plugins.iter().find(|p| p.name == name)
    }
}

pub fn valid_name(name: &str) -> Result<()> {
    let ok = name.len() <= 40
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(anyhow!("名前は小文字英数字と '-'(先頭は英字、40 文字まで): {name:?}"))
    }
}

fn id_labels_ok(parts: &[&str]) -> bool {
    !parts.is_empty()
        && !parts[0].is_empty()
        && parts[0].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && parts[1..].iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'))
}

/// pluginId の文法(schema/action-ref.schema.json `pluginId`: `^[a-z0-9]+(\.[a-z0-9_-]+)+$`)。
pub fn valid_plugin_id(id: &str) -> Result<()> {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() >= 2 && id_labels_ok(&parts) {
        Ok(())
    } else {
        Err(anyhow!("pluginId の形ではありません(例 net.example.later): {id:?}"))
    }
}

/// 名前空間の文法: pluginId の先頭部分。1 ラベル(`sample`)でも、2 ラベル以上(`net.example`)でもよい
/// (`<namespace>.<name>` が pluginId になる)。
pub fn valid_namespace(ns: &str) -> Result<()> {
    let parts: Vec<&str> = ns.split('.').collect();
    if id_labels_ok(&parts) {
        Ok(())
    } else {
        Err(anyhow!("名前空間の形ではありません(例 net.example、sample): {ns:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_accepts_one_or_more_labels() {
        assert!(valid_namespace("sample").is_ok());
        assert!(valid_namespace("net.example").is_ok());
        assert!(valid_namespace("Net").is_err());
        assert!(valid_namespace("").is_err());
        assert!(valid_namespace("a..b").is_err());
        assert!(valid_plugin_id("sample").is_err());
        assert!(valid_plugin_id("sample.timeline").is_ok());
    }

    #[test]
    fn target_dir_follows_cargo() {
        let root = Path::new("/ws");
        // 環境変数はテストの並列実行で共有されるので、値を戻す。
        let saved = std::env::var_os("CARGO_TARGET_DIR");
        std::env::remove_var("CARGO_TARGET_DIR");
        assert_eq!(cargo_target_dir(root), PathBuf::from("/ws/target"));
        std::env::set_var("CARGO_TARGET_DIR", "../shared");
        assert_eq!(cargo_target_dir(root), PathBuf::from("/ws/../shared"));
        std::env::set_var("CARGO_TARGET_DIR", "/abs/t");
        assert_eq!(cargo_target_dir(root), PathBuf::from("/abs/t"));
        match saved {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }
    }
}
