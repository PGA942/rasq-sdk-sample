//! Mastodon WASM plugin for RASQ (PROTOCOL.md §4.3 の `sns.*` 境界)。
//!
//! [`misskey-wasm-sample`](../misskey-wasm-sample) と同じ実装パターン
//! ——ホスト関数 `net.fetch` 越しに SNS の API を叩き、`rasq.post/1` の
//! ワイヤを返す——を Mastodon に当てたもの。RRL の画面コンストラクタは
//! 持たない**イベントハンドラー専用プラグイン**で、描画はホストの標準
//! PostTile に任せる(タイルを差し替えたくなったら `tl.customize` を足す)。
//!
//! Misskey サンプルとの主な違いは 4 点。どれも SNS 側の作りの違いで、
//! **契約(ワイヤ)は同じ形に揃えてある**:
//!
//! 1. **本文が HTML**。Mastodon は `<p>` / `<br>` / `<a>` / `<span>` の
//!    限られた HTML を返す。ホストの本文パイプライン (CONTRACT §10 の RSL)
//!    はプレーンテキストにインライン解析をかける前提なので、プラグイン側で
//!    平文へ潰してから渡す([`html_to_text`])。`bodyFormat` は `plain`。
//! 2. **REST の作りが違う**。Misskey は全部 `POST /api/<endpoint>` に
//!    JSON を投げるが、Mastodon は GET/POST を使い分けてクエリ文字列を取る。
//!    認証は `Authorization: Bearer`([`fetch`])。
//! 3. **ページングが `max_id`**。Misskey の `untilId` に相当する。
//! 4. **リアクションが無い**。お気に入り(favourite)を唯一の「リアクション」
//!    として `sns.reactions` に写し、絵文字は ⭐ 固定にする。引用は
//!    ネイティブに存在しないので `sns.quotes` は C01 を返し、ホストが
//!    「共有(引用ノート)」欄を隠す。
//!
//! ブースト(reblog)は Misskey の純リノートと同じ規則で中身へ展開し、
//! `repostedBy` と `repostTargetId` を載せる (D-100 / D-112)。

use serde_json::{json, Value};
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};

const SCOPE: &str = "rasq.official.mastodon:mastodon:mastodon.social:sample:0.1.0:sns";

#[link(wasm_import_module = "rasq")]
extern "C" {
    /// ホスト関数 import (PROTOCOL §4.1)。リクエスト JSON の ptr/len を渡し、
    /// レスポンスを書いた `(ptr << 32) | len` を受け取る(同期 v1)。
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

// ── wasm ABI (PROTOCOL §4.1) ──────────────────────────────────────

fn host_call(fn_name: &str, args: Value) -> Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
    let bytes = request.as_bytes();
    let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
    let ptr = (packed >> 32) as u32;
    let len = (packed & 0xFFFF_FFFF) as u32;
    // ホストは我々の `alloc` で確保した領域に書いたので、ここで回収する。
    let response = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
    serde_json::from_slice(&response).unwrap_or(Value::Null)
}

fn pack(mut buf: Vec<u8>) -> u64 {
    // `dealloc` は `Vec::from_raw_parts(ptr, len, len)` で再構成するので、
    // capacity は len と一致していなければならない。
    buf.shrink_to_fit();
    let len = buf.len() as u64;
    let ptr = buf.as_ptr() as u64;
    mem::forget(buf);
    (ptr << 32) | len
}

fn read_str(ptr: u32, len: u32) -> String {
    unsafe {
        let slice = std::slice::from_raw_parts(ptr as *const u8, len as usize);
        String::from_utf8_lossy(slice).into_owned()
    }
}

#[no_mangle]
pub extern "C" fn alloc(len: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr() as u32;
    mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: u32, len: u32) {
    unsafe {
        drop(Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize));
    }
}

/// 画面コンストラクタを持たないプラグインなので、ツリーは空のまま。
/// ホストはランタイムのロード時に一度これを読む。
#[no_mangle]
pub extern "C" fn snapshot() -> u64 {
    let msg = json!({
        "schema": "rasq.boundary/1",
        "scope": SCOPE,
        "type": "snapshot",
        "rev": 1,
        "root": { "t": "Column", "k": "root", "c": [] }
    });
    pack(msg.to_string().into_bytes())
}

/// 差分は出さない(ツリーを持たないため)。
#[no_mangle]
pub extern "C" fn patch() -> u64 {
    pack(Value::Null.to_string().into_bytes())
}

// ── Mastodon REST ────────────────────────────────────────────────

/// クエリ値の percent-encoding(RFC 3986 の unreserved 以外を全部逃がす)。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `[(key, value)]` → `?a=1&b=2`。空の値は落とす(`max_id` 未指定など)。
fn query_string(params: &[(&str, String)]) -> String {
    let pairs: Vec<String> = params
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

/// Mastodon API を `net.fetch` 経由で叩く共通口。
///
/// `args.host` / `args.baseUrl` で接続先、`args.token` があれば
/// `Authorization: Bearer` を付ける(Misskey サンプルが `i` をボディに
/// 積むのと同じ役割)。2xx 以外はエラー、204/空ボディは Null。
fn fetch(args: &Value, method: &str, path: &str, body: Option<Value>) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social");
    // baseUrl はテストと自前ホスティング向けの上書き(既定は https)。
    let base = match args["baseUrl"].as_str() {
        Some(b) => b.trim_end_matches('/').to_string(),
        None => format!("https://{host}"),
    };
    let mut headers = json!({ "Accept": "application/json" });
    if let Some(token) = args["token"].as_str() {
        headers["Authorization"] = json!(format!("Bearer {token}"));
    }
    let mut request = json!({
        "method": method,
        "url": format!("{base}{path}"),
        "headers": headers,
    });
    if let Some(b) = body {
        request["headers"]["Content-Type"] = json!("application/json");
        request["body"] = json!(b.to_string());
    }

    let response = host_call("net.fetch", request);
    if response["ok"].as_bool() != Some(true) {
        return Err(response
            .get("error")
            .cloned()
            .unwrap_or(json!({ "code": "B01", "message": "net.fetch failed" })));
    }
    let value = &response["value"];
    let status = value["status"].as_u64().unwrap_or(0);
    if !(200..300).contains(&status) {
        // 401/403/404 はそのまま HTTP コードで返す — ホストの
        // SnsApiException がコードごとに扱いを変える (CONTRACT §9)。
        return Err(json!({ "code": status.to_string(), "message": format!("HTTP {status} from {path}") }));
    }
    let text = value["bodyText"].as_str().unwrap_or("");
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).map_err(|e| json!({ "code": "D01", "message": format!("bad JSON from {path}: {e}") }))
}

fn get(args: &Value, path: &str) -> Result<Value, Value> {
    fetch(args, "GET", path, None)
}

fn post(args: &Value, path: &str, body: Value) -> Result<Value, Value> {
    fetch(args, "POST", path, Some(body))
}

/// 一覧の次カーソル = 最後の要素の id(Mastodon の `max_id` ページング)。
/// Link ヘッダーも使えるが、`max_id` の方がプロキシを跨いでも壊れない。
fn next_cursor(list: &[Value]) -> Value {
    match list.last().and_then(|v| v["id"].as_str()) {
        Some(id) => json!(id),
        None => Value::Null,
    }
}

fn as_list(v: Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

// ── HTML → 平文 ──────────────────────────────────────────────────

/// Mastodon の本文 HTML を平文に潰す。
///
/// ホストは本文をプレーンテキストとして受け取り、URL / `@mention` /
/// `#hashtag` / `:emoji:` を自前で解析する (CONTRACT §10)。生の HTML を
/// 渡すとタグがそのまま出てしまうので、プラグイン側で落としておく。
///
/// Mastodon が使う HTML は限定的(`<p>` `<br>` `<a>` `<span>`)なので、
/// 段落と改行だけ保って残りのタグは剥がす。`<a>` はリンクテキスト
/// (Mastodon は表示用に短縮した URL を入れている)をそのまま残し、
/// ホストのリンク検出に任せる。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        // タグを読み切る。
        let mut tag = String::new();
        for t in chars.by_ref() {
            if t == '>' {
                break;
            }
            tag.push(t);
        }
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        match name.as_str() {
            "br" => out.push('\n'),
            // 段落の切れ目は空行。先頭の `<p>` では改行を足さない。
            "p" if closing => out.push_str("\n\n"),
            _ => {}
        }
    }
    decode_entities(out.trim())
}

/// Mastodon が出す範囲の HTML 実体参照を戻す。
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" | "#39" => Some("'".to_string()),
            "nbsp" => Some(" ".to_string()),
            e if e.starts_with("#x") || e.starts_with("#X") => {
                u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32).map(String::from)
            }
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32).map(String::from),
            _ => None,
        };
        match decoded {
            Some(d) => {
                out.push_str(&d);
                rest = &rest[end + 1..];
            }
            None => {
                // 知らない実体はそのまま残す(壊すより読めた方がよい)。
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ── ワイヤ変換 ───────────────────────────────────────────────────

/// Mastodon の `acct` は、ローカルなら `name`、リモートなら `name@host`。
/// RASQ の `AccountId` は handle と instance を分けて持つので割る。
fn split_acct<'a>(acct: &'a str, fallback_instance: &'a str) -> (&'a str, &'a str) {
    match acct.split_once('@') {
        Some((handle, host)) => (handle, host),
        None => (acct, fallback_instance),
    }
}

/// Mastodon Account → `rasq.post/1` の user ワイヤ。
fn user_wire(account: &Value, fallback_instance: &str) -> Value {
    let acct = account["acct"].as_str().unwrap_or("?");
    let (handle, instance) = split_acct(acct, fallback_instance);
    let display = match account["display_name"].as_str() {
        Some(n) if !n.is_empty() => n,
        _ => handle,
    };
    json!({
        "id": account["id"],
        "kind": "mastodon",
        "instance": instance,
        "handle": handle,
        "displayName": display,
        "avatarUrl": account["avatar"],
        "bio": account["note"].as_str().map(html_to_text),
    })
}

fn media_type(kind: &str) -> &'static str {
    match kind {
        "image" => "image",
        "gifv" => "gif",
        "video" => "video",
        "audio" => "file",
        _ => "file",
    }
}

fn media_wire(m: &Value) -> Value {
    json!({
        "id": m["id"],
        "type": media_type(m["type"].as_str().unwrap_or("")),
        "url": m["url"],
        "thumbnailUrl": m["preview_url"],
        "alt": m["description"],
        // Mastodon はメディア単位のセンシティブ指定を持たない。投稿の
        // `sensitive` を各タイルへ配る(呼び出し側で足す)。
    })
}

fn poll_wire(poll: &Value) -> Value {
    // ホスト契約の `voted` は「閲覧者が投票した選択肢の番号の並び」
    // (`votedIndices` になる)。Mastodon の `voted` は真偽値なので、番号は
    // `own_votes` から取る。未投票・未ログイン(`own_votes` 無し)は空の並び。
    let voted: Vec<Value> = if poll["voted"].as_bool() == Some(false) {
        Vec::new()
    } else {
        as_list(poll["own_votes"].clone()).into_iter().filter(Value::is_u64).collect()
    };
    json!({
        "options": as_list(poll["options"].clone())
            .iter()
            .map(|o| json!({ "text": o["title"], "votes": o["votes_count"] }))
            .collect::<Vec<Value>>(),
        "multiple": poll["multiple"].as_bool().unwrap_or(false),
        "expiresAt": poll["expires_at"],
        "voted": voted,
    })
}

/// Mastodon Status → `rasq.post/1` の Post ワイヤ。
///
/// **ブーストの展開 (D-100/D-112)**: Mastodon のブーストは `content` が
/// 空で `reblog` に中身が入る。そのまま返すと「内容のない投稿」になるので、
/// Misskey の純リノートと同じ規則で中身へ展開し、`repostedBy` に外側の
/// 投稿者、`repostTargetId` に中身の id を載せる。ホストはアクションと
/// スレッドをこの id へ読み替える。
fn post_wire(status: &Value, fallback_instance: &str) -> Value {
    post_wire_at(status, fallback_instance, 0)
}

fn post_wire_at(status: &Value, fallback_instance: &str, depth: usize) -> Value {
    // ブーストの展開は 1 段だけ(Mastodon も入れ子ブーストは作らない)。
    if depth == 0 && status["reblog"].is_object() {
        let inner = &status["reblog"];
        let mut wire = post_wire_at(inner, fallback_instance, depth + 1);
        wire["id"] = status["id"].clone();
        wire["createdAt"] = status["created_at"].clone();
        wire["repostedBy"] = user_wire(&status["account"], fallback_instance);
        wire["repostTargetId"] = inner["id"].clone();
        return wire;
    }

    let sensitive = status["sensitive"].as_bool().unwrap_or(false);
    let media: Vec<Value> = as_list(status["media_attachments"].clone())
        .iter()
        .map(|m| {
            let mut w = media_wire(m);
            w["sensitive"] = json!(sensitive);
            w
        })
        .collect();

    let mut wire = json!({
        "id": status["id"],
        "author": user_wire(&status["account"], fallback_instance),
        "createdAt": status["created_at"],
        "body": html_to_text(status["content"].as_str().unwrap_or("")),
        // HTML はここで潰したので、ホストから見ればただの平文。
        "bodyFormat": "plain",
        "media": media,
        "counts": {
            "reply": status["replies_count"],
            "repost": status["reblogs_count"],
            // お気に入りは「リアクション」欄に出す (このプラグインの規約)。
            "reaction": status["favourites_count"],
        },
        "visibility": status["visibility"],
        "url": status["url"],
        "replyToId": status["in_reply_to_id"],
    });

    if let Some(cw) = status["spoiler_text"].as_str() {
        if !cw.is_empty() {
            wire["cw"] = json!(cw);
        }
    }
    if status["poll"].is_object() {
        wire["poll"] = poll_wire(&status["poll"]);
    }
    // お気に入り済みは「⭐ でリアクション済み」として見せる。
    if status["favourited"].as_bool().unwrap_or(false) {
        wire["myReaction"] = json!("⭐");
    }
    // カスタム絵文字の URL(`:shortcode:` の描画にホストが使う)。
    let emojis: serde_json::Map<String, Value> = as_list(status["emojis"].clone())
        .iter()
        .filter_map(|e| {
            let name = e["shortcode"].as_str()?;
            Some((format!(":{name}:"), e["url"].clone()))
        })
        .collect();
    if !emojis.is_empty() {
        wire["reactionEmojis"] = Value::Object(emojis);
    }
    wire
}

/// Mastodon Account → Profile ワイヤ (`sns.user`)。
fn profile_wire(account: &Value, host: &str) -> Value {
    let acct = account["acct"].as_str().unwrap_or("?");
    let (handle, instance) = split_acct(acct, host);
    json!({
        "user": user_wire(account, host),
        "bannerUrl": account["header"],
        "description": account["note"].as_str().map(html_to_text),
        "followersCount": account["followers_count"],
        "followingCount": account["following_count"],
        "postsCount": account["statuses_count"],
        "createdAt": account["created_at"],
        "isLocked": account["locked"],
        "isBot": account["bot"],
        "isSuspended": account["suspended"],
        "fields": as_list(account["fields"].clone())
            .iter()
            .map(|f| json!({ "name": f["name"], "value": f["value"].as_str().map(html_to_text) }))
            .collect::<Vec<Value>>(),
        "profileUrl": account["url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://{instance}/@{handle}")),
    })
}

// ── sns.* (PROTOCOL §4.3) ────────────────────────────────────────

/// `TimelineKind.id` (CONTRACT §6) → Mastodon のパス。
///
/// Mastodon には Misskey の「ソーシャル」に当たる TL が無い。`social` は
/// ホーム TL に倒す(タブは宣言側 [`sns_timelines`] で出さない)。
/// `featured` はトレンド投稿を当てる。
fn timeline_path(kind: &str) -> String {
    match kind {
        "local" => "/api/v1/timelines/public?local=true".into(),
        "global" => "/api/v1/timelines/public".into(),
        "featured" => "/api/v1/trends/statuses".into(),
        k if k.starts_with("list:") => format!("/api/v1/timelines/list/{}", urlencode(&k[5..])),
        k if k.starts_with("tag:") => format!("/api/v1/timelines/tag/{}", urlencode(&k[4..])),
        _ => "/api/v1/timelines/home".into(),
    }
}

/// `call("sns.inspectServer", {host})` (CONTRACT §2): サーバー検索 /
/// PDS 変更の画面が出す情報。**認証前に呼ばれる**ので token は無い。
///
/// SNS ごとの取り方はプラグインの領分なので境界関数にしてある — ホストが
/// 「Mastodon なら /api/v1/instance」を知る必要はない。v2 があればそちら
/// (4.0+)、無ければ v1 に落ちる。
fn sns_inspect_server(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    // v2 は {domain, title, description, version, thumbnail:{url},
    // registrations:{enabled}, contact:{account}}。
    if let Ok(v2) = get(args, "/api/v2/instance") {
        if v2.is_object() {
            return Ok(json!({ "server": {
                "host": v2["domain"].as_str().unwrap_or(&host),
                "name": v2["title"].as_str().unwrap_or(&host),
                "software": "mastodon",
                "version": v2["version"],
                "description": v2["description"],
                "iconUrl": v2["thumbnail"]["url"],
                "registrationOpen": v2["registrations"]["enabled"],
                "maintainer": v2["contact"]["account"]["acct"],
            } }));
        }
    }
    let v1 = get(args, "/api/v1/instance")?;
    Ok(json!({ "server": {
        "host": v1["uri"].as_str().unwrap_or(&host),
        "name": v1["title"].as_str().unwrap_or(&host),
        "software": "mastodon",
        "version": v1["version"],
        "description": v1["short_description"].as_str().or(v1["description"].as_str()).map(html_to_text),
        "iconUrl": v1["thumbnail"],
        "registrationOpen": v1["registrations"],
        "maintainer": v1["contact_account"]["acct"],
    } }))
}

/// `call("sns.capabilities")`: セッション単位の能力値。投稿の文字数上限は
/// インスタンス設定で変わる(既定 500)。
fn sns_capabilities(args: &Value) -> Result<Value, Value> {
    let max = get(args, "/api/v2/instance")
        .ok()
        .and_then(|i| i["configuration"]["statuses"]["max_characters"].as_u64())
        .unwrap_or(500);
    Ok(json!({ "maxPostLength": max }))
}

/// `call("sns.timelines")`: このアカウントで出せる基本 TL。
fn sns_timelines(_args: &Value) -> Result<Value, Value> {
    Ok(json!({ "timelines": [
        { "id": "home",   "label": "ホーム" },
        { "id": "local",  "label": "ローカル" },
        { "id": "global", "label": "連合" },
        { "id": "featured", "label": "トレンド" },
    ] }))
}

fn sns_timeline(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let kind = args["kind"].as_str().unwrap_or("home");
    let path = timeline_path(kind);
    let limit = args["limit"].as_u64().unwrap_or(20).min(40);
    // timeline_path が既にクエリを持つことがある(local=true)。
    let sep = if path.contains('?') { "&" } else { "?" };
    let q = query_string(&[
        ("limit", limit.to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let path = format!("{path}{}", q.replacen('?', sep, 1));
    let list = as_list(get(args, &path)?);
    let items: Vec<Value> = list.iter().map(|s| post_wire(s, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

/// `call("sns.thread", {id})`: status + context → Thread ワイヤ。
fn sns_thread(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let id = urlencode(args["id"].as_str().unwrap_or(""));
    let target = get(args, &format!("/api/v1/statuses/{id}"))?;
    let context = get(args, &format!("/api/v1/statuses/{id}/context"))?;
    // Mastodon の ancestors は既に古い順。
    let ancestors: Vec<Value> = as_list(context["ancestors"].clone()).iter().map(|s| post_wire(s, &host)).collect();
    let descendants: Vec<Value> = as_list(context["descendants"].clone()).iter().map(|s| post_wire(s, &host)).collect();
    Ok(json!({ "thread": { "post": post_wire(&target, &host), "ancestors": ancestors, "descendants": descendants } }))
}

/// アクションバーの宣言 (D-075)。Mastodon にリアクションは無いので、
/// お気に入りを `Reaction` ジャンルのボタンとして出す — 押した位置と
/// 意味が他 SNS と揃う(同ジャンルは同スロットに束ねられる)。
fn sns_actions(_args: &Value) -> Result<Value, Value> {
    Ok(json!({ "actions": [
        { "id": "reply",  "genreTag": "Reply",  "label": "返信",       "icon": "reply",        "confirmMode": "confirm", "fow": "interstitial", "countSource": "self" },
        { "id": "reblog", "genreTag": "Repost", "label": "ブースト",   "icon": "repeat",       "doneIcon": "repeat_on",   "countSource": "self" },
        { "id": "favourite", "genreTag": "Reaction", "label": "お気に入り", "icon": "star_border", "doneIcon": "star",    "countSource": "self" },
        { "id": "bookmark", "genreTag": "Bookmark", "label": "ブックマーク", "icon": "bookmark_border", "doneIcon": "bookmark", "countSource": "none" }
    ] }))
}

/// `call("sns.performAction", {actionId, postId, args})` (D-075)。
/// 押すたびにトグルする(`args.undo` があればそれを優先)。
fn sns_perform_action(args: &Value) -> Result<Value, Value> {
    let id = urlencode(args["postId"].as_str().unwrap_or(""));
    let action = args["actionId"].as_str().unwrap_or("");
    let inner = &args["args"];

    let (on_path, off_path, count_key) = match action {
        "reblog" => ("reblog", "unreblog", "reblogs_count"),
        "favourite" => ("favourite", "unfavourite", "favourites_count"),
        "bookmark" => ("bookmark", "unbookmark", ""),
        "reply" => {
            // 返信はコンポーザーを開くだけ(投稿は sns.createPost 経由)。
            return Ok(json!({ "result": { "done": false, "message": "compose" } }));
        }
        other => return Err(json!({ "code": "C01", "message": format!("unsupported action: {other}") })),
    };

    // 現在の状態を見てトグル方向を決める。undo が明示されていればそれに従う。
    let undo = match inner["undo"].as_bool() {
        Some(u) => u,
        None => {
            let current = get(args, &format!("/api/v1/statuses/{id}"))?;
            match action {
                "reblog" => current["reblogged"].as_bool().unwrap_or(false),
                "favourite" => current["favourited"].as_bool().unwrap_or(false),
                _ => current["bookmarked"].as_bool().unwrap_or(false),
            }
        }
    };
    let path = if undo { off_path } else { on_path };
    let updated = post(args, &format!("/api/v1/statuses/{id}/{path}"), json!({}))?;

    let done = !undo;
    let count = if count_key.is_empty() { Value::Null } else { updated[count_key].clone() };
    Ok(json!({ "result": { "done": done, "count": count } }))
}

/// `call("sns.createPost", {draft})` (D-076)。
fn sns_create_post(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let draft = &args["draft"];

    // ブーストは投稿ではなくアクション。ホストの NewPost が repostOfId を
    // 持って来たらブーストへ振り替える。
    if let Some(repost_of) = draft["repostOfId"].as_str() {
        let created = post(args, &format!("/api/v1/statuses/{}/reblog", urlencode(repost_of)), json!({}))?;
        return Ok(json!({ "post": post_wire(&created, &host) }));
    }

    let mut body = json!({ "status": draft["text"].as_str().unwrap_or("") });
    if let Some(cw) = draft["cw"].as_str() {
        if !cw.is_empty() {
            body["spoiler_text"] = json!(cw);
            body["sensitive"] = json!(true);
        }
    }
    if let Some(reply) = draft["replyToId"].as_str() {
        body["in_reply_to_id"] = json!(reply);
    }
    if let Some(v) = draft["visibility"].as_str() {
        // ホストの語彙 (public/home/followers/specified) を Mastodon へ。
        body["visibility"] = json!(match v {
            "followers" | "home" => "private",
            "specified" | "direct" => "direct",
            "unlisted" => "unlisted",
            _ => "public",
        });
    }
    if let Some(ids) = draft["mediaIds"].as_array() {
        if !ids.is_empty() {
            body["media_ids"] = json!(ids);
        }
    }
    if let Some(poll) = draft["poll"].as_object() {
        body["poll"] = json!({
            "options": poll.get("choices").cloned().unwrap_or(json!([])),
            "expires_in": poll.get("expiresAfterSec").and_then(Value::as_u64).unwrap_or(86400),
            "multiple": poll.get("multiple").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    let created = post(args, "/api/v1/statuses", body)?;
    Ok(json!({ "post": post_wire(&created, &host) }))
}

fn sns_me(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let me = get(args, "/api/v1/accounts/verify_credentials")?;
    Ok(json!({ "user": user_wire(&me, &host) }))
}

/// `call("sns.user", {id | handle+userHost?})` (D-076)。
fn sns_user(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let account = if let Some(id) = args["id"].as_str() {
        get(args, &format!("/api/v1/accounts/{}", urlencode(id)))?
    } else {
        // `acct` はローカルなら `name`、リモートなら `name@host`。
        let handle = args["handle"].as_str().unwrap_or("");
        let acct = match args["userHost"].as_str() {
            Some(h) if !h.is_empty() && h != host => format!("{handle}@{h}"),
            _ => handle.to_string(),
        };
        get(args, &format!("/api/v1/accounts/lookup?acct={}", urlencode(&acct)))?
    };
    Ok(json!({ "profile": profile_wire(&account, &host) }))
}

/// `call("sns.userPosts", {id, filter, limit, cursor?})` (D-076)。
/// 返信/引用の振り分け (D-069): Mastodon に引用は無いので `quotes` は空。
fn sns_user_posts(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let filter = args["filter"].as_str().unwrap_or("posts");
    if filter == "quotes" {
        return Ok(json!({ "page": { "items": [], "nextCursor": Value::Null } }));
    }
    let id = urlencode(args["id"].as_str().unwrap_or(""));
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
        ("exclude_replies", (filter == "posts").to_string()),
        ("only_media", (filter == "media").to_string()),
    ]);
    let list = as_list(get(args, &format!("/api/v1/accounts/{id}/statuses{q}"))?);
    // `replies` タブはサーバー側で絞れないので手元で残す。
    let items: Vec<Value> = list
        .iter()
        .filter(|s| filter != "replies" || !s["in_reply_to_id"].is_null())
        .map(|s| post_wire(s, &host))
        .collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

/// `call("sns.reactions", {postId, ...})`。
///
/// Mastodon にリアクションは無いので、**お気に入りを唯一のリアクション**
/// (⭐)として一覧に写す。こうするとホストのリアクション一覧画面が
/// そのまま「お気に入りした人」一覧になる。
fn sns_reactions(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let id = urlencode(args["postId"].as_str().unwrap_or(""));
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(30).min(80).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let list = as_list(get(args, &format!("/api/v1/statuses/{id}/favourited_by{q}"))?);
    let items: Vec<Value> = list
        .iter()
        .map(|u| json!({ "id": u["id"], "user": user_wire(u, &host), "reaction": "⭐" }))
        .collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

fn sns_reposters(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let id = urlencode(args["postId"].as_str().unwrap_or(""));
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(30).min(80).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let list = as_list(get(args, &format!("/api/v1/statuses/{id}/reblogged_by{q}"))?);
    let items: Vec<Value> = list.iter().map(|u| user_wire(u, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

fn sns_emojis(args: &Value) -> Result<Value, Value> {
    let list = as_list(get(args, "/api/v1/custom_emojis")?);
    let emojis: Vec<Value> = list
        .iter()
        .filter(|e| e["visible_in_picker"].as_bool().unwrap_or(true))
        .map(|e| json!({ "name": e["shortcode"], "url": e["url"], "category": e["category"] }))
        .collect();
    Ok(json!({ "emojis": emojis }))
}

/// Mastodon の通知種別 → RASQ の `NotificationKind` (PROTOCOL §4.3)。
fn notification_kind(t: &str) -> &'static str {
    match t {
        "favourite" => "reaction",
        "reblog" => "repost",
        "mention" => "mention",
        "follow" => "follow",
        "follow_request" => "followRequest",
        "poll" => "pollEnded",
        // 「フォロー中の人が投稿した」通知。投稿タイルとして出す。
        "status" | "update" => "quote",
        "admin.sign_up" | "admin.report" | "severed_relationships" => "system",
        _ => "other",
    }
}

fn notification_wire(n: &Value, host: &str) -> Value {
    let kind = notification_kind(n["type"].as_str().unwrap_or(""));
    let mut wire = json!({
        "id": n["id"],
        "kind": kind,
        "createdAt": n["created_at"],
        // Mastodon の通知に既読フラグは無い(既読管理はマーカー API)。
        "read": false,
        "actors": if n["account"].is_object() { json!([user_wire(&n["account"], host)]) } else { json!([]) },
        "post": if n["status"].is_object() { post_wire(&n["status"], host) } else { Value::Null },
    });
    if kind == "reaction" {
        // お気に入り = ⭐ リアクション(このプラグインの規約)。
        wire["reaction"] = json!("⭐");
        if let Some(uid) = n["account"]["id"].as_str() {
            wire["reactionsByActor"] = json!({ uid: "⭐" });
        }
    }
    wire
}

fn sns_notifications(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let list = as_list(get(args, &format!("/api/v1/notifications{q}"))?);
    let items: Vec<Value> = list.iter().map(|n| notification_wire(n, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

/// Mastodon Relationship → FollowState ワイヤ。
fn follow_state_wire(r: &Value) -> Value {
    json!({
        "following": r["following"].as_bool().unwrap_or(false),
        "followedBy": r["followed_by"].as_bool().unwrap_or(false),
        "requested": r["requested"].as_bool().unwrap_or(false),
        "muted": r["muting"].as_bool().unwrap_or(false),
        "blocked": r["blocking"].as_bool().unwrap_or(false),
        "blockedBy": r["blocked_by"].as_bool().unwrap_or(false),
    })
}

fn sns_relationships(args: &Value) -> Result<Value, Value> {
    let ids: Vec<String> = as_list(args["ids"].clone()).iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    if ids.is_empty() {
        return Ok(json!({ "states": {} }));
    }
    let q: String = ids.iter().map(|id| format!("id[]={}", urlencode(id))).collect::<Vec<_>>().join("&");
    let list = as_list(get(args, &format!("/api/v1/accounts/relationships?{q}"))?);
    let mut states = serde_json::Map::new();
    for r in list {
        if let Some(id) = r["id"].as_str() {
            states.insert(id.to_string(), follow_state_wire(&r));
        }
    }
    Ok(json!({ "states": states }))
}

/// `call("sns.setRelation", {id, relation, on})`。
/// Mastodon はどの操作でも Relationship を返してくれるので、そのまま写す
/// (鍵アカウントへのフォローは `requested: true` で返る)。
fn sns_set_relation(args: &Value) -> Result<Value, Value> {
    let id = urlencode(args["id"].as_str().unwrap_or(""));
    let on = args["on"].as_bool().unwrap_or(false);
    let verb = match (args["relation"].as_str().unwrap_or(""), on) {
        ("following", true) => "follow",
        ("following", false) => "unfollow",
        ("muted", true) => "mute",
        ("muted", false) => "unmute",
        ("blocked", true) => "block",
        ("blocked", false) => "unblock",
        (other, _) => return Err(json!({ "code": "C01", "message": format!("unknown relation {other:?}") })),
    };
    let relationship = post(args, &format!("/api/v1/accounts/{id}/{verb}"), json!({}))?;
    Ok(json!({ "state": follow_state_wire(&relationship) }))
}

fn sns_follows(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let id = urlencode(args["id"].as_str().unwrap_or(""));
    let direction = if args["direction"].as_str() == Some("following") { "following" } else { "followers" };
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(20).min(80).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let list = as_list(get(args, &format!("/api/v1/accounts/{id}/{direction}{q}"))?);
    let items: Vec<Value> = list.iter().map(|u| user_wire(u, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

fn sns_user_list(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let path = match args["kind"].as_str().unwrap_or("") {
        "muted" => "/api/v1/mutes",
        "blocked" => "/api/v1/blocks",
        "followRequests" => "/api/v1/follow_requests",
        other => return Err(json!({ "code": "C01", "message": format!("unknown user list {other:?}") })),
    };
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(30).min(80).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let list = as_list(get(args, &format!("{path}{q}"))?);
    let items: Vec<Value> = list.iter().map(|u| user_wire(u, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

fn sns_search(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let users = args["kind"].as_str() == Some("users");
    let q = query_string(&[
        ("q", args["q"].as_str().unwrap_or("").to_string()),
        ("type", if users { "accounts".into() } else { "statuses".into() }),
        ("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
        // 検索は既定で「自分が関わったもの」に絞られるので広げる。
        ("resolve", "true".into()),
    ]);
    let res = get(args, &format!("/api/v2/search{q}"))?;
    if users {
        let list = as_list(res["accounts"].clone());
        let items: Vec<Value> = list.iter().map(|u| user_wire(u, &host)).collect();
        return Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }));
    }
    let list = as_list(res["statuses"].clone());
    let items: Vec<Value> = list.iter().map(|s| post_wire(s, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

fn sns_trending_tags(args: &Value) -> Result<Value, Value> {
    let list = as_list(get(args, "/api/v1/trends/tags")?);
    let tags: Vec<Value> = list
        .iter()
        .map(|t| {
            // history は日毎の {uses, accounts}。直近の accounts を件数に使う。
            let count = t["history"].as_array().and_then(|h| h.first()).and_then(|d| d["accounts"].as_str()).and_then(|s| s.parse::<u64>().ok());
            json!({ "tag": t["name"], "count": count })
        })
        .collect();
    Ok(json!({ "tags": tags }))
}

fn sns_favorites(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    // 「ブックマーク」はホスト側では favorites 欄に出す (CONTRACT §5)。
    // Mastodon はお気に入りとブックマークが別物なので、保存の意味が近い
    // ブックマークを採る。
    let list = as_list(get(args, &format!("/api/v1/bookmarks{q}"))?);
    let items: Vec<Value> = list.iter().map(|s| post_wire(s, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

/// `call("sns.dmRooms")` (CONTRACT §7)。
///
/// Mastodon には会話 API があるので、Misskey サンプルのように限定公開
/// 投稿を手で束ねる必要がない — `conversations` がまさに「相手ごとに
/// 束ねた direct 投稿」を返す。
fn sns_dm_rooms(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let list = as_list(get(args, "/api/v1/conversations?limit=40")?);
    let rooms: Vec<Value> = list
        .iter()
        .filter_map(|c| {
            let peer = c["accounts"].as_array()?.first()?;
            Some(json!({
                "id": c["id"],
                "kind": "direct",
                "peer": user_wire(peer, &host),
                "lastPost": if c["last_status"].is_object() { post_wire(&c["last_status"], &host) } else { Value::Null },
                "unreadCount": if c["unread"].as_bool().unwrap_or(false) { 1 } else { 0 },
            }))
        })
        .collect();
    Ok(json!({ "rooms": rooms }))
}

/// `call("sns.sendDm", {peerId, draft})`。
///
/// Mastodon の direct 投稿は**本文に相手のメンションが要る**(メンションが
/// 無いと誰にも届かない)。ホストの NewPost には宛先の概念しか無いので、
/// ここで `@acct` を前置する。
fn sns_send_dm(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let peer_id = args["peerId"].as_str().unwrap_or("");
    let account = get(args, &format!("/api/v1/accounts/{}", urlencode(peer_id)))?;
    let acct = account["acct"].as_str().unwrap_or("");
    let text = args["draft"]["text"].as_str().unwrap_or("");
    let mention = format!("@{acct}");
    let status = if text.contains(&mention) { text.to_string() } else { format!("{mention} {text}") };

    let mut body = json!({ "status": status, "visibility": "direct" });
    if let Some(reply) = args["draft"]["replyToId"].as_str() {
        body["in_reply_to_id"] = json!(reply);
    }
    if let Some(ids) = args["draft"]["mediaIds"].as_array() {
        if !ids.is_empty() {
            body["media_ids"] = json!(ids);
        }
    }
    let created = post(args, "/api/v1/statuses", body)?;
    Ok(json!({ "post": post_wire(&created, &host) }))
}

// ── メディア / プロフィール / リスト (D-136) ───────────────────────

/// multipart/form-data の本体を組み立てる。`net.fetch` はバイト列を
/// `{b64}` で受けるので(host-fn schema)、組み立てはプラグインの仕事。
fn multipart_body(boundary: &str, fields: &[(&str, &str)], file: Option<(&str, &str, &str, &[u8])>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    if let Some((name, filename, mime, bytes)) = file {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n")
                .as_bytes(),
        );
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// `call("sns.uploadMedia", {bytesB64, filename, mimeType?, alt?})` → `{media}`。
/// Mastodon は `POST /api/v2/media`(multipart)。`sensitive` は投稿単位の
/// 属性なので、ここでは受け取っても捨てる — 投稿時に `sensitive` が付く。
fn sns_upload_media(args: &Value) -> Result<Value, Value> {
    use base64::Engine as _;
    let Some(b64) = args["bytesB64"].as_str() else {
        return Err(json!({ "code": "D01", "message": "bytesB64 required" }));
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| json!({ "code": "D01", "message": format!("bad bytesB64: {e}") }))?;

    let filename = args["filename"].as_str().unwrap_or("upload.bin");
    let mime = args["mimeType"].as_str().unwrap_or("application/octet-stream");
    let alt = args["alt"].as_str().unwrap_or("");
    let boundary = "----rasqUploadBoundary7MA4YWxkTrZu0gW";
    let mut fields: Vec<(&str, &str)> = vec![];
    if !alt.is_empty() {
        fields.push(("description", alt));
    }
    let body = multipart_body(boundary, &fields, Some(("file", filename, mime, &bytes)));

    let host = args["host"].as_str().unwrap_or("mastodon.social");
    let base = match args["baseUrl"].as_str() {
        Some(b) => b.trim_end_matches('/').to_string(),
        None => format!("https://{host}"),
    };
    let mut headers = json!({ "Content-Type": format!("multipart/form-data; boundary={boundary}") });
    if let Some(token) = args["token"].as_str() {
        headers["Authorization"] = json!(format!("Bearer {token}"));
    }
    let response = host_call(
        "net.fetch",
        json!({
            "method": "POST",
            "url": format!("{base}/api/v2/media"),
            "headers": headers,
            "body": { "b64": base64::engine::general_purpose::STANDARD.encode(&body) },
        }),
    );
    if response["ok"].as_bool() != Some(true) {
        return Err(response.get("error").cloned().unwrap_or(json!({ "code": "B01", "message": "net.fetch failed" })));
    }
    let status = response["value"]["status"].as_u64().unwrap_or(0);
    // 202 = 処理中(大きい動画)。id は返るのでそのまま使える。
    if !(200..300).contains(&status) {
        return Err(json!({ "code": status.to_string(), "message": format!("HTTP {status} from /api/v2/media") }));
    }
    let media: Value = serde_json::from_str(response["value"]["bodyText"].as_str().unwrap_or(""))
        .map_err(|e| json!({ "code": "D01", "message": format!("bad JSON from /api/v2/media: {e}") }))?;
    Ok(json!({ "media": media_wire(&media) }))
}

/// `call("sns.updateProfile", {update})` → `{profile}`。
/// Mastodon の `PATCH /accounts/update_credentials` は multipart 前提だが、
/// 画像を含まない項目は JSON でも通る。アバター/ヘッダーの差し替えは
/// メディア id ではなくファイルそのものを要求するため、v1 では扱わない。
fn sns_update_profile(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let u = &args["update"];
    let mut body = json!({});
    if let Some(v) = u["displayName"].as_str() {
        body["display_name"] = json!(v);
    }
    if let Some(v) = u["description"].as_str() {
        body["note"] = json!(v);
    }
    if let Some(fields) = u["fields"].as_array() {
        // fields_attributes[0][name] 形式は JSON でも配列で受け付けられる。
        body["fields_attributes"] = json!(fields
            .iter()
            .map(|f| json!({ "name": f["name"], "value": f["value"] }))
            .collect::<Vec<Value>>());
    }
    let account = fetch(args, "PATCH", "/api/v1/accounts/update_credentials", Some(body))?;
    Ok(json!({ "profile": profile_wire(&account, &host) }))
}

fn list_wire(list: &Value) -> Value {
    json!({
        "id": list["id"],
        "name": list["title"],
        // Mastodon のリストにメンバー id は含まれない(別途 accounts を引く)。
        "memberIds": [],
        "isPublic": false,
    })
}

fn sns_lists(args: &Value) -> Result<Value, Value> {
    let list = as_list(get(args, "/api/v1/lists")?);
    Ok(json!({ "lists": list.iter().map(list_wire).collect::<Vec<Value>>() }))
}

fn sns_list_members(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let id = urlencode(args["id"].as_str().unwrap_or(""));
    let list = as_list(get(args, &format!("/api/v1/lists/{id}/accounts?limit=80"))?);
    Ok(json!({ "users": list.iter().map(|u| user_wire(u, &host)).collect::<Vec<Value>>() }))
}

fn sns_update_list_member(args: &Value) -> Result<Value, Value> {
    let id = urlencode(args["listId"].as_str().unwrap_or(""));
    let user = args["userId"].as_str().unwrap_or("");
    let method = if args["add"].as_bool().unwrap_or(false) { "POST" } else { "DELETE" };
    fetch(args, method, &format!("/api/v1/lists/{id}/accounts"), Some(json!({ "account_ids": [user] }))).map(|_| json!({}))
}

/// `call("sns.users", {ids})` → `{profiles}`。Mastodon には一括取得が無いので
/// 1 件ずつ引く(ホストは相手のプロフィールを数件しか要求しない)。
fn sns_users(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let mut profiles = vec![];
    for id in as_list(args["ids"].clone()).iter().filter_map(|v| v.as_str()) {
        // 1 件失敗しても残りは返す(消えたアカウントで一覧全体を落とさない)。
        if let Ok(a) = get(args, &format!("/api/v1/accounts/{}", urlencode(id))) {
            profiles.push(profile_wire(&a, &host));
        }
    }
    Ok(json!({ "profiles": profiles }))
}

/// `call("sns.reactedPosts")` (D-103): 自分がリアクションした投稿。
/// Mastodon ではお気に入り(favourites)がそれにあたる。`sns.favorites`
/// (ホストのブックマーク欄)はブックマークを返すので、別物として分ける。
fn sns_reacted_posts(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("mastodon.social").to_string();
    let q = query_string(&[
        ("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()),
        ("max_id", args["cursor"].as_str().unwrap_or("").to_string()),
    ]);
    let list = as_list(get(args, &format!("/api/v1/favourites{q}"))?);
    let items: Vec<Value> = list.iter().map(|s| post_wire(s, &host)).collect();
    Ok(json!({ "page": { "items": items, "nextCursor": next_cursor(&list) } }))
}

// ── boundary の call ディスパッチ (PROTOCOL §3.2) ──────────────────

#[no_mangle]
pub extern "C" fn call(
    fn_ptr: u32,
    fn_len: u32,
    args_ptr: u32,
    args_len: u32,
    call_id_ptr: u32,
    call_id_len: u32,
) -> u64 {
    let fn_name = read_str(fn_ptr, fn_len);
    let args = read_str(args_ptr, args_len);
    let call_id = read_str(call_id_ptr, call_id_len);
    let args_value: Value = serde_json::from_str(&args).unwrap_or(Value::Null);

    let outcome: Result<Value, Value> = match fn_name.as_str() {
        "sns.inspectServer" => sns_inspect_server(&args_value),
        "sns.timelines" => sns_timelines(&args_value),
        "sns.capabilities" => sns_capabilities(&args_value),
        "sns.timeline" => sns_timeline(&args_value),
        "sns.thread" => sns_thread(&args_value),
        "sns.actions" => sns_actions(&args_value),
        "sns.performAction" => sns_perform_action(&args_value),
        "sns.createPost" => sns_create_post(&args_value),
        "sns.deletePost" => fetch(
            &args_value,
            "DELETE",
            &format!("/api/v1/statuses/{}", urlencode(args_value["id"].as_str().unwrap_or(""))),
            None,
        )
        .map(|_| json!({})),
        "sns.me" => sns_me(&args_value),
        "sns.user" => sns_user(&args_value),
        "sns.userPosts" => sns_user_posts(&args_value),
        "sns.reactions" => sns_reactions(&args_value),
        "sns.reposters" => sns_reposters(&args_value),
        "sns.emojis" => sns_emojis(&args_value),
        "sns.notifications" => sns_notifications(&args_value),
        "sns.markNotificationsRead" => post(&args_value, "/api/v1/markers", json!({})).map(|_| json!({})),
        "sns.relationships" => sns_relationships(&args_value),
        "sns.setRelation" => sns_set_relation(&args_value),
        "sns.follows" => sns_follows(&args_value),
        "sns.userList" => sns_user_list(&args_value),
        "sns.resolveFollowRequest" => {
            let verb = if args_value["accept"].as_bool().unwrap_or(false) { "authorize" } else { "reject" };
            let id = urlencode(args_value["id"].as_str().unwrap_or(""));
            post(&args_value, &format!("/api/v1/follow_requests/{id}/{verb}"), json!({})).map(|_| json!({}))
        }
        "sns.reportUser" => post(
            &args_value,
            "/api/v1/reports",
            json!({ "account_id": args_value["id"], "comment": args_value["comment"] }),
        )
        .map(|_| json!({})),
        "sns.search" => sns_search(&args_value),
        "sns.trendingTags" => sns_trending_tags(&args_value),
        "sns.favorites" => sns_favorites(&args_value),
        "sns.dmRooms" => sns_dm_rooms(&args_value),
        "sns.sendDm" => sns_send_dm(&args_value),
        // メディア / プロフィール / リスト (D-136)。
        "sns.uploadMedia" => sns_upload_media(&args_value),
        "sns.updateProfile" => sns_update_profile(&args_value),
        "sns.lists" => sns_lists(&args_value),
        "sns.listInfo" => get(&args_value, &format!("/api/v1/lists/{}", urlencode(args_value["id"].as_str().unwrap_or("")))).map(|l| json!({ "list": list_wire(&l) })),
        "sns.listMembers" => sns_list_members(&args_value),
        "sns.createList" => post(&args_value, "/api/v1/lists", json!({ "title": args_value["name"] })).map(|l| json!({ "list": list_wire(&l) })),
        "sns.updateListMember" => sns_update_list_member(&args_value),
        "sns.users" => sns_users(&args_value),
        "sns.reactedPosts" => sns_reacted_posts(&args_value),
        // クリップ (D-032) と Play (D-039) は Misskey 固有。未実装 = C01 で
        // 落ちるので、ホストが該当タブ/メニューを隠す。
        // 未対応は C01 — ホストが該当 UI を隠す (PROTOCOL §4.3)。
        // Mastodon に引用投稿はネイティブに存在しない。
        other => Err(json!({ "code": "C01", "message": format!("unsupported fn: {other}") })),
    };

    let mut msg = json!({
        "schema": "rasq.boundary/1",
        "scope": SCOPE,
        "type": "result",
        "callId": call_id,
        "ok": outcome.is_ok(),
    });
    match outcome {
        Ok(value) => msg["value"] = value,
        Err(error) => msg["error"] = error,
    }
    pack(msg.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_bodies_flatten_to_text_the_host_can_parse() {
        // 典型的な Mastodon の本文: 段落 + リンク + メンション。
        let html = "<p>打ち上げ成功！<br />次は <a href=\"https://example.test/tags/rocket\" class=\"mention hashtag\">#<span>rocket</span></a> です</p><p>おつかれさま</p>";
        let text = html_to_text(html);
        assert_eq!(text, "打ち上げ成功！\n次は #rocket です\n\nおつかれさま");
        // タグは 1 つも残らない。
        assert!(!text.contains('<'), "tags leaked: {text}");
    }

    #[test]
    fn html_entities_come_back_as_characters() {
        assert_eq!(html_to_text("<p>a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#39;</p>"), "a & b <c> \"d\" 'e'");
        assert_eq!(html_to_text("<p>&#x1F680; &#128640;</p>"), "🚀 🚀");
        // 知らない実体は壊さずそのまま残す。
        assert_eq!(html_to_text("<p>&unknownentity; ok</p>"), "&unknownentity; ok");
    }

    #[test]
    fn acct_splits_into_handle_and_instance() {
        assert_eq!(split_acct("alice", "mastodon.social"), ("alice", "mastodon.social"));
        assert_eq!(split_acct("bob@mstdn.jp", "mastodon.social"), ("bob", "mstdn.jp"));
    }

    /// `voted` は真偽値ではなく、投票した選択肢の番号の並び(ホストの
    /// `pollFrom` が `votedIndices` として読む)。真偽値を送ると閲覧者の票が
    /// 失われる(以前はタイムラインごと落ちた)。
    #[test]
    fn poll_voted_is_the_list_of_own_vote_indices() {
        let poll = |extra: Value| {
            let mut p = json!({
                "options": [
                    { "title": "A", "votes_count": 3 },
                    { "title": "B", "votes_count": 1 },
                    { "title": "C", "votes_count": 0 },
                ],
                "multiple": true,
                "expires_at": "2026-01-01T00:00:00.000Z",
            });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let w = poll_wire(&poll(json!({ "voted": true, "own_votes": [0, 2] })));
        assert_eq!(w["voted"], json!([0, 2]));
        assert_eq!(w["options"][0], json!({ "text": "A", "votes": 3 }));
        // 未投票は空の並び。
        assert_eq!(poll_wire(&poll(json!({ "voted": false, "own_votes": [] })))["voted"], json!([]));
        // 未ログインの取得(公開トレンド等)は `voted` / `own_votes` 自体が無い。
        assert_eq!(poll_wire(&poll(json!({})))["voted"], json!([]));
        // `voted: false` なのに番号が残っていても票として扱わない。
        assert_eq!(poll_wire(&poll(json!({ "voted": false, "own_votes": [1] })))["voted"], json!([]));
        // 投稿のワイヤにもそのまま載る。
        let status = json!({
            "id": "1", "created_at": "2025-01-01T00:00:00.000Z", "content": "<p>q</p>",
            "account": { "id": "9", "acct": "alice", "username": "alice" },
            "poll": poll(json!({ "voted": true, "own_votes": [1] })),
        });
        assert_eq!(post_wire(&status, "mastodon.social")["poll"]["voted"], json!([1]));
    }

    /// D-100/D-112: ブーストは中身へ展開し、外側の id と投稿者を載せる。
    /// 展開しないと本文の無い「内容のない投稿」になる。
    #[test]
    fn boost_unwraps_to_the_inner_status() {
        let status = json!({
            "id": "outer1",
            "content": "",
            "created_at": "2026-09-02T00:00:00.000Z",
            "account": { "id": "u2", "acct": "booster", "display_name": "ブーストした人" },
            "reblog": {
                "id": "inner1",
                "content": "<p>元の投稿</p>",
                "created_at": "2026-09-01T00:00:00.000Z",
                "account": { "id": "u1", "acct": "author", "display_name": "書いた人" },
                "media_attachments": [],
                "favourites_count": 3,
            },
        });
        let wire = post_wire(&status, "example.test");
        assert_eq!(wire["id"], "outer1", "アクションは外側の id へ");
        assert_eq!(wire["repostTargetId"], "inner1", "スレッド等は中身へ読み替える");
        assert_eq!(wire["body"], "元の投稿");
        assert_eq!(wire["author"]["displayName"], "書いた人");
        assert_eq!(wire["repostedBy"]["displayName"], "ブーストした人");
        assert_eq!(wire["counts"]["reaction"], 3);
    }

    #[test]
    fn plain_status_maps_media_cw_and_favourite_state() {
        let status = json!({
            "id": "s1",
            "content": "<p>本文</p>",
            "spoiler_text": "注意",
            "sensitive": true,
            "created_at": "2026-09-01T00:00:00.000Z",
            "account": { "id": "u1", "acct": "author@remote.test", "display_name": "遠くの人" },
            "media_attachments": [
                { "id": "m1", "type": "image", "url": "https://x.test/1.png", "preview_url": "https://x.test/1s.png", "description": "写真" },
                { "id": "m2", "type": "gifv", "url": "https://x.test/2.mp4" },
            ],
            "replies_count": 1,
            "reblogs_count": 2,
            "favourites_count": 3,
            "favourited": true,
            "visibility": "public",
            "emojis": [{ "shortcode": "party", "url": "https://x.test/party.png" }],
        });
        let wire = post_wire(&status, "example.test");
        assert_eq!(wire["body"], "本文");
        assert_eq!(wire["bodyFormat"], "plain");
        assert_eq!(wire["cw"], "注意");
        // 投稿の sensitive を各メディアタイルへ配る。
        assert_eq!(wire["media"][0]["sensitive"], true);
        assert_eq!(wire["media"][0]["type"], "image");
        assert_eq!(wire["media"][1]["type"], "gif");
        assert_eq!(wire["media"][0]["alt"], "写真");
        // リモートユーザーは acct から instance を割り出す。
        assert_eq!(wire["author"]["handle"], "author");
        assert_eq!(wire["author"]["instance"], "remote.test");
        // お気に入り = ⭐ リアクション済み。
        assert_eq!(wire["myReaction"], "⭐");
        assert_eq!(wire["reactionEmojis"][":party:"], "https://x.test/party.png");
    }

    #[test]
    fn timeline_ids_map_to_mastodon_paths() {
        assert_eq!(timeline_path("home"), "/api/v1/timelines/home");
        assert_eq!(timeline_path("local"), "/api/v1/timelines/public?local=true");
        assert_eq!(timeline_path("global"), "/api/v1/timelines/public");
        assert_eq!(timeline_path("featured"), "/api/v1/trends/statuses");
        assert_eq!(timeline_path("list:42"), "/api/v1/timelines/list/42");
        assert_eq!(timeline_path("tag:ロケット"), "/api/v1/timelines/tag/%E3%83%AD%E3%82%B1%E3%83%83%E3%83%88");
        // Mastodon に「ソーシャル」は無いのでホームへ倒す。
        assert_eq!(timeline_path("social"), "/api/v1/timelines/home");
    }

    #[test]
    fn notification_types_map_onto_rasq_kinds() {
        assert_eq!(notification_kind("favourite"), "reaction");
        assert_eq!(notification_kind("reblog"), "repost");
        assert_eq!(notification_kind("follow_request"), "followRequest");
        assert_eq!(notification_kind("admin.sign_up"), "system");
        assert_eq!(notification_kind("brand_new"), "other");
    }

    #[test]
    fn favourite_notification_carries_the_star_reaction() {
        let n = json!({
            "id": "n1",
            "type": "favourite",
            "created_at": "2026-09-01T00:00:00.000Z",
            "account": { "id": "u1", "acct": "fan", "display_name": "ファン" },
            "status": { "id": "s1", "content": "<p>本文</p>", "account": { "id": "me", "acct": "me" }, "media_attachments": [] },
        });
        let wire = notification_wire(&n, "example.test");
        assert_eq!(wire["kind"], "reaction");
        assert_eq!(wire["reaction"], "⭐");
        assert_eq!(wire["reactionsByActor"]["u1"], "⭐");
        assert_eq!(wire["post"]["body"], "本文");
    }

    #[test]
    fn relationship_maps_onto_follow_state() {
        let r = json!({ "id": "u1", "following": false, "requested": true, "muting": false, "blocked_by": true });
        let wire = follow_state_wire(&r);
        assert_eq!(wire["following"], false);
        assert_eq!(wire["requested"], true, "鍵アカウントへのフォローは申請止まり");
        assert_eq!(wire["blockedBy"], true);
        // 欠けたフラグは false。
        assert_eq!(wire["blocked"], false);
    }

    #[test]
    fn query_strings_encode_and_drop_empty_values() {
        assert_eq!(query_string(&[("limit", "20".into()), ("max_id", String::new())]), "?limit=20");
        assert_eq!(query_string(&[("q", "a b&c".into())]), "?q=a%20b%26c");
        assert_eq!(query_string(&[("a", String::new())]), "");
    }

    #[test]
    fn cursor_is_the_last_id() {
        let list = vec![json!({ "id": "1" }), json!({ "id": "2" })];
        assert_eq!(next_cursor(&list), json!("2"));
        assert_eq!(next_cursor(&[]), Value::Null);
    }

    /// multipart は境界に専用の口を作らず、プラグインが本体を組み立てて
    /// `net.fetch` の `{b64}` に載せる。組み立てを間違えるとサーバー側で
    /// 黙って弾かれるので、バイト列を直接固定しておく。
    #[test]
    fn multipart_body_frames_fields_and_the_file() {
        let body = multipart_body("BOUND", &[("i", "tok"), ("name", "a.png")], Some(("file", "a.png", "image/png", &[0u8, 1, 2])));
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("--BOUND\r\nContent-Disposition: form-data; name=\"i\"\r\n\r\ntok\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n"));
        assert!(text.ends_with("--BOUND--\r\n"));
        // 生バイトがそのまま入っていること(UTF-8 に潰れていない)。
        let needle = b"image/png\r\n\r\n\x00\x01\x02\r\n";
        assert!(
            body.windows(needle.len()).any(|w| w == needle),
            "raw bytes were not embedded verbatim"
        );
    }

    /// ファイル無し(フィールドだけ)でも壊れない。
    #[test]
    fn multipart_body_without_a_file_is_still_terminated() {
        let body = multipart_body("B", &[("a", "1")], None);
        assert_eq!(String::from_utf8_lossy(&body), "--B\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--B--\r\n");
    }
}
