//! 設定ページ (D-044 / rasq-client D-482): `sns.settingsPages` / `sns.settingsRead`。
//!
//! ホストの「SNSプラグイン」の行(アカウント / タイムライン / セキュリティ / プライバシーとセーフモード)は、
//! プラグインが `sns.settingsPages` で返した `rasq.settings-page/1` のページを開く。返さないページの行は
//! ホストが出さない。
//!
//! mixi2 の API(申請制、実モードのエンドポイントは仮置き)にはプロフィールや公開範囲を変える口が
//! 見当たらないので、`account` は**読むだけ**(`sns.me` と同じもの)にし、変えるのは mixi2 のアプリと案内する。
//! `timeline` / `safety` はローカル保存で、どの SNS でも同じように効く。

use serde_json::{json, Value};

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

fn account_page() -> Value {
    page(
        "account",
        "アカウント",
        "remote",
        json!({ "displayName": "", "handle": "", "bio": "" }),
        vec![
            header("プロフィール"),
            value_tile("表示名", "displayName"),
            value_tile("ユーザー名", "handle"),
            header("自己紹介"),
            json!({ "t": "Text", "p": { "text": "$.settings.bio", "pad": 16 } }),
            notice("mixi2 の API ではプロフィールや公開範囲を変えられないため、ここでは表示だけです。変えるときは mixi2 のアプリで行ってください。"),
        ],
    )
}

fn timeline_page() -> Value {
    page(
        "timeline",
        "タイムライン",
        "local",
        json!({ "withReposts": true, "withReplies": true }),
        vec![
            header("表示する投稿"),
            switch("withReposts", "リポストを表示", Some("オフにするとリポストをタイムラインから外します")),
            switch("withReplies", "返信を表示", Some("オフにすると返信をタイムラインから外します")),
        ],
    )
}

/// 閲覧注意メディア / 注釈。ホストが投稿タイルに当てる(どの SNS プラグインも同じキー)。
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
            header("注釈"),
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
            header("ログインの方法"),
            json!({ "t": "ValueListTile", "p": { "title": "ログインに使っているもの", "value": "RASQ サーバー経由の OAuth" } }),
            note("RASQ が受け取るのは mixi2 が発行したアクセストークンだけで、パスワードは受け取りません。トークンは端末内の安全な領域にのみ保存されます。"),
            notice("パスワード・二要素認証・連携しているアプリの取り消しは、mixi2 のアプリの設定で行ってください。"),
        ],
    )
}

/// `call("sns.settingsPages")` → `{pages}`。
pub(crate) fn sns_settings_pages() -> Value {
    json!({ "pages": [account_page(), timeline_page(), safety_page(), security_page()] })
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
