//! mixi2 WASM plugin **sample** for RASQ (PROTOCOL.md §4.3 の `sns.*` 境界、D-206)。
//!
//! mixi2 の公式 API は **申請制で、OAuth の認可サーバー(RASQ 側のブローカー)も要る**。
//! どちらも揃うまで実通信は検証できないので、このサンプルは 2 つのモードを持つ:
//!
//! - **ダミーモード** (`host == "dummy"` / トークンが `dummy:` で始まる): ネットワークを使わず、
//!   プラグイン内のメモリに持った投稿 / いいね / リポスト / 通知で `sns.*` を全部こなす。
//!   ホスト側のログイン → TL → アクション → 投稿 → 通知の流れをこれで確認する。
//! - **実モード**(それ以外のホスト): `net.fetch` で `https://<host>/api/v1/...` を叩く。
//!   エンドポイントと JSON の形は **未検証の仮置き**(Mastodon 互換の REST を仮定)。
//!   API 申請が通ったら [`api`] の定数とワイヤ変換([`user_wire`] / [`post_wire`])を実物に合わせる。
//!   認証はホストのブローカー(`/auth/sns/mixi2/begin`)が発行した Bearer トークンを `args.token` で受け取る。
//!
//! ワイヤ(`rasq.post/1`)は Mastodon / Bluesky サンプルと同じ形。アクションは 返信 / リポスト /
//! いいね(⭐ 固定の `Reaction`)/ 引用。

use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};

mod settings;
mod text;

use rasq_plugin_kit::i18n;
use text::TEXT;

const SCOPE: &str = "rasq.official.mixi2:mixi2:mixi.social:sample:0.1.0:sns";
const KIND: &str = "mixi2";
const DEFAULT_HOST: &str = "mixi.social";
/// ダミーモードのホスト名。ログイン画面の「ダミーアカウントで試す」が渡す。
pub const DUMMY_HOST: &str = "dummy";
const STAR: &str = "⭐";
const MAX_POST_LENGTH: usize = 149;
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

/// 実モードのエンドポイント(未検証の仮置き)。
mod api {
    pub const ME: &str = "/api/v1/accounts/verify_credentials";
    pub const HOME: &str = "/api/v1/timelines/home";
    pub const LOCAL: &str = "/api/v1/timelines/public";
    pub const STATUSES: &str = "/api/v1/statuses";
    pub const ACCOUNTS: &str = "/api/v1/accounts";
    pub const NOTIFICATIONS: &str = "/api/v1/notifications";
    pub const SEARCH: &str = "/api/v2/search";
}

#[link(wasm_import_module = "rasq")]
extern "C" {
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

// ── wasm ABI (PROTOCOL §4.1) ──────────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn host_call(fn_name: &str, args: Value) -> Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
    let bytes = request.as_bytes();
    let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
    let ptr = (packed >> 32) as u32;
    let len = (packed & 0xFFFF_FFFF) as u32;
    let response = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
    serde_json::from_slice(&response).unwrap_or(Value::Null)
}

/// ネイティブのテストではホスト関数が無い(ダミーモードだけを通す)。
#[cfg(not(target_arch = "wasm32"))]
fn host_call(_fn_name: &str, _args: Value) -> Value {
    let _ = HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed);
    json!({ "ok": false, "error": { "code": "B01", "message": "no host in native tests" } })
}

fn pack(mut buf: Vec<u8>) -> u64 {
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

#[no_mangle]
pub extern "C" fn snapshot() -> u64 {
    let msg = json!({ "schema": "rasq.boundary/1", "scope": SCOPE, "type": "snapshot", "rev": 1, "root": { "t": "Column", "k": "root", "c": [] } });
    pack(msg.to_string().into_bytes())
}

#[no_mangle]
pub extern "C" fn patch() -> u64 {
    pack(Value::Null.to_string().into_bytes())
}

// ── 共通 ──────────────────────────────────────────────────────────

fn err(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn str_or<'a>(v: &'a Value, d: &'a str) -> &'a str {
    v.as_str().unwrap_or(d)
}

fn host_of(args: &Value) -> String {
    str_or(&args["host"], DEFAULT_HOST).to_string()
}

fn is_dummy(args: &Value) -> bool {
    str_or(&args["host"], "") == DUMMY_HOST || str_or(&args["token"], "").starts_with("dummy:")
}

fn now_of(args: &Value) -> String {
    str_or(&args["now"], EPOCH).to_string()
}

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

fn query_string(params: &[(&str, String)]) -> String {
    let pairs: Vec<String> = params.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

fn as_list(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        _ => Vec::new(),
    }
}

fn page(items: Vec<Value>, next: Value) -> Value {
    json!({ "page": { "items": items, "nextCursor": next } })
}

// ── 実モード: REST(未検証の仮置き)──────────────────────────────

fn fetch(args: &Value, method: &str, path: &str, body: Option<Value>) -> Result<Value, Value> {
    let base = match args["baseUrl"].as_str() {
        Some(b) => b.trim_end_matches('/').to_string(),
        None => format!("https://{}", host_of(args)),
    };
    let mut headers = json!({ "Accept": "application/json" });
    if let Some(token) = args["token"].as_str() {
        headers["Authorization"] = json!(format!("Bearer {token}"));
    }
    let mut request = json!({ "method": method, "url": format!("{base}{path}"), "headers": headers });
    if let Some(b) = body {
        request["headers"]["Content-Type"] = json!("application/json");
        request["body"] = json!(b.to_string());
    }
    let response = host_call("net.fetch", request);
    if response["ok"].as_bool() != Some(true) {
        return Err(response.get("error").cloned().unwrap_or(err("B01", "net.fetch failed")));
    }
    let value = &response["value"];
    let status = value["status"].as_u64().unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(err(&status.to_string(), format!("HTTP {status} from {path}")));
    }
    let text = value["bodyText"].as_str().unwrap_or("");
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).map_err(|e| err("D01", format!("bad JSON from {path}: {e}")))
}

fn get(args: &Value, path: &str) -> Result<Value, Value> {
    fetch(args, "GET", path, None)
}

fn post(args: &Value, path: &str, body: Value) -> Result<Value, Value> {
    fetch(args, "POST", path, Some(body))
}

fn next_cursor(list: &[Value]) -> Value {
    match list.last().and_then(|v| v["id"].as_str()) {
        Some(id) => json!(id),
        None => Value::Null,
    }
}

/// 実モードのユーザー → user ワイヤ(仮置き: Mastodon 互換の account を仮定)。
fn user_wire(account: &Value, host: &str) -> Value {
    let handle = str_or(&account["acct"], str_or(&account["username"], "?"));
    let (handle, instance) = match handle.split_once('@') {
        Some((h, i)) => (h.to_string(), i.to_string()),
        None => (handle.to_string(), host.to_string()),
    };
    let display = match account["display_name"].as_str() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => handle.clone(),
    };
    json!({ "id": account["id"], "kind": KIND, "instance": instance, "handle": handle, "displayName": display, "avatarUrl": account["avatar"], "bio": account["note"] })
}

/// 実モードの投稿 → post ワイヤ(仮置き)。
fn post_wire(status: &Value, host: &str) -> Value {
    if status["reblog"].is_object() {
        let inner = &status["reblog"];
        let mut wire = post_wire(inner, host);
        wire["id"] = status["id"].clone();
        wire["createdAt"] = status["created_at"].clone();
        wire["repostedBy"] = user_wire(&status["account"], host);
        wire["repostTargetId"] = inner["id"].clone();
        return wire;
    }
    let mut wire = json!({
        "id": status["id"],
        "author": user_wire(&status["account"], host),
        "createdAt": status["created_at"],
        "body": status["text"].as_str().or(status["content"].as_str()).unwrap_or(""),
        "bodyFormat": "plain",
        "media": as_list(status["media_attachments"].clone()).iter().map(|m| json!({ "id": m["id"], "type": if str_or(&m["type"], "image") == "video" { "video" } else { "image" }, "url": m["url"], "thumbnailUrl": m["preview_url"] })).collect::<Vec<_>>(),
        "counts": { "reply": status["replies_count"], "repost": status["reblogs_count"], "reaction": status["favourites_count"] },
        "visibility": status["visibility"],
        "url": status["url"],
        "replyToId": status["in_reply_to_id"],
    });
    if status["favourited"].as_bool().unwrap_or(false) {
        wire["myReaction"] = json!(STAR);
    }
    wire
}

// ── ダミーモード ────────────────────────────────────────────────

struct Dummy {
    posts: Vec<Value>,
    liked: HashSet<String>,
    reposted: HashSet<String>,
    notifications: Vec<Value>,
    seq: u64,
}

thread_local! {
    static DUMMY: RefCell<Option<Dummy>> = const { RefCell::new(None) };
}

fn dummy_user(id: &str, handle: &str, name: &str) -> Value {
    json!({ "id": id, "kind": KIND, "instance": DUMMY_HOST, "handle": handle, "displayName": name, "avatarUrl": Value::Null, "bio": format!("{name}(ダミー)") })
}

fn dummy_me() -> Value {
    dummy_user("u-me", "me", "わたし(ダミー)")
}

fn dummy_post(id: &str, user: Value, at: &str, body: &str) -> Value {
    json!({ "id": id, "author": user, "createdAt": at, "body": body, "bodyFormat": "plain", "media": [], "counts": { "reply": 0, "repost": 0, "reaction": 0 }, "visibility": "public", "url": format!("https://{DUMMY_HOST}/posts/{id}") })
}

fn dummy_seed() -> Dummy {
    let alice = dummy_user("u-alice", "alice", "アリス");
    let bob = dummy_user("u-bob", "bob", "ボブ");
    let posts = vec![
        dummy_post("d-8", alice.clone(), "2026-09-13T09:00:00.000Z", "mixi2 サンプルのダミー投稿です。API 申請が通ったら本物に差し替えます #rasq"),
        dummy_post("d-7", bob.clone(), "2026-09-13T08:30:00.000Z", "おはよう。今日は天気がいい"),
        dummy_post("d-6", alice.clone(), "2026-09-13T08:00:00.000Z", "いいねとリポストはプラグインの中で数えています"),
        dummy_post("d-5", bob.clone(), "2026-09-13T07:30:00.000Z", "返信するとスレッドに並びます"),
        dummy_post("d-4", alice.clone(), "2026-09-13T07:00:00.000Z", "検索は本文の部分一致"),
        dummy_post("d-3", bob.clone(), "2026-09-13T06:30:00.000Z", "通知はいいね / リポスト / 返信で増えます"),
        dummy_post("d-2", alice, "2026-09-13T06:00:00.000Z", "ここまで読んだらログイン完了です"),
        dummy_post("d-1", bob, "2026-09-13T05:30:00.000Z", "最初の投稿"),
    ];
    Dummy { posts, liked: HashSet::new(), reposted: HashSet::new(), notifications: vec![json!({ "id": "n-1", "kind": "follow", "createdAt": "2026-09-13T09:30:00.000Z", "read": false, "actors": [dummy_user("u-alice", "alice", "アリス")], "post": Value::Null })], seq: 100 }
}

fn with_dummy<T>(f: impl FnOnce(&mut Dummy) -> T) -> T {
    DUMMY.with(|d| {
        let mut d = d.borrow_mut();
        if d.is_none() {
            *d = Some(dummy_seed());
        }
        f(d.as_mut().unwrap())
    })
}

/// ダミーの投稿を「いま」の状態(いいね / リポスト済み)で写す。
fn dummy_view(d: &Dummy, p: &Value) -> Value {
    let mut w = p.clone();
    let id = str_or(&p["id"], "").to_string();
    if d.liked.contains(&id) {
        w["myReaction"] = json!(STAR);
    }
    if d.reposted.contains(&id) {
        w["myRepost"] = json!(true);
    }
    w
}

fn dummy_call(fn_name: &str, args: &Value) -> Result<Value, Value> {
    match fn_name {
        "sns.login" => Ok(json!({ "session": { "token": format!("dummy:{}", str_or(&args["identifier"], "me")), "handle": "me" }, "me": dummy_me() })),
        "sns.inspectServer" => Ok(json!({ "server": { "host": DUMMY_HOST, "name": "mixi2 (ダミー)", "software": "mixi2", "registrationOpen": false, "description": "API 申請前の動作確認用" } })),
        "sns.me" => Ok(json!({ "user": dummy_me() })),
        "sns.timeline" => with_dummy(|d| {
            let limit = args["limit"].as_u64().unwrap_or(20) as usize;
            let start = args["cursor"].as_str().and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
            let items: Vec<Value> = d.posts.iter().skip(start).take(limit).map(|p| dummy_view(d, p)).collect();
            let next = if start + limit < d.posts.len() { json!((start + limit).to_string()) } else { Value::Null };
            Ok(page(items, next))
        }),
        "sns.post" => with_dummy(|d| {
            let id = str_or(&args["id"], "");
            d.posts.iter().find(|p| p["id"] == id).map(|p| json!({ "post": dummy_view(d, p) })).ok_or_else(|| err("404", "no such post"))
        }),
        "sns.thread" => with_dummy(|d| {
            let id = str_or(&args["id"], "").to_string();
            let target = d.posts.iter().find(|p| p["id"] == id).cloned().ok_or_else(|| err("404", "no such post"))?;
            let mut ancestors = Vec::new();
            let mut cur = target.clone();
            while let Some(parent) = cur["replyToId"].as_str().and_then(|pid| d.posts.iter().find(|p| p["id"] == pid)).cloned() {
                ancestors.insert(0, dummy_view(d, &parent));
                cur = parent;
            }
            let descendants: Vec<Value> = d.posts.iter().rev().filter(|p| p["replyToId"] == id).map(|p| dummy_view(d, p)).collect();
            Ok(json!({ "thread": { "post": dummy_view(d, &target), "ancestors": ancestors, "descendants": descendants } }))
        }),
        "sns.performAction" => with_dummy(|d| {
            let id = str_or(&args["postId"], "").to_string();
            let action = str_or(&args["actionId"], "");
            let idx = d.posts.iter().position(|p| p["id"] == id).ok_or_else(|| err("404", "no such post"))?;
            let now = now_of(args);
            let (set, key) = match action {
                "favorite" => (&mut d.liked, "reaction"),
                "repost" => (&mut d.reposted, "repost"),
                "reply" | "quote" => return Ok(json!({ "result": { "done": false, "message": "compose" } })),
                other => return Err(err("C01", format!("unsupported action: {other}"))),
            };
            let undo = args["args"]["undo"].as_bool().unwrap_or_else(|| set.contains(&id));
            let n = d.posts[idx]["counts"][key].as_i64().unwrap_or(0);
            if undo {
                set.remove(&id);
                d.posts[idx]["counts"][key] = json!((n - 1).max(0));
            } else {
                set.insert(id.clone());
                d.posts[idx]["counts"][key] = json!(n + 1);
                d.seq += 1;
                d.notifications.insert(0, json!({ "id": format!("n-{}", d.seq), "kind": if key == "repost" { "repost" } else { "reaction" }, "createdAt": now, "read": false, "actors": [dummy_me()], "post": d.posts[idx].clone(), "reaction": if key == "repost" { Value::Null } else { json!(STAR) } }));
            }
            Ok(json!({ "result": { "done": !undo, "count": d.posts[idx]["counts"][key] } }))
        }),
        "sns.createPost" => with_dummy(|d| {
            let draft = &args["draft"];
            let text = str_or(&draft["text"], "").to_string();
            empty_post(args, &text)?;
            if text.chars().count() > MAX_POST_LENGTH {
                return Err(err("D01", TEXT.f(i18n::locale_of(args), "error.postTooLong", &[("max", &MAX_POST_LENGTH.to_string())])));
            }
            d.seq += 1;
            let id = format!("d-{}", d.seq);
            let mut p = dummy_post(&id, dummy_me(), &now_of(args), &text);
            if let Some(r) = draft["replyToId"].as_str() {
                p["replyToId"] = json!(r);
                if let Some(parent) = d.posts.iter_mut().find(|q| q["id"] == r) {
                    let n = parent["counts"]["reply"].as_i64().unwrap_or(0);
                    parent["counts"]["reply"] = json!(n + 1);
                }
            }
            if let Some(q) = draft["quoteOfId"].as_str().or(draft["quoteId"].as_str()) {
                if let Some(quoted) = d.posts.iter().find(|x| x["id"] == q).cloned() {
                    p["quote"] = quoted;
                }
            }
            if let Some(v) = draft["visibility"].as_str() {
                p["visibility"] = json!(v);
            }
            d.posts.insert(0, p.clone());
            Ok(json!({ "post": p }))
        }),
        "sns.deletePost" => with_dummy(|d| {
            let id = str_or(&args["id"], "");
            d.posts.retain(|p| p["id"] != id);
            Ok(json!({}))
        }),
        "sns.user" => {
            let id = str_or(&args["id"], str_or(&args["handle"], "me"));
            let u = with_dummy(|d| d.posts.iter().map(|p| p["author"].clone()).find(|a| a["id"] == id || a["handle"] == id)).unwrap_or_else(dummy_me);
            let count = with_dummy(|d| d.posts.iter().filter(|p| p["author"]["id"] == u["id"]).count());
            Ok(json!({ "profile": { "user": u, "description": "ダミーのプロフィール", "followersCount": 3, "followingCount": 2, "postsCount": count, "createdAt": "2026-01-01T00:00:00.000Z" } }))
        }
        "sns.userPosts" => with_dummy(|d| {
            let id = str_or(&args["id"], "");
            let filter = str_or(&args["filter"], "posts");
            let items: Vec<Value> = d.posts.iter().filter(|p| p["author"]["id"] == id).filter(|p| match filter { "replies" => p["replyToId"].is_string(), "quotes" => p["quote"].is_object(), "media" => !as_list(p["media"].clone()).is_empty(), _ => !p["replyToId"].is_string() }).map(|p| dummy_view(d, p)).collect();
            Ok(page(items, Value::Null))
        }),
        "sns.reactions" => with_dummy(|d| {
            let id = str_or(&args["postId"], "");
            let items: Vec<Value> = if d.liked.contains(id) { vec![json!({ "id": "u-me", "user": dummy_me(), "reaction": STAR })] } else { vec![] };
            Ok(page(items, Value::Null))
        }),
        "sns.reposters" => with_dummy(|d| {
            let id = str_or(&args["postId"], "");
            let items: Vec<Value> = if d.reposted.contains(id) { vec![dummy_me()] } else { vec![] };
            Ok(page(items, Value::Null))
        }),
        "sns.reactedPosts" => with_dummy(|d| Ok(page(d.posts.iter().filter(|p| d.liked.contains(str_or(&p["id"], ""))).map(|p| dummy_view(d, p)).collect(), Value::Null))),
        "sns.notifications" => with_dummy(|d| Ok(page(d.notifications.clone(), Value::Null))),
        "sns.markNotificationsRead" => with_dummy(|d| {
            for n in d.notifications.iter_mut() {
                n["read"] = json!(true);
            }
            Ok(json!({}))
        }),
        "sns.search" => with_dummy(|d| {
            let q = str_or(&args["q"], "").to_lowercase();
            if str_or(&args["kind"], "posts") == "users" {
                let mut seen = HashSet::new();
                let items: Vec<Value> = d.posts.iter().map(|p| p["author"].clone()).filter(|a| seen.insert(str_or(&a["id"], "").to_string())).filter(|a| str_or(&a["displayName"], "").to_lowercase().contains(&q) || str_or(&a["handle"], "").to_lowercase().contains(&q)).collect();
                return Ok(page(items, Value::Null));
            }
            let items: Vec<Value> = d.posts.iter().filter(|p| str_or(&p["body"], "").to_lowercase().contains(&q)).map(|p| dummy_view(d, p)).collect();
            Ok(page(items, Value::Null))
        }),
        "sns.users" => with_dummy(|d| {
            let ids: Vec<String> = as_list(args["ids"].clone()).iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
            let users: Vec<Value> = ids.iter().filter_map(|id| d.posts.iter().map(|p| p["author"].clone()).find(|a| a["id"] == id.as_str())).collect();
            Ok(json!({ "users": users }))
        }),
        "sns.relationships" => {
            let mut states = Map::new();
            for id in as_list(args["ids"].clone()) {
                if let Some(id) = id.as_str() {
                    states.insert(id.to_string(), json!({ "following": id != "u-me", "followedBy": id == "u-alice", "requested": false, "muted": false, "blocked": false, "blockedBy": false }));
                }
            }
            Ok(json!({ "states": states }))
        }
        "sns.setRelation" => Ok(json!({ "state": { "following": args["on"].as_bool().unwrap_or(false), "followedBy": false, "requested": false, "muted": false, "blocked": false, "blockedBy": false } })),
        "sns.trendingTags" => Ok(json!({ "tags": [{ "tag": "rasq", "count": 3 }, { "tag": "mixi2", "count": 2 }] })),
        other => Err(err("C01", format!("unsupported fn (dummy): {other}"))),
    }
}

// ── 実モード ────────────────────────────────────────────────────

fn real_call(fn_name: &str, args: &Value) -> Result<Value, Value> {
    let host = host_of(args);
    match fn_name {
        // 実モードのログインはブローカー(OAuth)の担当。資格情報では入れない。
        "sns.login" => Err(err("A01", TEXT.t(i18n::locale_of(args), "error.loginOauthOnly"))),
        "sns.inspectServer" => Ok(json!({ "server": { "host": host, "name": host, "software": "mixi2", "registrationOpen": false } })),
        "sns.me" => Ok(json!({ "user": user_wire(&get(args, api::ME)?, &host) })),
        "sns.timeline" => {
            let path = match str_or(&args["kind"], "home") { "local" | "global" => api::LOCAL, _ => api::HOME };
            let q = query_string(&[("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()), ("max_id", str_or(&args["cursor"], "").to_string())]);
            let list = as_list(get(args, &format!("{path}{q}"))?);
            Ok(page(list.iter().map(|s| post_wire(s, &host)).collect(), next_cursor(&list)))
        }
        "sns.post" => Ok(json!({ "post": post_wire(&get(args, &format!("{}/{}", api::STATUSES, urlencode(str_or(&args["id"], ""))))?, &host) })),
        "sns.thread" => {
            let id = urlencode(str_or(&args["id"], ""));
            let target = get(args, &format!("{}/{id}", api::STATUSES))?;
            let context = get(args, &format!("{}/{id}/context", api::STATUSES))?;
            Ok(json!({ "thread": { "post": post_wire(&target, &host), "ancestors": as_list(context["ancestors"].clone()).iter().map(|s| post_wire(s, &host)).collect::<Vec<_>>(), "descendants": as_list(context["descendants"].clone()).iter().map(|s| post_wire(s, &host)).collect::<Vec<_>>() } }))
        }
        "sns.performAction" => {
            let id = urlencode(str_or(&args["postId"], ""));
            let (on, off) = match str_or(&args["actionId"], "") {
                "favorite" => ("favourite", "unfavourite"),
                "repost" => ("reblog", "unreblog"),
                "reply" | "quote" => return Ok(json!({ "result": { "done": false, "message": "compose" } })),
                other => return Err(err("C01", format!("unsupported action: {other}"))),
            };
            let undo = args["args"]["undo"].as_bool().unwrap_or(false);
            post(args, &format!("{}/{id}/{}", api::STATUSES, if undo { off } else { on }), json!({}))?;
            Ok(json!({ "result": { "done": !undo } }))
        }
        "sns.createPost" => {
            let draft = &args["draft"];
            empty_post(args, str_or(&draft["text"], ""))?;
            let mut body = json!({ "status": str_or(&draft["text"], "") });
            if let Some(r) = draft["replyToId"].as_str() {
                body["in_reply_to_id"] = json!(r);
            }
            Ok(json!({ "post": post_wire(&post(args, api::STATUSES, body)?, &host) }))
        }
        "sns.deletePost" => fetch(args, "DELETE", &format!("{}/{}", api::STATUSES, urlencode(str_or(&args["id"], ""))), None).map(|_| json!({})),
        "sns.user" => {
            let id = urlencode(str_or(&args["id"], str_or(&args["handle"], "")));
            let a = get(args, &format!("{}/{id}", api::ACCOUNTS))?;
            Ok(json!({ "profile": { "user": user_wire(&a, &host), "description": a["note"], "followersCount": a["followers_count"], "followingCount": a["following_count"], "postsCount": a["statuses_count"], "createdAt": a["created_at"] } }))
        }
        "sns.userPosts" => {
            let id = urlencode(str_or(&args["id"], ""));
            let q = query_string(&[("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()), ("max_id", str_or(&args["cursor"], "").to_string())]);
            let list = as_list(get(args, &format!("{}/{id}/statuses{q}", api::ACCOUNTS))?);
            Ok(page(list.iter().map(|s| post_wire(s, &host)).collect(), next_cursor(&list)))
        }
        "sns.notifications" => {
            let q = query_string(&[("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string()), ("max_id", str_or(&args["cursor"], "").to_string())]);
            let list = as_list(get(args, &format!("{}{q}", api::NOTIFICATIONS))?);
            let items: Vec<Value> = list.iter().map(|n| json!({ "id": n["id"], "kind": match str_or(&n["type"], "") { "favourite" => "reaction", "reblog" => "repost", "mention" => "mention", "follow" => "follow", _ => "other" }, "createdAt": n["created_at"], "read": false, "actors": if n["account"].is_object() { json!([user_wire(&n["account"], &host)]) } else { json!([]) }, "post": if n["status"].is_object() { post_wire(&n["status"], &host) } else { Value::Null } })).collect();
            Ok(page(items, next_cursor(&list)))
        }
        "sns.search" => {
            let users = str_or(&args["kind"], "posts") == "users";
            let q = query_string(&[("q", str_or(&args["q"], "").to_string()), ("type", if users { "accounts".into() } else { "statuses".into() }), ("limit", args["limit"].as_u64().unwrap_or(20).min(40).to_string())]);
            let res = get(args, &format!("{}{q}", api::SEARCH))?;
            if users {
                let list = as_list(res["accounts"].clone());
                return Ok(page(list.iter().map(|u| user_wire(u, &host)).collect(), next_cursor(&list)));
            }
            let list = as_list(res["statuses"].clone());
            Ok(page(list.iter().map(|s| post_wire(s, &host)).collect(), next_cursor(&list)))
        }
        other => Err(err("C01", format!("unsupported fn: {other}"))),
    }
}

// ── 共通の宣言と dispatch ───────────────────────────────────────

fn sns_actions(loc: &str) -> Value {
    let t = |k| TEXT.t(loc, k);
    json!({ "actions": [
        { "id": "reply",    "genreTag": "Reply",    "label": t("action.reply"),    "icon": "reply",        "confirmMode": "confirm", "fow": "interstitial", "countSource": "self" },
        { "id": "repost",   "genreTag": "Repost",   "label": t("action.repost"), "icon": "repeat",       "doneIcon": "repeat_on", "countSource": "self" },
        { "id": "favorite", "genreTag": "Reaction", "label": t("action.favorite"), "icon": "star_border",  "doneIcon": "star",      "countSource": "self", "emoji": STAR },
        { "id": "quote",    "genreTag": "Quote",    "label": t("action.quote"),    "icon": "format_quote", "confirmMode": "confirm", "fow": "interstitial", "countSource": "self" },
    ] })
}

/// `call("sns.<fn>", args)` の本体。wasm の `call` export とテストが共用する。
/// 本文なしの投稿 (D-494): このサンプルは添付 / 投票を送れない(`sns.uploadMedia` が無い)ので、本文が空なら空の投稿を作らずに断る。
fn empty_post(args: &Value, text: &str) -> Result<(), Value> {
    if text.trim().is_empty() {
        return Err(err("D01", TEXT.t(i18n::locale_of(args), "error.emptyPost")));
    }
    Ok(())
}

pub fn call_impl(fn_name: &str, args: &Value) -> Result<Value, Value> {
    let loc = i18n::locale_of(args);
    match fn_name {
        "sns.capabilities" => return Ok(json!({ "maxPostLength": MAX_POST_LENGTH })),
        "sns.timelines" => return Ok(json!({ "timelines": [{ "id": "home", "label": TEXT.t(loc, "timeline.home") }, { "id": "local", "label": TEXT.t(loc, "timeline.local") }] })),
        "sns.actions" => return Ok(sns_actions(loc)),
        "sns.emojis" => return Ok(json!({ "emojis": [] })),
        // ハッシュタグの検索(コンポーザーの `#` の補完、rasq-client D-475)は持たない: C01 でホストは候補を出さない。
        "sns.search" if args["kind"].as_str() == Some("hashtags") => return Err(err("C01", "unsupported: sns.search kind=hashtags")),
        // 設定ページ (D-044 / rasq-client D-482)。宣言はログイン前(匿名ワーカー)にも答える。
        "sns.settingsPages" => return Ok(settings::sns_settings_pages(args)),
        "sns.settingsRead" if args["area"].as_str() == Some("account") => {
            let me = call_impl("sns.me", args)?;
            return Ok(json!({ "values": settings::account_values(&me["user"]) }));
        }
        "sns.settingsRead" => return Err(err("C01", "no such remote settings area")),
        // account は読むだけ(書くタイルを出していない)。
        "sns.settingsWrite" => return Err(err("C01", TEXT.t(loc, "error.accountReadOnly"))),
        _ => {}
    }
    let mut out = if is_dummy(args) { dummy_call(fn_name, args) } else { real_call(fn_name, args) };
    // TL 設定 (D-482): リポスト / 返信を外す。カーソルは外す前のもの(次のページはずれない)。
    if fn_name == "sns.timeline" {
        if let Ok(v) = out.as_mut() {
            let tl_settings = &args["settings"]["timeline"];
            if let Some(items) = v["page"]["items"].as_array_mut() {
                items.retain(|p| settings::keep_in_timeline(p, tl_settings));
            }
        }
    }
    out
}

#[no_mangle]
pub extern "C" fn call(fn_ptr: u32, fn_len: u32, args_ptr: u32, args_len: u32, call_id_ptr: u32, call_id_len: u32) -> u64 {
    let fn_name = read_str(fn_ptr, fn_len);
    let args = read_str(args_ptr, args_len);
    let call_id = read_str(call_id_ptr, call_id_len);
    let args_value: Value = serde_json::from_str(&args).unwrap_or(Value::Null);
    let outcome = call_impl(&fn_name, &args_value);
    let mut msg = json!({ "schema": "rasq.boundary/1", "scope": SCOPE, "type": "result", "callId": call_id, "ok": outcome.is_ok() });
    match outcome {
        Ok(value) => msg["value"] = value,
        Err(error) => msg["error"] = error,
    }
    pack(msg.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy(fn_name: &str, args: Value) -> Value {
        let mut a = args;
        a["host"] = json!(DUMMY_HOST);
        a["token"] = json!("dummy:me");
        call_impl(fn_name, &a).unwrap_or_else(|e| panic!("{fn_name}: {e}"))
    }

    /// ハッシュタグの検索は持たない(C01 → ホストは候補を出さない、rasq-client D-475)。
    #[test]
    fn hashtag_search_is_unsupported() {
        let mut a = json!({ "kind": "hashtags", "q": "打", "host": DUMMY_HOST, "token": "dummy:me" });
        assert_eq!(call_impl("sns.search", &a).unwrap_err()["code"], json!("C01"));
        a["token"] = json!("real-token");
        assert_eq!(call_impl("sns.search", &a).unwrap_err()["code"], json!("C01"));
    }

    #[test]
    fn dummy_login_timeline_actions_post_and_notifications() {
        let login = dummy("sns.login", json!({ "identifier": "me" }));
        assert!(login["session"]["token"].as_str().unwrap().starts_with("dummy:"));
        assert_eq!(login["me"]["kind"], json!("mixi2"));
        let tl = dummy("sns.timeline", json!({ "kind": "home", "limit": 5 }));
        assert_eq!(tl["page"]["items"].as_array().unwrap().len(), 5);
        assert_eq!(tl["page"]["nextCursor"], json!("5"));
        let tl2 = dummy("sns.timeline", json!({ "kind": "home", "limit": 5, "cursor": "5" }));
        assert_eq!(tl2["page"]["items"].as_array().unwrap().len(), 3);
        // いいね → myReaction ⭐、通知が増える、もう一度で取り消し。
        let r = dummy("sns.performAction", json!({ "actionId": "favorite", "postId": "d-7", "args": {}, "now": "2026-09-13T10:00:00.000Z" }));
        assert_eq!(r["result"]["done"], json!(true));
        assert_eq!(dummy("sns.post", json!({ "id": "d-7" }))["post"]["myReaction"], json!(STAR));
        assert_eq!(dummy("sns.notifications", json!({}))["page"]["items"][0]["kind"], json!("reaction"));
        let r = dummy("sns.performAction", json!({ "actionId": "favorite", "postId": "d-7", "args": {} }));
        assert_eq!(r["result"]["done"], json!(false));
        assert!(dummy("sns.post", json!({ "id": "d-7" }))["post"]["myReaction"].is_null());
        // 投稿 → TL の先頭、返信 → スレッド。
        let p = dummy("sns.createPost", json!({ "draft": { "text": "テスト投稿" }, "now": "2026-09-13T11:00:00.000Z" }));
        assert_eq!(dummy("sns.timeline", json!({ "limit": 1 }))["page"]["items"][0]["id"], p["post"]["id"]);
        let reply = dummy("sns.createPost", json!({ "draft": { "text": "返信", "replyToId": "d-5" } }));
        let th = dummy("sns.thread", json!({ "id": reply["post"]["id"] }));
        assert_eq!(th["thread"]["ancestors"][0]["id"], json!("d-5"));
        assert_eq!(dummy("sns.thread", json!({ "id": "d-5" }))["thread"]["descendants"].as_array().unwrap().len(), 1);
        // 検索 / 文字数上限。
        assert_eq!(dummy("sns.search", json!({ "q": "天気" }))["page"]["items"][0]["id"], json!("d-7"));
        let long = "あ".repeat(MAX_POST_LENGTH + 1);
        assert!(call_impl("sns.createPost", &json!({ "host": DUMMY_HOST, "draft": { "text": long } })).is_err());
    }

    /// 設定ページ (D-482): 4 ページを宣言し、account は sns.me を読むだけ(書けない)。
    #[test]
    fn settings_pages_account_is_read_only_and_timeline_filters_apply() {
        let pages = call_impl("sns.settingsPages", &json!({})).unwrap()["pages"].as_array().unwrap().clone();
        let areas: Vec<&str> = pages.iter().map(|p| p["area"].as_str().unwrap()).collect();
        assert_eq!(areas, ["account", "timeline", "safety", "security"]);
        for p in &pages {
            for node in p["root"]["c"].as_array().unwrap() {
                if let Some(k) = node["k"].as_str() {
                    assert!(p["defaults"].get(k).is_some(), "{}: {k} has no default", p["area"]);
                }
                if p["area"] == "account" {
                    assert!(node["on"].is_null(), "account は書くタイルを持たない");
                }
            }
        }
        let v = dummy("sns.settingsRead", json!({ "area": "account" }))["values"].clone();
        assert_eq!(v, json!({ "displayName": "わたし(ダミー)", "handle": "@me", "bio": "わたし(ダミー)(ダミー)" }));
        let a = json!({ "host": DUMMY_HOST, "token": "dummy:me", "area": "account", "key": "displayName", "value": "x" });
        assert_eq!(call_impl("sns.settingsWrite", &a).unwrap_err()["code"], json!("C01"));
        assert_eq!(dummy_err("sns.settingsRead", json!({ "area": "timeline" }))["code"], json!("C01"));

        // 返信を外す(ダミーの TL に返信を 1 件足してから)。
        dummy("sns.createPost", json!({ "draft": { "text": "返信です", "replyToId": "d-8" } }));
        let all = dummy("sns.timeline", json!({ "limit": 50 }))["page"]["items"].as_array().unwrap().len();
        let off = dummy("sns.timeline", json!({ "limit": 50, "settings": { "timeline": { "withReplies": false } } }))["page"]["items"].as_array().unwrap().clone();
        assert_eq!(off.len(), all - 1);
        assert!(off.iter().all(|p| !p["replyToId"].is_string()));
        assert!(settings::keep_in_timeline(&json!({ "repostedBy": {} }), &json!({})));
        assert!(!settings::keep_in_timeline(&json!({ "repostedBy": {} }), &json!({ "withReposts": false })));
    }

    /// 利用者に見せる文は表から引く(`args.locale`、PROTOCOL.md §4.6)。表の無い言語・読めないタグは ja。
    #[test]
    fn ui_text_comes_from_the_table_and_falls_back_to_ja() {
        let _ = i18n::take_misses();
        for f in ["sns.settingsPages", "sns.actions", "sns.timelines"] {
            let base = call_impl(f, &json!({})).unwrap();
            for loc in ["ja", "ja-JP", "fr", "", "??"] {
                assert_eq!(call_impl(f, &json!({ "locale": loc })).unwrap(), base, "{f} locale={loc}");
            }
            for loc in ["en", "en-US", "zh-Hans", "zh-Hant-TW", "ko"] {
                assert_ne!(call_impl(f, &json!({ "locale": loc })).unwrap(), base, "{f} locale={loc}");
            }
        }
        let pages = call_impl("sns.settingsPages", &json!({})).unwrap()["pages"].clone();
        assert_eq!(pages[0]["title"], "アカウント");
        assert_eq!(pages[2]["root"]["c"][1]["p"]["title"], "ぼかして表示");
        assert_eq!(pages[3]["root"]["c"][1]["p"]["value"], "RASQ サーバー経由の OAuth");
        let pages = call_impl("sns.settingsPages", &json!({ "locale": "en" })).unwrap()["pages"].clone();
        assert_eq!(pages[0]["title"], "Account");
        assert_eq!(pages[2]["root"]["c"][1]["p"]["title"], "Blur");
        assert_eq!(pages[3]["root"]["c"][1]["p"]["value"], "OAuth via the RASQ server");
        assert_eq!(call_impl("sns.timelines", &json!({})).unwrap()["timelines"][1]["label"], "みんな");
        assert_eq!(call_impl("sns.timelines", &json!({ "locale": "zh-Hans" })).unwrap()["timelines"][1]["label"], "大家");
        assert_eq!(call_impl("sns.actions", &json!({ "locale": "zh-Hant" })).unwrap()["actions"][1]["label"], "轉發");
        let e = call_impl("sns.login", &json!({ "host": "mixi.social" })).unwrap_err();
        assert_eq!(e["message"], "mixi2 は OAuth でログインします(RASQ サーバーのブローカー経由)。動作確認はホスト「dummy」で。");
        let e = call_impl("sns.login", &json!({ "host": "mixi.social", "locale": "ko" })).unwrap_err();
        assert_eq!(e["message"], "mixi2는 OAuth로 로그인합니다(RASQ 서버의 브로커 경유). 동작 확인은 호스트 'dummy'로 하세요.");
        let long = "あ".repeat(MAX_POST_LENGTH + 1);
        let e = call_impl("sns.createPost", &json!({ "host": DUMMY_HOST, "draft": { "text": long } })).unwrap_err();
        assert_eq!(e["message"], "本文は 149 文字まで");
        let e = call_impl("sns.createPost", &json!({ "host": DUMMY_HOST, "locale": "en", "draft": { "text": long } })).unwrap_err();
        assert_eq!(e["message"], "Text can be up to 149 characters");
        assert!(i18n::take_misses().is_empty(), "表に無いキーを引いた");
    }

    fn dummy_err(fn_name: &str, args: Value) -> Value {
        let mut a = args;
        a["host"] = json!(DUMMY_HOST);
        a["token"] = json!("dummy:me");
        call_impl(fn_name, &a).unwrap_err()
    }

    #[test]
    fn real_mode_refuses_credential_login_and_declares_actions() {
        let e = call_impl("sns.login", &json!({ "host": "mixi.social", "identifier": "x", "password": "y" })).unwrap_err();
        assert_eq!(e["code"], json!("A01"));
        let a = call_impl("sns.actions", &json!({})).unwrap();
        assert_eq!(a["actions"].as_array().unwrap().len(), 4);
        // ネイティブテストにはホストが無いので実モードの fetch は B01。
        assert_eq!(call_impl("sns.me", &json!({ "host": "mixi.social", "token": "t" })).unwrap_err()["code"], json!("B01"));
    }

    /// 本文なしの投稿 (D-494): このサンプルは添付 / 投票を送れない(`sns.uploadMedia` が無い)ので、本文が空なら
    /// 空の投稿を送らずに断る(実モードも、通信の前に)。
    #[test]
    fn empty_text_is_refused_in_both_modes() {
        let e = dummy_err("sns.createPost", json!({ "draft": { "text": " \n", "poll": { "choices": ["a", "b"] } } }));
        assert_eq!(e["code"], json!("D01"));
        assert_eq!(e["message"], "本文を入れてください(mixi2 には添付 / 投票を送れません)");
        let e = call_impl("sns.createPost", &json!({ "host": "mixi.social", "token": "t", "draft": { "text": "", "mediaIds": ["m1"] } })).unwrap_err();
        assert_eq!(e["code"], json!("D01"));
    }
}
