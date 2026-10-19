//! 利用者に見せる文の表(i18n、PROTOCOL.md §4.6)。
//!
//! ホストは UI の言語を BCP 47 のタグ(`"ja"` / `"en"` / `"zh-Hant"` …)で **すべての call の `args.locale`** に載せる
//! (`args.now` と同じ予約キー。無い・空なら `"ja"`)。プラグインは文を Rust のリテラルで書かず、キーで表から引く:
//!
//! ```
//! use rasq_plugin_kit::i18n;
//!
//! rasq_plugin_kit::i18n_catalog! {
//!     pub static TEXT = {
//!         "ja": {
//!             "greet.button" => "挨拶する",
//!             "greet.count" => "こんにちは!({n} 回目)",
//!         },
//!         // 言語を足すときは表を足すだけ(無いキーは ja に落ちる):
//!         // "en": { "greet.button" => "Say hello" },
//!     };
//! }
//!
//! let args = serde_json::json!({ "locale": "en-US" });
//! let loc = i18n::locale_of(&args);
//! assert_eq!(TEXT.t(loc, "greet.button"), "挨拶する"); // en の表が無いので ja
//! assert_eq!(TEXT.f(loc, "greet.count", &[("n", "3")]), "こんにちは!(3 回目)");
//! ```
//!
//! - 引き方: `zh-Hant-TW` → `zh-Hant` → `zh` → 基準の言語(`ja`)。大文字小文字と `_` / `-` は区別しない。
//!   その言語の表にキーが無ければ次へ落ちるので、訳しかけの表を置いてもよい。基準の表にも無いキーはキーそのものを返す
//!   (native のテストでは [`take_misses`] で拾える)。
//! - `args` の無い `snapshot()` などのために、[`rasq_plugin!`](crate::rasq_plugin) は call のたびに [`set_current`] で
//!   言語を覚える。[`current`] はいちばん新しい call の言語(まだ call が無ければ `"ja"`)。
//! - 言語を足したら、テストで [`Catalog::check`] が空であることを確かめる(基準に無いキー・差し込み `{name}` の食い違い)。

use serde_json::Value;
use std::cell::RefCell;

/// `args.locale` が無い・空のときの言語。基準の表の言語でもある。
pub const DEFAULT_LOCALE: &str = "ja";

/// ホストが言語を載せる予約キー(`args.locale`)。
pub const ARG_KEY: &str = "locale";

/// 1 言語の表: `(キー, 文)`。
pub type Table = &'static [(&'static str, &'static str)];

/// 文の表の束。基準の言語(ふつう `ja`)の表と、ほかの言語の表。[`i18n_catalog!`](crate::i18n_catalog) で作る。
pub struct Catalog {
    base_locale: &'static str,
    base: Table,
    locales: &'static [(&'static str, Table)],
}

impl Catalog {
    pub const fn new(base_locale: &'static str, base: Table, locales: &'static [(&'static str, Table)]) -> Self {
        Catalog { base_locale, base, locales }
    }

    /// 基準の言語のタグ。
    pub fn base_locale(&self) -> &'static str {
        self.base_locale
    }

    /// 表を持っている言語(基準を含む)。
    pub fn locales(&self) -> Vec<&'static str> {
        std::iter::once(self.base_locale).chain(self.locales.iter().map(|(l, _)| *l)).collect()
    }

    /// `locale` の文。無ければ親のタグ → 基準の言語の順に落ちる。基準にも無ければキーそのもの
    /// (キーがリテラルなら戻りも `&'static str`)。
    pub fn t<'a>(&self, locale: &str, key: &'a str) -> &'a str {
        for tag in fallback_chain(locale) {
            if same_tag(&tag, self.base_locale) {
                break;
            }
            if let Some((_, table)) = self.locales.iter().find(|(l, _)| same_tag(l, &tag)) {
                if let Some(s) = lookup(table, key) {
                    return s;
                }
            }
        }
        lookup(self.base, key).unwrap_or_else(|| {
            record_miss(key);
            key
        })
    }

    /// 差し込み付きの文。表の `{name}` を `vars` の値に置き換える(`{{` / `}}` の逃がしは無い)。
    pub fn f(&self, locale: &str, key: &str, vars: &[(&str, &str)]) -> String {
        let mut out = self.t(locale, key).to_string();
        for (name, value) in vars {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }

    /// 表の食い違い(テスト用)。基準の表のキーの重複、ほかの言語の表の「基準に無いキー」「差し込み `{name}` の違い」。
    pub fn check(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (k, _) in self.base {
            if !seen.insert(*k) {
                out.push(format!("{}: キーが重複しています: {k}", self.base_locale));
            }
        }
        for (loc, table) in self.locales {
            if fallback_chain(loc).is_empty() {
                out.push(format!("{loc}: BCP 47 のタグとして読めません"));
            }
            for (k, v) in table.iter() {
                match lookup(self.base, k) {
                    None => out.push(format!("{loc}: 基準({})に無いキー: {k}", self.base_locale)),
                    Some(base) if placeholders(base) != placeholders(v) => {
                        out.push(format!("{loc}: {k} の差し込みが基準と違います: {:?} / {:?}", placeholders(base), placeholders(v)))
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

fn lookup(table: Table, key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn placeholders(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(name, _)| name)).collect();
    v.sort_unstable();
    v
}

fn same_tag(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).all(|(x, y)| x.eq_ignore_ascii_case(&y) || (matches!(x, b'-' | b'_') && matches!(y, b'-' | b'_')))
}

/// 引く順のタグ: `"zh-Hant-TW"` → `["zh-Hant-TW", "zh-Hant", "zh"]`。`_` は `-` に直す。
/// 読めないタグ(空・英数字と `-` 以外を含む)は空。
pub fn fallback_chain(locale: &str) -> Vec<String> {
    let tag = locale.trim().replace('_', "-");
    let parts: Vec<&str> = tag.split('-').collect();
    if tag.is_empty() || parts.iter().any(|p| p.is_empty() || p.len() > 8 || !p.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return Vec::new();
    }
    (1..=parts.len()).rev().map(|n| parts[..n].join("-")).collect()
}

/// `args.locale`(BCP 47)。無い・空・文字列でない・読めないときは [`DEFAULT_LOCALE`]。
pub fn locale_of(args: &Value) -> &str {
    match args.get(ARG_KEY).and_then(Value::as_str) {
        Some(s) if !fallback_chain(s).is_empty() => s.trim(),
        _ => DEFAULT_LOCALE,
    }
}

thread_local! {
    static CURRENT: RefCell<String> = RefCell::new(DEFAULT_LOCALE.to_string());
    static MISSES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// call の `args.locale` を「いまの言語」として覚え、その値を返す(`rasq_plugin!` が call のたびに呼ぶ)。
/// `args.locale` が無ければ `"ja"` に戻す(古いホスト = 日本語)。
pub fn set_current(args: &Value) -> String {
    let loc = locale_of(args).to_string();
    CURRENT.with(|c| c.borrow_mut().clone_from(&loc));
    loc
}

/// いまの言語(いちばん新しい call の `args.locale`。まだ call が無ければ `"ja"`)。`snapshot()` など args の無い所で使う。
pub fn current() -> String {
    CURRENT.with(|c| c.borrow().clone())
}

/// native(テスト)でだけ記録する。wasm では記録しない(取り出す人がいないので溜まるだけ)。
fn record_miss(key: &str) {
    if cfg!(not(target_arch = "wasm32")) {
        MISSES.with(|m| m.borrow_mut().push(key.to_string()));
    }
}

/// テスト用: 基準の表にも無かったキー(native だけ。取り出すと空になる)。
pub fn take_misses() -> Vec<String> {
    MISSES.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

/// 文の表([`Catalog`])を `static` で宣言する。最初の言語が基準(ふつう `"ja"`)で、ほかの言語は無いキーを基準に落とす。
///
/// ```
/// rasq_plugin_kit::i18n_catalog! {
///     pub(crate) static TEXT = {
///         "ja": { "save" => "保存" },
///         "en": { "save" => "Save" },
///     };
/// }
/// assert_eq!(TEXT.t("en-GB", "save"), "Save");
/// assert_eq!(TEXT.t("fr", "save"), "保存");
/// ```
#[macro_export]
macro_rules! i18n_catalog {
    ($vis:vis static $name:ident = { $base:literal : { $($bk:literal => $bv:literal),* $(,)? } $(, $loc:literal : { $($k:literal => $v:literal),* $(,)? })* $(,)? };) => {
        $vis static $name: $crate::i18n::Catalog = $crate::i18n::Catalog::new($base, &[$(($bk, $bv)),*], &[$(($loc, &[$(($k, $v)),*])),*]);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    crate::i18n_catalog! {
        static TEXT = {
            "ja": { "save" => "保存", "count" => "{n} 件", "only.ja" => "日本語だけ" },
            "en": { "save" => "Save", "count" => "{n} items" },
            "zh-Hant": { "save" => "儲存" },
        };
    }

    #[test]
    fn locale_of_falls_back_to_ja() {
        assert_eq!(locale_of(&json!({})), "ja");
        assert_eq!(locale_of(&json!({ "locale": "" })), "ja");
        assert_eq!(locale_of(&json!({ "locale": "  " })), "ja");
        assert_eq!(locale_of(&json!({ "locale": 3 })), "ja");
        assert_eq!(locale_of(&json!({ "locale": "en!" })), "ja");
        assert_eq!(locale_of(&Value::Null), "ja");
        assert_eq!(locale_of(&json!({ "locale": "zh-Hant" })), "zh-Hant");
    }

    #[test]
    fn fallback_chain_drops_subtags() {
        assert_eq!(fallback_chain("zh-Hant-TW"), ["zh-Hant-TW", "zh-Hant", "zh"]);
        assert_eq!(fallback_chain("en_US"), ["en-US", "en"]);
        assert_eq!(fallback_chain("ja"), ["ja"]);
        assert!(fallback_chain("").is_empty());
        assert!(fallback_chain("en--US").is_empty());
    }

    #[test]
    fn lookup_falls_back_to_parent_then_base() {
        assert_eq!(TEXT.t("ja", "save"), "保存");
        assert_eq!(TEXT.t("en", "save"), "Save");
        assert_eq!(TEXT.t("EN-us", "save"), "Save");
        assert_eq!(TEXT.t("zh-Hant-TW", "save"), "儲存");
        assert_eq!(TEXT.t("zh_hant", "save"), "儲存");
        assert_eq!(TEXT.t("zh-Hans", "save"), "保存"); // zh の表は無い → ja
        assert_eq!(TEXT.t("fr", "save"), "保存");
        assert_eq!(TEXT.t("ja-JP", "save"), "保存");
        assert_eq!(TEXT.t("en", "only.ja"), "日本語だけ"); // 訳しかけの表
        assert_eq!(TEXT.t("", "save"), "保存");
        assert!(take_misses().is_empty());
    }

    #[test]
    fn unknown_keys_are_recorded() {
        let _ = take_misses();
        assert_eq!(TEXT.t("ja", "nope"), "nope");
        assert_eq!(TEXT.t("en", "nope2"), "nope2");
        assert_eq!(take_misses(), ["nope", "nope2"]);
    }

    #[test]
    fn format_substitutes_placeholders() {
        assert_eq!(TEXT.f("ja", "count", &[("n", "3")]), "3 件");
        assert_eq!(TEXT.f("en", "count", &[("n", "3")]), "3 items");
    }

    #[test]
    fn check_reports_bad_tables() {
        assert!(TEXT.check().is_empty(), "{:?}", TEXT.check());
        crate::i18n_catalog! {
            static BAD = {
                "ja": { "a" => "{n} 件", "a" => "重複" },
                "en": { "a" => "{m} items", "b" => "extra" },
                "e n": { "a" => "x" },
            };
        }
        let problems = BAD.check();
        assert_eq!(problems.len(), 5, "{problems:?}"); // 重複 / 差し込み / 基準に無い b / 読めないタグ / e n の差し込み
        assert_eq!(BAD.locales(), ["ja", "en", "e n"]);
    }

    #[test]
    fn current_follows_the_latest_call() {
        assert_eq!(set_current(&json!({ "locale": "en" })), "en");
        assert_eq!(current(), "en");
        assert_eq!(set_current(&json!({})), "ja");
        assert_eq!(current(), "ja");
    }
}
