//! 設定ページ (D-044 / rasq-client D-482): `sns.settingsPages` / `sns.settingsRead`。
//!
//! ホストの「SNSプラグイン」の行(アカウント / タイムライン / セキュリティ / プライバシーとセーフモード)は、
//! プラグインが `sns.settingsPages` で返した `rasq.settings-page/1` のページを開く。返さないページの行は
//! ホストが出さない。
//!
//! mixi2 の API(申請制、実モードのエンドポイントは仮置き)にはプロフィールや公開範囲を変える口が
//! 見当たらないので、`account` は**読むだけ**(`sns.me` と同じもの)にし、変えるのは mixi2 のアプリと案内する。
//! `timeline` / `safety` はローカル保存で、どの SNS でも同じように効く。

use rasq_plugin_kit::i18n;
use serde_json::{json, Value};

use crate::text::TEXT;

// ── RRL の部品 ────────────────────────────────────────────────────

fn header(title: &str) -> Value {
    json!({ "t": "SectionHeader", "p": { "title": title } })
}

fn switch(k: &str, title: &str, subtitle: Option<&str>) -> Value {
    json!({ "t": "SwitchListTile", "k": k, "p": { "title": title, "subtitle": subtitle, "value": format!("$.settings.{k}") }, "on": { "change": "settings.set" } })
}

fn radio(k: &str, value: &str, title: &str, subtitle: Option<&str>) -> Value {
    json!({ "t": "RadioListTile", "k": k, "p": { "title": title, "subtitle": subtitle, "value": value, "group": format!("$.settings.{k}") }, "on": { "change": "settings.set" } })
}

fn value_tile(title: &str, k: &str) -> Value {
    json!({ "t": "ValueListTile", "p": { "title": title, "value": format!("$.settings.{k}") } })
}

fn notice(text: &str) -> Value {
    json!({ "t": "Notice", "p": { "kind": "info", "text": text } })
}

fn note(text: &str) -> Value {
    json!({ "t": "Text", "p": { "text": text, "pad": 16 } })
}

fn page(area: &str, title: &str, source: &str, defaults: Value, children: Vec<Value>) -> Value {
    json!({ "schema": "rasq.settings-page/1", "area": area, "title": title, "source": source, "defaults": defaults, "root": { "t": "Column", "c": children } })
}

// ── ページ ────────────────────────────────────────────────────────
// 文は crate::text::TEXT の表から `args.locale` で引く(PROTOCOL.md §4.6)。

fn account_page(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    page(
        "account",
        t("account.title"),
        "remote",
        json!({ "displayName": "", "handle": "", "bio": "" }),
        vec![
            header(t("account.profile")),
            value_tile(t("account.displayName"), "displayName"),
            value_tile(t("account.handle"), "handle"),
            header(t("account.bio")),
            json!({ "t": "Text", "p": { "text": "$.settings.bio", "pad": 16 } }),
            notice(t("account.readOnlyNotice")),
        ],
    )
}

fn timeline_page(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    page(
        "timeline",
        t("timeline.title"),
        "local",
        json!({ "withReposts": true, "withReplies": true }),
        vec![
            header(t("timeline.shownPosts")),
            switch("withReposts", t("timeline.withReposts"), Some(t("timeline.withReposts.sub"))),
            switch("withReplies", t("timeline.withReplies"), Some(t("timeline.withReplies.sub"))),
        ],
    )
}

/// 閲覧注意メディア / 注釈。ホストが投稿タイルに当てる(どの SNS プラグインも同じキー)。
fn safety_page(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    page(
        "safety",
        t("safety.title"),
        "local",
        json!({ "sensitiveMedia": "blur", "openCw": false }),
        vec![
            header(t("safety.sensitiveMedia")),
            radio("sensitiveMedia", "blur", t("safety.blur"), Some(t("safety.blur.sub"))),
            radio("sensitiveMedia", "show", t("safety.show"), None),
            radio("sensitiveMedia", "hide", t("safety.hide"), Some(t("safety.hide.sub"))),
            header(t("safety.cw")),
            switch("openCw", t("safety.openCw"), None),
        ],
    )
}

fn security_page(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    page(
        "security",
        t("security.title"),
        "local",
        json!({}),
        vec![
            header(t("security.loginMethod")),
            json!({ "t": "ValueListTile", "p": { "title": t("security.loginWith"), "value": t("security.loginWith.value") } }),
            note(t("security.tokenNote")),
            notice(t("security.elsewhere")),
        ],
    )
}

/// `call("sns.settingsPages")` → `{pages}`。文は `args.locale`(無ければ ja)で引く。
pub(crate) fn sns_settings_pages(args: &Value) -> Value {
    let loc = i18n::locale_of(args);
    json!({ "pages": [account_page(loc), timeline_page(loc), safety_page(loc), security_page(loc)] })
}

/// `account` の値 = 自分のユーザー(`sns.me` のワイヤ)。
pub(crate) fn account_values(me: &Value) -> Value {
    json!({
        "displayName": me["displayName"].as_str().unwrap_or(""),
        "handle": me["handle"].as_str().map(|h| format!("@{h}")).unwrap_or_default(),
        "bio": me["bio"].as_str().unwrap_or(""),
    })
}

/// TL の 1 件(post ワイヤ)を残すか(`args.settings.timeline` の withReposts / withReplies)。
pub(crate) fn keep_in_timeline(wire: &Value, tl_settings: &Value) -> bool {
    if tl_settings["withReposts"].as_bool() == Some(false) && wire["repostedBy"].is_object() {
        return false;
    }
    !(tl_settings["withReplies"].as_bool() == Some(false) && wire["replyToId"].is_string())
}
