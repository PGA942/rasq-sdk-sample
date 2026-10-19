//! 設定ページ (D-044 / rasq-client D-482): `sns.settingsPages` / `sns.settingsRead` / `sns.settingsWrite`。
//!
//! ホストの「SNSプラグイン」の行(アカウント / タイムライン / セキュリティ / プライバシーとセーフモード)は、
//! プラグインが `sns.settingsPages` で返した `rasq.settings-page/1` のページを開く。返さないページの行は
//! ホストが出さない。
//!
//! - `account`  … source: remote — `i` / `i/update`(プロフィールの文字、公開範囲、アカウントの種類)
//! - `timeline` … ローカル保存 — 値は毎呼び出しの `args.settings.timeline` に載り、TL 取得の
//!   `withRenotes` / `withReplies` になる([`super::sns_timeline`] が読む)
//! - `safety`   … ローカル保存 — ホストが投稿タイルの閲覧注意メディア / 注釈 (CW) の表示に使う
//! - `security` … 表示だけ — パスワードが要る操作は web 版へ(`on: {"tap": "web.open"}`)

use serde_json::{json, Value};

use crate::misskey_fetch;

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

/// 文字の設定(名前・自己紹介など)。「保存」で `settings.set`(payload.text)。
fn input(k: &str, label: &str, max_length: u32, multiline: bool) -> Value {
    json!({ "t": "Input", "k": k, "p": { "label": label, "value": format!("$.settings.{k}"), "maxLength": max_length, "multiline": multiline, "submitLabel": "保存", "clearOnSubmit": false, "allowEmpty": true, "pad": 16 }, "on": { "submit": "settings.set" } })
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

const VISIBILITIES: [(&str, &str); 3] = [("public", "公開"), ("followers", "フォロワーだけ"), ("private", "非公開")];

fn account_page() -> Value {
    let mut c = vec![
        header("プロフィール"),
        input("name", "名前", 50, false),
        input("description", "自己紹介", 1500, true),
        input("location", "場所", 50, false),
        input("birthday", "誕生日(例: 2000-01-31)", 10, false),
        web_link("settings/profile", "アイコン・バナー・補足情報", Some("web 版で編集します")),
        header("公開範囲"),
        switch("isLocked", "フォローを承認制にする", Some("フォローされる前にあなたの承認が必要になります")),
        switch("autoAcceptFollowed", "フォロー中の人からのフォローは自動で承認", None),
        switch("isExplorable", "アカウントを見つけやすくする", Some("「みつける」やユーザー検索に表示されます")),
        switch("hideOnlineStatus", "オンライン状態を隠す", None),
        switch("publicReactions", "リアクション一覧を公開する", None),
        switch("noCrawle", "検索エンジンに載せない", Some("検索エンジンによるインデックスを避けるよう求めます")),
        header("フォローの一覧を見せる相手"),
    ];
    for (v, label) in VISIBILITIES {
        c.push(radio("followingVisibility", v, label, None));
    }
    c.push(header("フォロワーの一覧を見せる相手"));
    for (v, label) in VISIBILITIES {
        c.push(radio("followersVisibility", v, label, None));
    }
    c.extend([
        header("アカウントの種類"),
        switch("isBot", "Botとして設定", None),
        switch("isCat", "Catにする", Some("発言が Cat 語になり、猫耳が付きます")),
        note("これらの設定は接続先の Misskey サーバーに保存されます。"),
    ]);
    page("account", "アカウント", "remote", account_defaults(), c)
}

fn account_defaults() -> Value {
    json!({
        "name": "", "description": "", "location": "", "birthday": "",
        "isLocked": false, "autoAcceptFollowed": true, "isExplorable": true, "hideOnlineStatus": false,
        "publicReactions": true, "noCrawle": false, "followingVisibility": "public", "followersVisibility": "public",
        "isBot": false, "isCat": false,
    })
}

fn timeline_page() -> Value {
    page(
        "timeline",
        "タイムライン",
        "local",
        json!({ "withRenotes": true, "withReplies": false }),
        vec![
            header("表示する投稿"),
            switch("withRenotes", "リノートを表示", Some("オフにすると引用のないリノートを取得しません")),
            switch("withReplies", "他の人への返信を表示", Some("フォロー中の人が他の人へ送った返信をTLに含めます")),
        ],
    )
}

/// 閲覧注意メディア / 注釈 (CW)。ホストが投稿タイルに当てる(どの SNS プラグインも同じキー)。
fn safety_page() -> Value {
    page(
        "safety",
        "プライバシーとセーフモード",
        "local",
        json!({ "sensitiveMedia": "blur", "openCw": false }),
        vec![
            header("閲覧注意(センシティブ)メディア"),
            radio("sensitiveMedia", "blur", "ぼかして表示", Some("タップで開きます(標準)")),
            radio("sensitiveMedia", "show", "ぼかさず表示", None),
            radio("sensitiveMedia", "hide", "表示しない(セーフモード)", Some("閲覧注意メディアをタイルから取り除きます")),
            header("注釈(CW)"),
            switch("openCw", "注釈付き投稿を自動で開く", None),
        ],
    )
}

fn security_page() -> Value {
    page(
        "security",
        "セキュリティ",
        "local",
        json!({}),
        vec![
            header("web で行う設定"),
            web_link("settings/security", "パスワード・二要素認証・ログイン履歴", Some("パスワードが必要な操作は web 版で行います")),
            web_link("settings/api", "アクセストークンの管理", Some("発行済みトークンの確認と失効")),
            web_link("settings/apps", "連携しているアプリ", Some("アプリに与えた許可の確認と取り消し")),
            note("アクセストークンは端末内の安全な領域にのみ保存されます。"),
        ],
    )
}

/// `call("sns.settingsPages")` → `{pages}`。
pub(crate) fn sns_settings_pages(_args: &Value) -> Result<Value, Value> {
    Ok(json!({ "pages": [account_page(), timeline_page(), safety_page(), security_page()] }))
}

// ── account の読み書き ────────────────────────────────────────────

const BOOL_KEYS: [&str; 8] = ["isLocked", "autoAcceptFollowed", "isExplorable", "hideOnlineStatus", "publicReactions", "noCrawle", "isBot", "isCat"];
const TEXT_KEYS: [&str; 4] = ["name", "description", "location", "birthday"];
const VISIBILITY_KEYS: [&str; 2] = ["followingVisibility", "followersVisibility"];

fn account_values(me: &Value) -> Value {
    let defaults = account_defaults();
    let mut out = json!({});
    for k in BOOL_KEYS {
        out[k] = json!(me[k].as_bool().unwrap_or(defaults[k].as_bool().unwrap_or(false)));
    }
    for k in TEXT_KEYS {
        out[k] = json!(me[k].as_str().unwrap_or(""));
    }
    // 古いサーバー(2023.12 より前)は両方を `ffVisibility` 1 つで持つ。
    for k in VISIBILITY_KEYS {
        out[k] = json!(me[k].as_str().or(me["ffVisibility"].as_str()).unwrap_or("public"));
    }
    out
}

fn is_birthday(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// 1 キーを `i/update` の本文へ。許可リストに無いキー / 形の違う値は断る。
fn account_update_body(key: &str, value: &Value) -> Result<Value, Value> {
    let bad = |m: &str| Err(json!({ "code": "D01", "message": m }));
    let v = if BOOL_KEYS.contains(&key) {
        match value.as_bool() {
            Some(b) => json!(b),
            None => return bad("値は true / false です"),
        }
    } else if TEXT_KEYS.contains(&key) {
        let s = value.as_str().unwrap_or("").trim();
        match key {
            // 名前と誕生日は空 = 消す(null)。
            "name" | "birthday" | "location" if s.is_empty() => Value::Null,
            "birthday" if !is_birthday(s) => return bad("誕生日は 2000-01-31 の形で入れてください"),
            _ => json!(s),
        }
    } else if VISIBILITY_KEYS.contains(&key) {
        match value.as_str() {
            Some(s) if VISIBILITIES.iter().any(|(v, _)| *v == s) => json!(s),
            _ => return bad("公開範囲は public / followers / private です"),
        }
    } else {
        return Err(json!({ "code": "C01", "message": format!("unknown account setting: {key}") }));
    };
    let mut body = json!({});
    body[key] = v;
    Ok(body)
}

/// `call("sns.settingsRead", {area})`: remote ページ(`account`)だけ。
pub(crate) fn sns_settings_read(args: &Value) -> Result<Value, Value> {
    if args["area"].as_str() != Some("account") {
        return Err(json!({ "code": "C01", "message": "no such remote settings area" }));
    }
    let me = misskey_fetch(args, "i", json!({}))?;
    Ok(json!({ "values": account_values(&me) }))
}

/// `call("sns.settingsWrite", {area, key, value})` → 書いたあと読み直した `{values}`。
pub(crate) fn sns_settings_write(args: &Value) -> Result<Value, Value> {
    if args["area"].as_str() != Some("account") {
        return Err(json!({ "code": "C01", "message": "no such remote settings area" }));
    }
    let Some(key) = args["key"].as_str() else {
        return Err(json!({ "code": "D01", "message": "key required" }));
    };
    let body = account_update_body(key, &args["value"])?;
    // i/update は更新後の自分を返す(サーバーが弾いた変更もそのまま見える)。
    let me = misskey_fetch(args, "i/update", body)?;
    Ok(json!({ "values": account_values(&me) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{set_host_hook, take_host_log};

    /// `net.fetch` を Misskey の API 単位でまねる。`handler(endpoint, body)` の戻りが 200 の本文。
    fn mock_api<F: FnMut(&str, &Value) -> Value + 'static>(mut handler: F) {
        set_host_hook(move |fn_name, a| match fn_name {
            "net.fetch" => {
                let url = a["url"].as_str().unwrap_or("");
                let endpoint = url.split("/api/").nth(1).unwrap_or("").to_string();
                let body: Value = serde_json::from_str(a["body"].as_str().unwrap_or("null")).unwrap_or(Value::Null);
                json!({ "status": 200, "headers": {}, "bodyText": handler(&endpoint, &body).to_string() })
            }
            _ => json!({}),
        });
        let _ = take_host_log();
    }

    fn args(extra: Value) -> Value {
        let mut a = json!({ "host": "misskey.example", "token": "tok" });
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
            assert_eq!(p["schema"], "rasq.settings-page/1");
            for node in p["root"]["c"].as_array().unwrap() {
                if node["on"]["change"] == "settings.set" || node["on"]["submit"] == "settings.set" {
                    let k = node["k"].as_str().unwrap();
                    assert!(p["defaults"].get(k).is_some(), "{}: {k} has no default", p["area"]);
                }
            }
        }
        assert_eq!(pages[0]["source"], "remote");
        assert_eq!(pages[1]["defaults"], json!({ "withRenotes": true, "withReplies": false }));
    }

    #[test]
    fn account_read_maps_i_and_falls_back_to_ff_visibility() {
        mock_api(|ep, _| {
            assert_eq!(ep, "i");
            json!({ "id": "me", "name": "ゼポス", "description": null, "birthday": "2000-01-31", "isLocked": true, "isCat": true, "ffVisibility": "followers" })
        });
        let v = sns_settings_read(&args(json!({ "area": "account" }))).unwrap()["values"].clone();
        assert_eq!(v["name"], "ゼポス");
        assert_eq!(v["description"], "");
        assert_eq!(v["birthday"], "2000-01-31");
        assert_eq!(v["isLocked"], true);
        assert_eq!(v["isCat"], true);
        assert_eq!(v["isExplorable"], true); // 欠けたら既定
        assert_eq!(v["followingVisibility"], "followers");
        assert_eq!(v["followersVisibility"], "followers");
        let sent = take_host_log();
        assert_eq!(serde_json::from_str::<Value>(sent[0].1["body"].as_str().unwrap()).unwrap()["i"], "tok");
    }

    #[test]
    fn account_write_sends_one_key_to_i_update_and_returns_the_server_state() {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<(String, Value)>::new()));
        let log = seen.clone();
        mock_api(move |ep, body| {
            log.borrow_mut().push((ep.to_string(), body.clone()));
            json!({ "name": body["name"].clone(), "followingVisibility": "private" })
        });
        let v = sns_settings_write(&args(json!({ "area": "account", "key": "name", "value": "  新しい名前 " }))).unwrap()["values"].clone();
        assert_eq!(v["name"], "新しい名前");
        assert_eq!(v["followingVisibility"], "private");
        let (ep, body) = seen.borrow()[0].clone();
        assert_eq!(ep, "i/update");
        assert_eq!(body["name"], "新しい名前");
        assert_eq!(body["i"], "tok");
    }

    #[test]
    fn account_write_rejects_unknown_keys_and_bad_values_without_calling_the_server() {
        mock_api(|_, _| panic!("must not call the server"));
        let w = |key: &str, value: Value| sns_settings_write(&args(json!({ "area": "account", "key": key, "value": value }))).unwrap_err()["code"].clone();
        assert_eq!(w("isAdmin", json!(true)), "C01");
        assert_eq!(w("isLocked", json!("yes")), "D01");
        assert_eq!(w("birthday", json!("31/01/2000")), "D01");
        assert_eq!(w("followersVisibility", json!("everyone")), "D01");
        assert_eq!(sns_settings_read(&args(json!({ "area": "timeline" }))).unwrap_err()["code"], "C01");
    }

    #[test]
    fn empty_birthday_clears_it() {
        assert_eq!(account_update_body("birthday", &json!("")).unwrap(), json!({ "birthday": null }));
        assert_eq!(account_update_body("description", &json!("")).unwrap(), json!({ "description": "" }));
    }
}
