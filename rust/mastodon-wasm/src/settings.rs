//! 設定ページ (D-044 / rasq-client D-482): `sns.settingsPages` / `sns.settingsRead` / `sns.settingsWrite`。
//!
//! ホストの「SNSプラグイン」の行(アカウント / タイムライン / セキュリティ / プライバシーとセーフモード)は、
//! プラグインが `sns.settingsPages` で返した `rasq.settings-page/1` のページを開く。返さないページの行は
//! ホストが出さない。
//!
//! - `account`  … source: remote — `GET /api/v1/accounts/verify_credentials` /
//!   `PATCH /api/v1/accounts/update_credentials`(表示名・自己紹介、公開範囲、投稿の既定、Bot)
//! - `timeline` … ローカル保存 — 値は毎呼び出しの `args.settings.timeline` に載る。Mastodon の TL API には
//!   ブースト / 返信を外す引数が無いので、取ってきたページから外す([`keep_in_timeline`])
//! - `safety`   … ローカル保存 — ホストが投稿タイルの閲覧注意メディア / CW の表示に使う
//! - `security` … 表示だけ — パスワードが要る操作は web 版へ(`on: {"tap": "web.open"}`)

use rasq_plugin_kit::i18n;
use serde_json::{json, Value};

use crate::text::TEXT;
use crate::{fetch, get, html_to_text};

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

/// 文字の設定(表示名・自己紹介)。「保存」で `settings.set`(payload.text)。
fn input(loc: &str, k: &str, label: &str, max_length: u32, multiline: bool) -> Value {
    json!({ "t": "Input", "k": k, "p": { "label": label, "value": format!("$.settings.{k}"), "maxLength": max_length, "multiline": multiline, "submitLabel": TEXT.t(loc, "settings.save"), "clearOnSubmit": false, "allowEmpty": true, "pad": 16 }, "on": { "submit": "settings.set" } })
}

fn web_link(path: &str, title: &str, subtitle: Option<&str>) -> Value {
    json!({ "t": "ListTile", "k": path, "p": { "title": title, "subtitle": subtitle }, "on": { "tap": "web.open" } })
}

fn note(text: &str) -> Value {
    json!({ "t": "Text", "p": { "text": text, "pad": 16 } })
}

fn page(area: &str, title: &str, source: &str, defaults: Value, children: Vec<Value>) -> Value {
    json!({ "schema": "rasq.settings-page/1", "area": area, "title": title, "source": source, "defaults": defaults, "root": { "t": "Column", "c": children } })
}

// ── ページ ────────────────────────────────────────────────────────
// 文は crate::text::TEXT の表から `args.locale` で引く(PROTOCOL.md §4.6)。

/// 投稿の公開範囲の値と、表示名 / 補足のキー。
const PRIVACIES: [(&str, &str, Option<&str>); 3] = [
    ("public", "account.privacy.public", Some("account.privacy.public.sub")),
    ("unlisted", "account.privacy.unlisted", Some("account.privacy.unlisted.sub")),
    ("private", "account.privacy.private", None),
];

fn account_defaults() -> Value {
    json!({
        "displayName": "", "note": "",
        "locked": false, "discoverable": false, "indexable": false, "hideCollections": false,
        "defaultPrivacy": "public", "defaultSensitive": false, "bot": false,
    })
}

fn account_page(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    let mut c = vec![
        header(t("account.profile")),
        input(loc, "displayName", t("account.displayName"), 30, false),
        input(loc, "note", t("account.note"), 500, true),
        web_link("settings/profile", t("account.profileOnWeb"), Some(t("settings.webOnly"))),
        header(t("account.visibility")),
        switch("locked", t("account.locked"), Some(t("account.locked.sub"))),
        switch("discoverable", t("account.discoverable"), Some(t("account.discoverable.sub"))),
        switch("indexable", t("account.indexable"), None),
        switch("hideCollections", t("account.hideCollections"), None),
        header(t("account.defaultPrivacy")),
    ];
    for (v, label, sub) in PRIVACIES {
        c.push(radio("defaultPrivacy", v, t(label), sub.map(t)));
    }
    c.extend([
        switch("defaultSensitive", t("account.defaultSensitive"), Some(t("account.defaultSensitive.sub"))),
        header(t("account.kind")),
        switch("bot", t("account.bot"), Some(t("account.bot.sub"))),
        note(t("account.savedOnServer")),
    ]);
    page("account", t("account.title"), "remote", account_defaults(), c)
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

/// 閲覧注意メディア / CW。ホストが投稿タイルに当てる(どの SNS プラグインも同じキー)。
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
            header(t("security.scopes")),
            json!({ "t": "ValueListTile", "p": { "title": t("security.scopes.value"), "value": "read write follow" } }),
            header(t("security.onWeb")),
            web_link("auth/edit", t("security.password"), Some(t("security.password.sub"))),
            web_link("settings/two_factor_authentication_methods", t("security.twoFactor"), None),
            web_link("oauth/authorized_applications", t("security.apps"), Some(t("security.apps.sub"))),
            note(t("security.tokenNote")),
        ],
    )
}

/// `call("sns.settingsPages")` → `{pages}`。文は `args.locale`(無ければ ja)で引く。
pub(crate) fn sns_settings_pages(args: &Value) -> Result<Value, Value> {
    let loc = i18n::locale_of(args);
    Ok(json!({ "pages": [account_page(loc), timeline_page(loc), safety_page(loc), security_page(loc)] }))
}

// ── timeline の当て方 ────────────────────────────────────────────

/// TL の 1 件を残すか(`args.settings.timeline` の withReposts / withReplies)。
/// 自分の投稿への返信(スレッドのつながり)は返信を外しても残す。
pub(crate) fn keep_in_timeline(status: &Value, tl_settings: &Value) -> bool {
    if tl_settings["withReposts"].as_bool() == Some(false) && !status["reblog"].is_null() {
        return false;
    }
    if tl_settings["withReplies"].as_bool() == Some(false) && !status["in_reply_to_id"].is_null() {
        let self_reply = status["in_reply_to_account_id"].as_str().is_some() && status["in_reply_to_account_id"] == status["account"]["id"];
        if !self_reply {
            return false;
        }
    }
    true
}

// ── account の読み書き ────────────────────────────────────────────

fn account_values(a: &Value) -> Value {
    let note = match a["source"]["note"].as_str() {
        Some(n) => n.to_string(),
        None => html_to_text(a["note"].as_str().unwrap_or("")),
    };
    json!({
        "displayName": a["display_name"].as_str().unwrap_or(""),
        "note": note,
        "locked": a["locked"].as_bool().unwrap_or(false),
        "discoverable": a["discoverable"].as_bool().unwrap_or(false),
        "indexable": a["indexable"].as_bool().unwrap_or(false),
        "hideCollections": a["hide_collections"].as_bool().unwrap_or(false),
        "defaultPrivacy": a["source"]["privacy"].as_str().unwrap_or("public"),
        "defaultSensitive": a["source"]["sensitive"].as_bool().unwrap_or(false),
        "bot": a["bot"].as_bool().unwrap_or(false),
    })
}

/// 1 キーを `update_credentials` の本文へ。許可リストに無いキー / 形の違う値は断る。
fn account_update_body(key: &str, value: &Value, loc: &str) -> Result<Value, Value> {
    let bad = |k: &str| Err(json!({ "code": "D01", "message": TEXT.t(loc, k) }));
    let flag = |field: &str| match value.as_bool() {
        Some(b) => Ok(json!({ field: b })),
        None => bad("error.notBool"),
    };
    match key {
        "displayName" => Ok(json!({ "display_name": value.as_str().unwrap_or("").trim() })),
        "note" => Ok(json!({ "note": value.as_str().unwrap_or("").trim() })),
        "locked" => flag("locked"),
        "discoverable" => flag("discoverable"),
        "indexable" => flag("indexable"),
        "hideCollections" => flag("hide_collections"),
        "bot" => flag("bot"),
        "defaultSensitive" => match value.as_bool() {
            Some(b) => Ok(json!({ "source": { "sensitive": b } })),
            None => bad("error.notBool"),
        },
        "defaultPrivacy" => match value.as_str() {
            Some(s) if PRIVACIES.iter().any(|(v, _, _)| *v == s) => Ok(json!({ "source": { "privacy": s } })),
            _ => bad("error.privacy"),
        },
        other => Err(json!({ "code": "C01", "message": format!("unknown account setting: {other}") })),
    }
}

/// `call("sns.settingsRead", {area})`: remote ページ(`account`)だけ。
pub(crate) fn sns_settings_read(args: &Value) -> Result<Value, Value> {
    if args["area"].as_str() != Some("account") {
        return Err(json!({ "code": "C01", "message": "no such remote settings area" }));
    }
    Ok(json!({ "values": account_values(&get(args, "/api/v1/accounts/verify_credentials")?) }))
}

/// `call("sns.settingsWrite", {area, key, value})` → 書いたあとの `{values}`。
/// `update_credentials` は更新後の CredentialAccount(`source` つき)を返すので、それをそのまま写す。
pub(crate) fn sns_settings_write(args: &Value) -> Result<Value, Value> {
    if args["area"].as_str() != Some("account") {
        return Err(json!({ "code": "C01", "message": "no such remote settings area" }));
    }
    let Some(key) = args["key"].as_str() else {
        return Err(json!({ "code": "D01", "message": "key required" }));
    };
    let body = account_update_body(key, &args["value"], i18n::locale_of(args))?;
    let account = fetch(args, "PATCH", "/api/v1/accounts/update_credentials", Some(body))?;
    Ok(json!({ "values": account_values(&account) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{set_host_hook, take_host_log};

    /// `net.fetch` をまねる。`handler(method, path, body)` の戻りが 200 の本文。
    fn mock_api<F: FnMut(&str, &str, &Value) -> Value + 'static>(mut handler: F) {
        set_host_hook(move |fn_name, a| match fn_name {
            "net.fetch" => {
                let url = a["url"].as_str().unwrap_or("");
                let path = url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap_or_default();
                let body: Value = serde_json::from_str(a["body"].as_str().unwrap_or("null")).unwrap_or(Value::Null);
                let method = a["method"].as_str().unwrap_or("").to_string();
                json!({ "status": 200, "headers": {}, "bodyText": handler(&method, &path, &body).to_string() })
            }
            _ => json!({}),
        });
        let _ = take_host_log();
    }

    fn args(extra: Value) -> Value {
        let mut a = json!({ "host": "mastodon.example", "token": "tok" });
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        a
    }

    #[test]
    fn declares_the_four_pages_and_every_tile_key_has_a_default() {
        let pages = sns_settings_pages(&json!({})).unwrap()["pages"].as_array().unwrap().clone();
        let areas: Vec<&str> = pages.iter().map(|p| p["area"].as_str().unwrap()).collect();
        assert_eq!(areas, ["account", "timeline", "safety", "security"]);
        for p in &pages {
            for node in p["root"]["c"].as_array().unwrap() {
                if node["on"]["change"] == "settings.set" || node["on"]["submit"] == "settings.set" {
                    let k = node["k"].as_str().unwrap();
                    assert!(p["defaults"].get(k).is_some(), "{}: {k} has no default", p["area"]);
                    if p["area"] == "account" {
                        // 画面の全キーが書ける(許可リストと揃っている)。
                        assert_ne!(account_update_body(k, &p["defaults"][k], "ja").map_err(|e| e["code"].clone()), Err(json!("C01")), "{k}");
                    }
                }
            }
        }
    }

    #[test]
    fn account_read_uses_the_plain_source_note() {
        mock_api(|m, path, _| {
            assert_eq!((m, path), ("GET", "/api/v1/accounts/verify_credentials"));
            json!({ "display_name": "ぜぽす", "note": "<p>HTML の自己紹介</p>", "locked": true, "bot": false, "hide_collections": true,
                    "source": { "note": "平文の自己紹介", "privacy": "unlisted", "sensitive": true } })
        });
        let v = sns_settings_read(&args(json!({ "area": "account" }))).unwrap()["values"].clone();
        assert_eq!(v["displayName"], "ぜぽす");
        assert_eq!(v["note"], "平文の自己紹介");
        assert_eq!(v["locked"], true);
        assert_eq!(v["hideCollections"], true);
        assert_eq!(v["defaultPrivacy"], "unlisted");
        assert_eq!(v["defaultSensitive"], true);
        assert_eq!(v["discoverable"], false); // null は false
        let (_, req) = take_host_log().into_iter().find(|(f, _)| f == "net.fetch").unwrap();
        assert_eq!(req["headers"]["Authorization"], "Bearer tok");
    }

    #[test]
    fn account_write_patches_update_credentials_with_nested_source() {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<(String, String, Value)>::new()));
        let log = seen.clone();
        mock_api(move |m, path, body| {
            log.borrow_mut().push((m.to_string(), path.to_string(), body.clone()));
            json!({ "display_name": "x", "source": { "privacy": body["source"]["privacy"].clone() } })
        });
        let v = sns_settings_write(&args(json!({ "area": "account", "key": "defaultPrivacy", "value": "private" }))).unwrap()["values"].clone();
        assert_eq!(v["defaultPrivacy"], "private");
        let (m, path, body) = seen.borrow()[0].clone();
        assert_eq!((m.as_str(), path.as_str()), ("PATCH", "/api/v1/accounts/update_credentials"));
        assert_eq!(body, json!({ "source": { "privacy": "private" } }));

        assert_eq!(account_update_body("hideCollections", &json!(true), "ja").unwrap(), json!({ "hide_collections": true }));
        assert_eq!(account_update_body("displayName", &json!(" 名前 "), "ja").unwrap(), json!({ "display_name": "名前" }));
    }

    #[test]
    fn account_write_rejects_unknown_keys_and_bad_values() {
        mock_api(|_, _, _| panic!("must not call the server"));
        let w = |key: &str, value: Value| sns_settings_write(&args(json!({ "area": "account", "key": key, "value": value }))).unwrap_err()["code"].clone();
        assert_eq!(w("email", json!("a@b")), "C01");
        assert_eq!(w("locked", json!("yes")), "D01");
        assert_eq!(w("defaultPrivacy", json!("direct")), "D01");
        assert_eq!(sns_settings_read(&args(json!({ "area": "security" }))).unwrap_err()["code"], "C01");
    }

    #[test]
    fn timeline_settings_drop_boosts_and_replies_but_keep_self_threads() {
        let plain = json!({ "id": "1", "account": { "id": "a" }, "reblog": null, "in_reply_to_id": null });
        let boost = json!({ "id": "2", "account": { "id": "a" }, "reblog": { "id": "9" }, "in_reply_to_id": null });
        let reply = json!({ "id": "3", "account": { "id": "a" }, "reblog": null, "in_reply_to_id": "7", "in_reply_to_account_id": "b" });
        let thread = json!({ "id": "4", "account": { "id": "a" }, "reblog": null, "in_reply_to_id": "8", "in_reply_to_account_id": "a" });
        let off = json!({ "withReposts": false, "withReplies": false });
        let kept: Vec<&str> = [&plain, &boost, &reply, &thread].iter().filter(|s| keep_in_timeline(s, &off)).map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(kept, ["1", "4"]);
        // 設定が無ければ全部残す。
        assert!([&plain, &boost, &reply, &thread].iter().all(|s| keep_in_timeline(s, &Value::Null)));
    }
}
