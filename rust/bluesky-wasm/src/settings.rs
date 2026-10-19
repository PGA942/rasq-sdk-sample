//! 設定ページ (D-044 / rasq-client D-482): `sns.settingsPages` / `sns.settingsRead` / `sns.settingsWrite`。
//!
//! ホストの「SNSプラグイン」の行(アカウント / タイムライン / セキュリティ / プライバシーとセーフモード)は、
//! プラグインが `sns.settingsPages` で返した `rasq.settings-page/1` のページを開く。返さないページの行は
//! ホストが出さない。
//!
//! - `account`  … source: remote — 自分のリポジトリのレコードを読み書きする:
//!   プロフィール `app.bsky.actor.profile/self`(表示名・自己紹介・ログアウト中の人に見せない =
//!   自己ラベル `!no-unauthenticated`)と、チャットの受け付け `chat.bsky.actor.declaration/self`
//!   (`allowIncoming`)。`com.atproto.repo.getRecord` → 1 項目だけ変えて `putRecord`(`swapRecord` で
//!   ほかの端末の変更を上書きしない)→ 読み直す
//! - `timeline` … ローカル保存 — 値は毎呼び出しの `args.settings.timeline` に載る。`getTimeline` には
//!   リポスト / 返信を外す引数が無いので、取ってきたページから外す([`keep_feed_item`])
//! - `safety`   … ローカル保存 — ホストが投稿タイルの閲覧注意メディアの表示に使う
//! - `security` … 表示だけ — アプリパスワードでログインしていること、変更は bsky.app へ
//!
//! 成人向けコンテンツの表示(`adultContentPref`)は iOS のアプリでは切り替えない決まりなので、ここには置かず
//! bsky.app の設定へのリンクだけにする。

use rasq_plugin_kit::i18n;
use serde_json::{json, Value};

use crate::text::TEXT;
use crate::{err, grapheme_count, xrpc_get, xrpc_post, Ctx};

const PROFILE: &str = "app.bsky.actor.profile";
const CHAT_DECLARATION: &str = "chat.bsky.actor.declaration";
const NO_UNAUTHENTICATED: &str = "!no-unauthenticated";
const MAX_DISPLAY_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 256;

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
fn input(loc: &str, k: &str, label: &str, max_length: usize, multiline: bool) -> Value {
    json!({ "t": "Input", "k": k, "p": { "label": label, "value": format!("$.settings.{k}"), "maxLength": max_length, "multiline": multiline, "submitLabel": TEXT.t(loc, "settings.save"), "clearOnSubmit": false, "allowEmpty": true, "pad": 16 }, "on": { "submit": "settings.set" } })
}

/// bsky.app を開く(アカウントの PDS の web ではないので完全な URL)。
fn web_link(url: &str, title: &str, subtitle: Option<&str>) -> Value {
    json!({ "t": "ListTile", "k": url, "p": { "title": title, "subtitle": subtitle }, "on": { "tap": "web.open" } })
}

fn note(text: &str) -> Value {
    json!({ "t": "Text", "p": { "text": text, "pad": 16 } })
}

fn page(area: &str, title: &str, source: &str, defaults: Value, children: Vec<Value>) -> Value {
    json!({ "schema": "rasq.settings-page/1", "area": area, "title": title, "source": source, "defaults": defaults, "root": { "t": "Column", "c": children } })
}

// ── ページ ────────────────────────────────────────────────────────
// 文は crate::text::TEXT の表から `args.locale` で引く(PROTOCOL.md §4.6)。

/// チャットの受け付けの値と、表示名のキー。
const ALLOW_INCOMING: [(&str, &str); 3] = [("all", "account.allowIncoming.all"), ("following", "account.allowIncoming.following"), ("none", "account.allowIncoming.none")];

fn account_defaults() -> Value {
    json!({ "displayName": "", "description": "", "noUnauthenticated": false, "allowIncoming": "following" })
}

fn account_page(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    let mut c = vec![
        header(t("account.profile")),
        input(loc, "displayName", t("account.displayName"), MAX_DISPLAY_NAME, false),
        input(loc, "description", t("account.description"), MAX_DESCRIPTION, true),
        web_link("https://bsky.app/", t("account.avatarBanner"), Some(t("account.avatarBanner.sub"))),
        header(t("account.visibility")),
        switch("noUnauthenticated", t("account.noUnauthenticated"), Some(t("account.noUnauthenticated.sub"))),
        header(t("account.allowIncoming")),
    ];
    for (v, label) in ALLOW_INCOMING {
        c.push(radio("allowIncoming", v, t(label), None));
    }
    c.extend([
        web_link("https://bsky.app/moderation", t("account.moderation"), Some(t("settings.onBskyApp"))),
        note(t("account.savedOnPds")),
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

/// 閲覧注意メディア。ホストが投稿タイルに当てる(どの SNS プラグインも同じキー)。
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
            note(t("security.appPasswordNote")),
            header(t("security.onBskyApp")),
            web_link("https://bsky.app/settings/app-passwords", t("security.appPasswords"), Some(t("security.appPasswords.sub"))),
            web_link("https://bsky.app/settings/account", t("security.account"), Some(t("security.account.sub"))),
        ],
    )
}

/// `call("sns.settingsPages")` → `{pages}`。文は `args.locale`(無ければ ja)で引く。
pub(crate) fn sns_settings_pages(args: &Value) -> Result<Value, Value> {
    let loc = i18n::locale_of(args);
    Ok(json!({ "pages": [account_page(loc), timeline_page(loc), safety_page(loc), security_page(loc)] }))
}

// ── timeline の当て方 ────────────────────────────────────────────

/// フォロー中 TL の 1 件(FeedViewPost)を残すか。自分の投稿への返信(スレッド)は残す。
pub(crate) fn keep_feed_item(item: &Value, tl_settings: &Value) -> bool {
    let repost = item["reason"]["$type"].as_str() == Some("app.bsky.feed.defs#reasonRepost");
    if tl_settings["withReposts"].as_bool() == Some(false) && repost {
        return false;
    }
    if tl_settings["withReplies"].as_bool() == Some(false) && !repost && item["post"]["record"]["reply"].is_object() {
        let author = &item["post"]["author"]["did"];
        let self_reply = author.is_string() && item["reply"]["parent"]["author"]["did"] == *author;
        if !self_reply {
            return false;
        }
    }
    true
}

// ── account の読み書き ────────────────────────────────────────────

/// 自分のリポジトリの 1 レコード `(value, cid)`。まだ無ければ `({}, None)`。
fn get_own_record(ctx: &Ctx, collection: &str) -> Result<(Value, Option<String>), Value> {
    match xrpc_get(ctx, "com.atproto.repo.getRecord", &[("repo", ctx.did()), ("collection", collection.into()), ("rkey", "self".into())]) {
        Ok(r) => Ok((r["value"].clone(), r["cid"].as_str().map(str::to_string))),
        Err(e) if e["message"].as_str().unwrap_or("").starts_with("RecordNotFound") => Ok((json!({}), None)),
        Err(e) => Err(e),
    }
}

fn put_own_record(ctx: &Ctx, collection: &str, mut record: Value, swap: Option<String>) -> Result<(), Value> {
    record["$type"] = json!(collection);
    let mut body = json!({ "repo": ctx.did(), "collection": collection, "rkey": "self", "record": record });
    // swapRecord: 読んでから書くまでにほかの端末が変えていたら、上書きせずに失敗させる。null = まだ無いことの確認。
    body["swapRecord"] = swap.map(Value::String).unwrap_or(Value::Null);
    xrpc_post(ctx, "com.atproto.repo.putRecord", body).map(|_| ())
}

fn self_labels(profile: &Value) -> Vec<String> {
    profile["labels"]["values"].as_array().map(|a| a.iter().filter_map(|l| l["val"].as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn account_values(profile: &Value, declaration: &Value) -> Value {
    json!({
        "displayName": profile["displayName"].as_str().unwrap_or(""),
        "description": profile["description"].as_str().unwrap_or(""),
        "noUnauthenticated": self_labels(profile).iter().any(|l| l == NO_UNAUTHENTICATED),
        "allowIncoming": declaration["allowIncoming"].as_str().unwrap_or("following"),
    })
}

fn read_account(ctx: &Ctx) -> Result<Value, Value> {
    let (profile, _) = get_own_record(ctx, PROFILE)?;
    // チャットの宣言が読めなくても(チャットを使ったことが無い等)プロフィールは出す。
    let declaration = get_own_record(ctx, CHAT_DECLARATION).map(|(v, _)| v).unwrap_or(json!({}));
    Ok(account_values(&profile, &declaration))
}

/// プロフィールのレコードの 1 項目を変える(ほかの項目 = アイコン・バナー・固定投稿などはそのまま)。
fn apply_profile_key(profile: &mut Value, key: &str, value: &Value, loc: &str) -> Result<(), Value> {
    if !profile.is_object() {
        *profile = json!({});
    }
    match key {
        "displayName" | "description" => {
            let s = value.as_str().unwrap_or("").trim();
            let max = if key == "displayName" { MAX_DISPLAY_NAME } else { MAX_DESCRIPTION };
            if grapheme_count(s) > max {
                return Err(err("D01", TEXT.f(loc, "error.tooLong", &[("max", &max.to_string())])));
            }
            if s.is_empty() {
                profile.as_object_mut().map(|m| m.remove(key));
            } else {
                profile[key] = json!(s);
            }
        }
        "noUnauthenticated" => {
            let on = value.as_bool().ok_or_else(|| err("D01", TEXT.t(loc, "error.notBool")))?;
            let mut labels: Vec<String> = self_labels(profile).into_iter().filter(|l| l != NO_UNAUTHENTICATED).collect();
            if on {
                labels.push(NO_UNAUTHENTICATED.to_string());
            }
            if labels.is_empty() {
                profile.as_object_mut().map(|m| m.remove("labels"));
            } else {
                let values: Vec<Value> = labels.iter().map(|l| json!({ "val": l })).collect();
                profile["labels"] = json!({ "$type": "com.atproto.label.defs#selfLabels", "values": values });
            }
        }
        other => return Err(err("C01", format!("unknown account setting: {other}"))),
    }
    Ok(())
}

pub(crate) fn sns_settings_read(ctx: &Ctx) -> Result<Value, Value> {
    if ctx.args["area"].as_str() != Some("account") {
        return Err(err("C01", "no such remote settings area"));
    }
    Ok(json!({ "values": read_account(ctx)? }))
}

pub(crate) fn sns_settings_write(ctx: &Ctx) -> Result<Value, Value> {
    if ctx.args["area"].as_str() != Some("account") {
        return Err(err("C01", "no such remote settings area"));
    }
    let key = ctx.args["key"].as_str().ok_or_else(|| err("D01", "key required"))?;
    let value = &ctx.args["value"];
    if key == "allowIncoming" {
        let v = value.as_str().filter(|s| ALLOW_INCOMING.iter().any(|(a, _)| a == s)).ok_or_else(|| err("D01", TEXT.t(i18n::locale_of(ctx.args), "error.allowIncoming")))?;
        let (_, cid) = get_own_record(ctx, CHAT_DECLARATION)?;
        put_own_record(ctx, CHAT_DECLARATION, json!({ "allowIncoming": v }), cid)?;
    } else {
        let (mut profile, cid) = get_own_record(ctx, PROFILE)?;
        apply_profile_key(&mut profile, key, value, i18n::locale_of(ctx.args))?;
        put_own_record(ctx, PROFILE, profile, cid)?;
    }
    // 書いたあと読み直す(サーバーが受け付けた形を出す)。
    Ok(json!({ "values": read_account(ctx)? }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dispatch, set_host_hook, take_host_log};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn args(extra: Value) -> Value {
        let token = json!({ "did": "did:plc:me", "handle": "me.bsky.social", "accessJwt": "a", "refreshJwt": "r" }).to_string();
        let mut a = json!({ "host": "pds.example", "token": token });
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        a
    }

    /// PDS をまねる: レコードは `repo` の中(collection → (value, cid))。putRecord で cid が進む。
    fn mock_pds(repo: Rc<RefCell<Vec<(String, Value, String)>>>, puts: Rc<RefCell<Vec<Value>>>) {
        set_host_hook(move |fn_name, a| {
            if fn_name != "net.fetch" {
                return json!({});
            }
            let url = a["url"].as_str().unwrap_or("").to_string();
            let nsid = url.split("/xrpc/").nth(1).unwrap_or("").split('?').next().unwrap_or("").to_string();
            let (status, body) = match nsid.as_str() {
                "com.atproto.repo.getRecord" => {
                    let collection = url.split("collection=").nth(1).unwrap_or("").split('&').next().unwrap_or("").to_string();
                    match repo.borrow().iter().find(|(c, _, _)| *c == collection) {
                        Some((_, v, cid)) => (200, json!({ "uri": "at://x", "cid": cid, "value": v })),
                        None => (400, json!({ "error": "RecordNotFound", "message": "Could not locate record" })),
                    }
                }
                "com.atproto.repo.putRecord" => {
                    let b: Value = serde_json::from_str(a["body"].as_str().unwrap_or("null")).unwrap();
                    puts.borrow_mut().push(b.clone());
                    let collection = b["collection"].as_str().unwrap().to_string();
                    let mut r = repo.borrow_mut();
                    r.retain(|(c, _, _)| *c != collection);
                    r.push((collection, b["record"].clone(), "cid-new".into()));
                    (200, json!({ "uri": "at://x", "cid": "cid-new" }))
                }
                _ => (404, json!({})),
            };
            json!({ "status": status, "headers": {}, "bodyText": body.to_string() })
        });
        let _ = take_host_log();
    }

    /// 利用者に見せる文は表から引く(`args.locale`、PROTOCOL.md §4.6)。いまは ja だけなので、どの言語でも ja と同じ文。
    #[test]
    fn ui_text_comes_from_the_table_and_falls_back_to_ja() {
        assert!(TEXT.check().is_empty(), "{:?}", TEXT.check());
        let _ = i18n::take_misses();
        for f in ["sns.settingsPages", "sns.actions", "sns.timelines"] {
            let base = dispatch(f, &json!({})).unwrap();
            for loc in ["ja", "ja-JP", "en", "en-US", "zh-Hant", "", "??"] {
                assert_eq!(dispatch(f, &json!({ "locale": loc })).unwrap(), base, "{f} locale={loc}");
            }
        }
        let pages = dispatch("sns.settingsPages", &json!({ "locale": "en" })).unwrap()["pages"].clone();
        assert_eq!(pages[0]["title"], "アカウント");
        assert_eq!(pages[0]["root"]["c"][7]["p"]["title"], "だれでも");
        assert_eq!(pages[3]["root"]["c"][1]["p"]["value"], "アプリパスワード");
        assert_eq!(dispatch("sns.timelines", &json!({})).unwrap()["timelines"][0]["label"], "フォロー中");
        // 書き込みのエラー(サーバーに行く前に断る)
        let mut profile = json!({});
        let e = apply_profile_key(&mut profile, "displayName", &json!("あ".repeat(MAX_DISPLAY_NAME + 1)), "en").unwrap_err();
        assert_eq!(e["message"], "64 文字までです");
        let e = dispatch("sns.settingsWrite", &args(json!({ "area": "account", "key": "allowIncoming", "value": "everyone", "locale": "en" }))).unwrap_err();
        assert_eq!(e["message"], "all / following / none のどれかです");
        assert!(i18n::take_misses().is_empty(), "表に無いキーを引いた");
    }

    #[test]
    fn declares_the_four_pages_and_every_tile_key_has_a_default() {
        let pages = dispatch("sns.settingsPages", &json!({})).unwrap()["pages"].as_array().unwrap().clone();
        let areas: Vec<&str> = pages.iter().map(|p| p["area"].as_str().unwrap()).collect();
        assert_eq!(areas, ["account", "timeline", "safety", "security"]);
        for p in &pages {
            for node in p["root"]["c"].as_array().unwrap() {
                if node["on"]["change"] == "settings.set" || node["on"]["submit"] == "settings.set" {
                    let k = node["k"].as_str().unwrap();
                    assert!(p["defaults"].get(k).is_some(), "{}: {k} has no default", p["area"]);
                }
                // bsky.app へのリンクは完全な URL(PDS の web ではない)。
                if node["on"]["tap"] == "web.open" {
                    assert!(node["k"].as_str().unwrap().starts_with("https://bsky.app/"));
                }
            }
        }
    }

    #[test]
    fn account_read_maps_profile_labels_and_chat_declaration() {
        let repo = Rc::new(RefCell::new(vec![
            (PROFILE.to_string(), json!({ "displayName": "ミー", "avatar": { "ref": "blob" }, "labels": { "values": [{ "val": NO_UNAUTHENTICATED }] } }), "cid-1".to_string()),
        ]));
        mock_pds(repo, Rc::new(RefCell::new(vec![])));
        let v = dispatch("sns.settingsRead", &args(json!({ "area": "account" }))).unwrap()["values"].clone();
        assert_eq!(v, json!({ "displayName": "ミー", "description": "", "noUnauthenticated": true, "allowIncoming": "following" }));
    }

    #[test]
    fn account_write_keeps_other_profile_fields_and_swaps_on_the_read_cid() {
        let repo = Rc::new(RefCell::new(vec![(PROFILE.to_string(), json!({ "$type": PROFILE, "displayName": "古い", "avatar": { "ref": "blob" } }), "cid-1".to_string())]));
        let puts = Rc::new(RefCell::new(vec![]));
        mock_pds(repo, puts.clone());
        let v = dispatch("sns.settingsWrite", &args(json!({ "area": "account", "key": "description", "value": " 自己紹介です " }))).unwrap()["values"].clone();
        assert_eq!(v["description"], "自己紹介です");
        assert_eq!(v["displayName"], "古い");
        let put = puts.borrow()[0].clone();
        assert_eq!(put["repo"], "did:plc:me");
        assert_eq!(put["rkey"], "self");
        assert_eq!(put["swapRecord"], "cid-1");
        assert_eq!(put["record"]["avatar"], json!({ "ref": "blob" })); // アイコンは消さない
        assert_eq!(put["record"]["$type"], PROFILE);

        // ログアウト中の人に見せない → 自己ラベル。オフで labels ごと外す。
        dispatch("sns.settingsWrite", &args(json!({ "area": "account", "key": "noUnauthenticated", "value": true }))).unwrap();
        assert_eq!(puts.borrow()[1]["record"]["labels"]["values"], json!([{ "val": NO_UNAUTHENTICATED }]));
        assert_eq!(puts.borrow()[1]["swapRecord"], "cid-new");
        let v = dispatch("sns.settingsWrite", &args(json!({ "area": "account", "key": "noUnauthenticated", "value": false }))).unwrap()["values"].clone();
        assert_eq!(v["noUnauthenticated"], false);
        assert!(puts.borrow()[2]["record"].get("labels").is_none());
    }

    #[test]
    fn chat_declaration_is_created_when_missing() {
        let repo = Rc::new(RefCell::new(vec![]));
        let puts = Rc::new(RefCell::new(vec![]));
        mock_pds(repo, puts.clone());
        let v = dispatch("sns.settingsWrite", &args(json!({ "area": "account", "key": "allowIncoming", "value": "none" }))).unwrap()["values"].clone();
        assert_eq!(v["allowIncoming"], "none");
        let put = puts.borrow()[0].clone();
        assert_eq!(put["collection"], CHAT_DECLARATION);
        assert_eq!(put["record"], json!({ "$type": CHAT_DECLARATION, "allowIncoming": "none" }));
        assert!(put["swapRecord"].is_null());
    }

    #[test]
    fn account_write_rejects_unknown_keys_bad_values_and_long_text() {
        mock_pds(Rc::new(RefCell::new(vec![])), Rc::new(RefCell::new(vec![])));
        let w = |key: &str, value: Value| dispatch("sns.settingsWrite", &args(json!({ "area": "account", "key": key, "value": value }))).unwrap_err()["code"].clone();
        assert_eq!(w("handle", json!("x.bsky.social")), "C01");
        assert_eq!(w("allowIncoming", json!("friends")), "D01");
        assert_eq!(w("noUnauthenticated", json!("yes")), "D01");
        assert_eq!(w("displayName", json!("あ".repeat(MAX_DISPLAY_NAME + 1))), "D01");
        assert_eq!(dispatch("sns.settingsRead", &args(json!({ "area": "timeline" }))).unwrap_err()["code"], "C01");
        // 未ログインは 401(ページの宣言だけは token 無しで答える)。
        assert_eq!(dispatch("sns.settingsRead", &json!({ "area": "account" })).unwrap_err()["code"], "401");
    }

    #[test]
    fn timeline_settings_drop_reposts_and_replies_but_keep_self_threads() {
        let me = json!({ "did": "did:plc:a" });
        let plain = json!({ "post": { "uri": "1", "author": me, "record": {} } });
        let repost = json!({ "post": { "uri": "2", "author": me, "record": {} }, "reason": { "$type": "app.bsky.feed.defs#reasonRepost" } });
        let reply = json!({ "post": { "uri": "3", "author": me, "record": { "reply": {} } }, "reply": { "parent": { "author": { "did": "did:plc:b" } } } });
        let thread = json!({ "post": { "uri": "4", "author": me, "record": { "reply": {} } }, "reply": { "parent": { "author": me } } });
        let off = json!({ "withReposts": false, "withReplies": false });
        let kept: Vec<&str> = [&plain, &repost, &reply, &thread].iter().filter(|i| keep_feed_item(i, &off)).map(|i| i["post"]["uri"].as_str().unwrap()).collect();
        assert_eq!(kept, ["1", "4"]);
        assert!([&plain, &repost, &reply, &thread].iter().all(|i| keep_feed_item(i, &Value::Null)));
    }
}
