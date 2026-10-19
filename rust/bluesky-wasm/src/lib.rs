//! Bluesky (AT Protocol) WASM plugin for RASQ (PROTOCOL.md §4.3 の `sns.*` 境界)。
//!
//! [`mastodon-wasm`](../mastodon-wasm) と同じ実装パターン——ホスト関数
//! `net.fetch` 越しに SNS の API を叩き、`rasq.post/1` のワイヤを返す——を
//! Bluesky に当てたもの。RRL の画面コンストラクタは持たない**イベント
//! ハンドラー専用プラグイン**で、描画はホストの標準 PostTile に任せる。
//!
//! Mastodon / Misskey サンプルとの違いは、どれも AT Protocol の作りに由来する。
//! **契約(ワイヤ)は同じ形に揃えてある**:
//!
//! 1. **すべての要求はアカウントの PDS へ** (`https://<host>/xrpc/<nsid>`)。
//!    読み取り系 (`app.bsky.*`) は PDS が AppView へ代理転送する。認証は
//!    `Authorization: Bearer <accessJwt>`([`xrpc`])。
//! 2. **投稿 id は `at://` URI**。レコードの操作(いいね・リポスト・返信・
//!    引用)には `{uri, cid}` の strong ref が要るので、見た投稿の cid を
//!    メモリ上の [`CID_CACHE`] に溜め、無ければ `getPosts` で引き直す。
//! 3. **トークンは JSON 文字列** `{did, handle, accessJwt, refreshJwt}`
//!    (`sns.login` が作り、ホストが `args.token` に毎回載せる)。accessJwt は
//!    ~2 時間で切れるので、`ExpiredToken` を見たら `refreshSession` して 1 回
//!    だけやり直し、新しい組を `storage.safe` の `session:<did>` に残す。
//!    以後の呼び出しはホストの token より保存済みの組を優先する。
//! 4. **本文はプレーンテキスト + facets**。リンク / ハッシュタグ / メンションは
//!    サーバー側で解析されず、投稿側が UTF-8 バイト範囲で `facets` を付ける
//!    ([`build_facets`])。表示側はホストの RSL 解析に任せる(`bodyFormat: plain`)。
//! 5. **300 グラフェム上限**、**ブックマーク / リスト無し**(`sns.favorites` /
//!    `sns.lists*` は C01 → ホストが該当 UI を隠す)、**リアクションは「いいね」
//!    のみ**(⭐ 固定。Mastodon サンプルと同じ規約)。引用はネイティブにある
//!    (`app.bsky.embed.record`)。
//! 6. **時計が無い**。`createdAt` / `seenAt` はホストが `args.now` に載せる
//!    ISO 8601 を使う(D-175)。渡されなければ `1970-01-01T00:00:00.000Z`
//!    になる — 投稿は通るが TL では過去の日付になる。
//!
//! リポスト(feed の `reasonRepost`)は Misskey の純リノートと同じ規則で中身へ
//! 展開し、`repostedBy` と `repostTargetId` を載せる (D-100 / D-112)。

use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use unicode_segmentation::UnicodeSegmentation;

const SCOPE: &str = "rasq.official.bluesky:bluesky:bsky.social:sample:0.1.0:sns";
const KIND: &str = "bluesky";
const DEFAULT_HOST: &str = "bsky.social";
/// Bluesky の投稿本文上限(`app.bsky.feed.post` の `maxGraphemes`)。
const MAX_POST_GRAPHEMES: usize = 300;
/// 「いいね」を写すリアクション文字列(Mastodon サンプルと同じ規約)。
const STAR: &str = "⭐";
/// Discover(旧 What's Hot)フィードジェネレーター。`global` TL に当てる。
const DISCOVER_FEED: &str = "at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot";
/// Bluesky チャットはこのサービスへ PDS 経由で代理転送される。
const CHAT_PROXY: &str = "did:web:api.bsky.chat#bsky_chat";
/// 通報の宛先(Bluesky のモデレーションサービス)。
const MOD_PROXY: &str = "did:plc:ar7c4by46qjdydhdevvrndac#atproto_labeler";
/// `getPosts` / `getProfiles` の 1 回あたり上限。
const BATCH: usize = 25;
/// `now` が渡されなかったときの createdAt(§6 の制限)。
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

// ── wasm ABI (PROTOCOL §4.1) ──────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "rasq")]
extern "C" {
    /// ホスト関数 import。リクエスト JSON の ptr/len を渡し、レスポンスを
    /// 書いた `(ptr << 32) | len` を受け取る(同期 v1)。
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// native(テスト)でホスト関数を差し替える口(rasq-plugin-kit と同じ形)。
    static HOST_HOOK: RefCell<Option<Box<dyn FnMut(&str, Value) -> Value>>> = RefCell::new(None);
    /// native でのホスト関数呼び出し記録(テストが読む)。
    static HOST_LOG: RefCell<Vec<(String, Value)>> = RefCell::new(Vec::new());
}

/// テスト用: ホスト関数の応答を差し替える(戻り値は `value` 部分。`{ok:true,value}` に包まれる)。
#[cfg(not(target_arch = "wasm32"))]
pub fn set_host_hook<F: FnMut(&str, Value) -> Value + 'static>(f: F) {
    HOST_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

/// テスト用: これまでのホスト関数呼び出し `(fn, args)` を取り出す(取り出すと空になる)。
#[cfg(not(target_arch = "wasm32"))]
pub fn take_host_log() -> Vec<(String, Value)> {
    HOST_LOG.with(|l| mem::take(&mut *l.borrow_mut()))
}

/// ホスト関数(`rasq.hostfn/1`)を呼び、レスポンス JSON(`{callId, ok, value|error}`)を返す。
fn host_call(fn_name: &str, args: Value) -> Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    #[cfg(target_arch = "wasm32")]
    {
        let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
        let bytes = request.as_bytes();
        let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
        let ptr = (packed >> 32) as u32;
        let len = (packed & 0xFFFF_FFFF) as u32;
        // ホストは我々の `alloc` で確保した領域に書いたので、ここで回収する。
        let response = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
        serde_json::from_slice(&response).unwrap_or(Value::Null)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        HOST_LOG.with(|l| l.borrow_mut().push((fn_name.to_string(), args.clone())));
        let hooked = HOST_HOOK.with(|h| h.borrow_mut().as_mut().map(|f| f(fn_name, args.clone())));
        match hooked {
            Some(v) => json!({"callId": call_id, "ok": true, "value": v}),
            None => json!({"callId": call_id, "ok": false, "error": {"code": "E01", "message": "no host (native)"}}),
        }
    }
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

// ── 共通ユーティリティ ───────────────────────────────────────────

/// `call` の失敗 `{code, message}`。HTTP 系はステータス文字列、それ以外は
/// CONTRACT §9 のコード(`C01` 未対応 / `D01` 入力不正 / `B01` 通信)。
fn err(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn unsupported(what: &str) -> Value {
    err("C01", format!("unsupported: {what}"))
}

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

/// `[(key, value)]` → `?a=1&b=2`。空の値は落とす(`cursor` 未指定など)。
/// 同じキーの繰り返し(`uris=a&uris=b`)はそのまま並べる。
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

fn as_list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn str_or<'a>(v: &'a Value, fallback: &'a str) -> &'a str {
    v.as_str().unwrap_or(fallback)
}

/// `at://did/collection/rkey` の rkey。
fn rkey_of(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or("")
}

/// `at://did/collection/rkey` の did。
fn repo_of(uri: &str) -> &str {
    uri.trim_start_matches("at://").split('/').next().unwrap_or("")
}

/// `{page: {items, nextCursor}}`。カーソルは AT Protocol のものをそのまま通す。
fn page(items: Vec<Value>, cursor: &Value) -> Value {
    json!({ "page": { "items": items, "nextCursor": if cursor.is_string() { cursor.clone() } else { Value::Null } } })
}

fn limit_arg(args: &Value, default: u64, max: u64) -> String {
    args["limit"].as_u64().unwrap_or(default).clamp(1, max).to_string()
}

fn cursor_arg(args: &Value) -> String {
    args["cursor"].as_str().unwrap_or("").to_string()
}

// ── セッション / XRPC ────────────────────────────────────────────

/// ホストが `args.token` に載せる JSON(`sns.login` の `session` と同形)。
#[derive(Clone, Debug, PartialEq)]
struct Session {
    did: String,
    handle: String,
    access_jwt: String,
    refresh_jwt: String,
}

impl Session {
    fn from_token(args: &Value) -> Result<Session, Value> {
        let raw = match args["token"].as_str() {
            Some(t) if !t.trim().is_empty() => t,
            _ => return Err(err("401", "no session token (login required)")),
        };
        let v: Value = serde_json::from_str(raw)
            .map_err(|_| err("401", "session token is not the {did, handle, accessJwt, refreshJwt} JSON"))?;
        let did = v["did"].as_str().unwrap_or("").to_string();
        let access = v["accessJwt"].as_str().unwrap_or("").to_string();
        if did.is_empty() || access.is_empty() {
            return Err(err("401", "session token lacks did / accessJwt"));
        }
        Ok(Session {
            did,
            handle: v["handle"].as_str().unwrap_or("").to_string(),
            access_jwt: access,
            refresh_jwt: v["refreshJwt"].as_str().unwrap_or("").to_string(),
        })
    }

    fn storage_key(&self) -> String {
        format!("session:{}", self.did)
    }
}

/// 1 回の `call` の文脈: 引数・接続先・(あれば)セッション。
struct Ctx<'a> {
    args: &'a Value,
    host: String,
    /// refresh で差し替わるので RefCell。匿名呼び出し(`sns.login` 等)は None。
    session: RefCell<Option<Session>>,
}

impl<'a> Ctx<'a> {
    fn anonymous(args: &'a Value) -> Ctx<'a> {
        let host = args["host"].as_str().filter(|h| !h.is_empty()).unwrap_or(DEFAULT_HOST).to_string();
        Ctx { args, host, session: RefCell::new(None) }
    }

    /// 認証つき。ホストの token を読み、`storage.safe` に refresh 済みの組が
    /// あればそちらを優先する(ホスト側の保存は古い accessJwt のまま)。
    fn authed(args: &'a Value) -> Result<Ctx<'a>, Value> {
        let ctx = Ctx::anonymous(args);
        let mut session = Session::from_token(args)?;
        let stored = host_call("storage.get", json!({ "ns": "safe", "key": session.storage_key() }));
        if stored["ok"] == true {
            let v = &stored["value"]["value"];
            if let (Some(a), Some(r)) = (v["accessJwt"].as_str(), v["refreshJwt"].as_str()) {
                if !a.is_empty() && !r.is_empty() {
                    session.access_jwt = a.to_string();
                    session.refresh_jwt = r.to_string();
                }
            }
        }
        *ctx.session.borrow_mut() = Some(session);
        Ok(ctx)
    }

    /// 接続先の基底 URL。`baseUrl` はテスト / 自前ホスティング向けの上書き。
    fn base(&self) -> String {
        match self.args["baseUrl"].as_str() {
            Some(b) => b.trim_end_matches('/').to_string(),
            None => format!("https://{}", self.host),
        }
    }

    fn did(&self) -> String {
        self.session.borrow().as_ref().map(|s| s.did.clone()).unwrap_or_default()
    }

    /// 現在時刻(ISO 8601)。wasm に時計は無いのでホストの `args.now` を使う。
    fn now(&self) -> String {
        self.args["now"].as_str().filter(|s| !s.is_empty()).unwrap_or(EPOCH).to_string()
    }
}

enum Body {
    Json(Value),
    /// バイナリ(`uploadBlob`)。`net.fetch` の `body: {b64}` にそのまま載せる。
    Raw { b64: String, mime: String },
}

/// XRPC を `net.fetch` 経由で叩く共通口。
///
/// 2xx 以外は `{code: "<status>", message: "<error>: <message>"}`。
/// `ExpiredToken` なら `refreshSession` して 1 回だけやり直す。
fn xrpc(ctx: &Ctx, method: &str, nsid: &str, params: &[(&str, String)], body: Option<Body>, proxy: Option<&str>) -> Result<Value, Value> {
    let mut request = json!({
        "method": method,
        "url": format!("{}/xrpc/{nsid}{}", ctx.base(), query_string(params)),
        "headers": { "Accept": "application/json" },
    });
    if let Some(p) = proxy {
        request["headers"]["atproto-proxy"] = json!(p);
    }
    match body {
        Some(Body::Json(v)) => {
            request["headers"]["Content-Type"] = json!("application/json");
            request["body"] = json!(v.to_string());
        }
        Some(Body::Raw { b64, mime }) => {
            request["headers"]["Content-Type"] = json!(mime);
            request["body"] = json!({ "b64": b64 });
        }
        None => {}
    }

    let mut retried = false;
    loop {
        if let Some(s) = ctx.session.borrow().as_ref() {
            request["headers"]["Authorization"] = json!(format!("Bearer {}", s.access_jwt));
        }
        let response = host_call("net.fetch", request.clone());
        if response["ok"].as_bool() != Some(true) {
            return Err(response.get("error").cloned().unwrap_or(err("B01", "net.fetch failed")));
        }
        let value = &response["value"];
        let status = value["status"].as_u64().unwrap_or(0);
        let text = value["bodyText"].as_str().unwrap_or("");
        if (200..300).contains(&status) {
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(text).map_err(|e| err("D01", format!("bad JSON from {nsid}: {e}")));
        }
        let body_json: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let error_name = body_json["error"].as_str().unwrap_or("");
        let can_refresh = ctx.session.borrow().as_ref().map(|s| !s.refresh_jwt.is_empty()).unwrap_or(false);
        if error_name == "ExpiredToken" && !retried && can_refresh {
            retried = true;
            refresh_session(ctx)?;
            continue;
        }
        let message = match (error_name, body_json["message"].as_str()) {
            ("", None) => format!("HTTP {status} from {nsid}"),
            (e, None) => format!("{e} ({nsid})"),
            ("", Some(m)) => m.to_string(),
            (e, Some(m)) => format!("{e}: {m}"),
        };
        return Err(err(&status.to_string(), message));
    }
}

fn xrpc_get(ctx: &Ctx, nsid: &str, params: &[(&str, String)]) -> Result<Value, Value> {
    xrpc(ctx, "GET", nsid, params, None, None)
}

fn xrpc_post(ctx: &Ctx, nsid: &str, body: Value) -> Result<Value, Value> {
    xrpc(ctx, "POST", nsid, &[], Some(Body::Json(body)), None)
}

/// `com.atproto.server.refreshSession`(`Authorization: Bearer <refreshJwt>`)。
/// 成功したら文脈のセッションを差し替え、`storage.safe` に新しい組を残す。
fn refresh_session(ctx: &Ctx) -> Result<(), Value> {
    let Some(current) = ctx.session.borrow().clone() else {
        return Err(err("401", "no session to refresh"));
    };
    let request = json!({
        "method": "POST",
        "url": format!("{}/xrpc/com.atproto.server.refreshSession", ctx.base()),
        "headers": { "Accept": "application/json", "Authorization": format!("Bearer {}", current.refresh_jwt) },
    });
    let response = host_call("net.fetch", request);
    if response["ok"].as_bool() != Some(true) {
        return Err(response.get("error").cloned().unwrap_or(err("B01", "net.fetch failed")));
    }
    let status = response["value"]["status"].as_u64().unwrap_or(0);
    let body: Value = serde_json::from_str(response["value"]["bodyText"].as_str().unwrap_or("")).unwrap_or(Value::Null);
    if !(200..300).contains(&status) || !body["accessJwt"].is_string() {
        // refreshJwt も切れている(90 日) — ログインし直してもらう。
        return Err(err("401", "session expired; please log in again"));
    }
    let renewed = Session {
        did: body["did"].as_str().unwrap_or(&current.did).to_string(),
        handle: body["handle"].as_str().unwrap_or(&current.handle).to_string(),
        access_jwt: body["accessJwt"].as_str().unwrap_or("").to_string(),
        refresh_jwt: body["refreshJwt"].as_str().unwrap_or(&current.refresh_jwt).to_string(),
    };
    host_call(
        "storage.set",
        json!({ "ns": "safe", "key": renewed.storage_key(), "value": { "accessJwt": renewed.access_jwt, "refreshJwt": renewed.refresh_jwt } }),
    );
    *ctx.session.borrow_mut() = Some(renewed);
    Ok(())
}

// ── cid キャッシュ / strong ref ─────────────────────────────────

/// 見た投稿の `uri → cid`。いいね・リポスト・返信・引用の strong ref に使う。
static CID_CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn remember_cid(uri: &str, cid: &Value) {
    if let (false, Some(c)) = (uri.is_empty(), cid.as_str()) {
        let mut guard = CID_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        guard.get_or_insert_with(HashMap::new).insert(uri.to_string(), c.to_string());
    }
}

fn cached_cid(uri: &str) -> Option<String> {
    let guard = CID_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    guard.as_ref().and_then(|m| m.get(uri).cloned())
}

/// `app.bsky.feed.getPosts`(25 件ずつ)。見た cid はキャッシュに入れる。
fn get_posts(ctx: &Ctx, uris: &[String]) -> Result<Vec<Value>, Value> {
    let mut out = vec![];
    for chunk in uris.chunks(BATCH) {
        let params: Vec<(&str, String)> = chunk.iter().map(|u| ("uris", u.clone())).collect();
        let res = xrpc_get(ctx, "app.bsky.feed.getPosts", &params)?;
        for p in as_list(&res["posts"]) {
            remember_cid(str_or(&p["uri"], ""), &p["cid"]);
            out.push(p);
        }
    }
    Ok(out)
}

fn get_post(ctx: &Ctx, uri: &str) -> Result<Value, Value> {
    get_posts(ctx, &[uri.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| err("404", format!("post not found: {uri}")))
}

/// `{uri, cid}`。cid が手元に無ければ `getPosts` で引く。
fn strong_ref(ctx: &Ctx, uri: &str) -> Result<Value, Value> {
    let cid = match cached_cid(uri) {
        Some(c) => c,
        None => get_post(ctx, uri)?["cid"].as_str().unwrap_or("").to_string(),
    };
    if cid.is_empty() {
        return Err(err("404", format!("no cid for {uri}")));
    }
    Ok(json!({ "uri": uri, "cid": cid }))
}

fn create_record(ctx: &Ctx, collection: &str, record: Value) -> Result<Value, Value> {
    xrpc_post(ctx, "com.atproto.repo.createRecord", json!({ "repo": ctx.did(), "collection": collection, "record": record }))
}

fn delete_record(ctx: &Ctx, collection: &str, rkey: &str) -> Result<Value, Value> {
    xrpc_post(ctx, "com.atproto.repo.deleteRecord", json!({ "repo": ctx.did(), "collection": collection, "rkey": rkey }))
}

// ── ワイヤ変換 ───────────────────────────────────────────────────

/// ProfileView(Basic/Detailed) → `rasq.post/1` の user ワイヤ。
/// Bluesky のハンドルはドメイン(`alice.bsky.social`)そのもの。`instance` は
/// 閲覧アカウントの PDS を入れる(AccountId のキー体系を崩さないため)。
fn user_wire(host: &str, p: &Value) -> Value {
    let handle = str_or(&p["handle"], "");
    let display = match p["displayName"].as_str() {
        Some(n) if !n.trim().is_empty() => n,
        _ => handle,
    };
    let mut w = json!({
        "id": p["did"],
        "kind": KIND,
        "instance": host,
        "handle": handle,
        "displayName": display,
    });
    if let Some(a) = p["avatar"].as_str() {
        w["avatarUrl"] = json!(a);
    }
    if let Some(d) = p["description"].as_str() {
        w["bio"] = json!(d);
    }
    w
}

/// ProfileViewDetailed → Profile ワイヤ (`sns.user`)。
fn profile_wire(host: &str, p: &Value) -> Value {
    let handle = str_or(&p["handle"], "");
    let mut w = json!({
        "user": user_wire(host, p),
        "followersCount": p["followersCount"],
        "followingCount": p["followsCount"],
        "postsCount": p["postsCount"],
        "profileUrl": format!("https://bsky.app/profile/{handle}"),
    });
    if let Some(b) = p["banner"].as_str() {
        w["bannerUrl"] = json!(b);
    }
    if let Some(d) = p["description"].as_str() {
        w["description"] = json!(d);
    }
    if let Some(c) = p["createdAt"].as_str() {
        w["createdAt"] = json!(c);
    }
    w
}

/// `viewer` → FollowState ワイヤ。Bluesky は関係を**レコードの URI**で表す
/// (`following` があればフォロー中)。フォロー申請の概念は無い。
fn follow_state_wire(viewer: &Value) -> Value {
    json!({
        "following": viewer["following"].is_string(),
        "followedBy": viewer["followedBy"].is_string(),
        "requested": false,
        "muted": viewer["muted"].as_bool().unwrap_or(false),
        "blocked": viewer["blocking"].is_string(),
        "blockedBy": viewer["blockedBy"].as_bool().unwrap_or(false),
    })
}

fn post_url(handle: &str, uri: &str) -> String {
    format!("https://bsky.app/profile/{handle}/post/{}", rkey_of(uri))
}

/// 埋め込み(`embed` view)をメディア / 引用 / リンクに振り分ける。
fn embed_into(host: &str, embed: &Value, media: &mut Vec<Value>, quote: &mut Option<Value>, link: &mut Option<String>, depth: usize) {
    match str_or(&embed["$type"], "") {
        "app.bsky.embed.images#view" => {
            for img in as_list(&embed["images"]) {
                let mut m = json!({
                    "id": img["fullsize"],
                    "type": "image",
                    "url": img["fullsize"],
                    "thumbnailUrl": img["thumb"],
                });
                if let Some(alt) = img["alt"].as_str() {
                    if !alt.is_empty() {
                        m["alt"] = json!(alt);
                    }
                }
                if let (Some(w), Some(h)) = (img["aspectRatio"]["width"].as_u64(), img["aspectRatio"]["height"].as_u64()) {
                    m["width"] = json!(w);
                    m["height"] = json!(h);
                }
                media.push(m);
            }
        }
        "app.bsky.embed.video#view" => {
            let mut m = json!({
                "id": embed["cid"],
                "type": "video",
                "url": embed["playlist"],
            });
            if let Some(t) = embed["thumbnail"].as_str() {
                m["thumbnailUrl"] = json!(t);
            }
            if let Some(alt) = embed["alt"].as_str() {
                m["alt"] = json!(alt);
            }
            media.push(m);
        }
        "app.bsky.embed.external#view" => {
            if let Some(u) = embed["external"]["uri"].as_str() {
                *link = Some(u.to_string());
            }
        }
        "app.bsky.embed.record#view" => {
            *quote = record_view_wire(host, &embed["record"], depth);
        }
        "app.bsky.embed.recordWithMedia#view" => {
            embed_into(host, &embed["media"], media, quote, link, depth);
            *quote = record_view_wire(host, &embed["record"]["record"], depth);
        }
        _ => {}
    }
}

/// `app.bsky.embed.record#viewRecord` → 引用先の Post。見つからない /
/// ブロック済み / フィード・リスト等の非投稿は None。入れ子は 1 段まで。
fn record_view_wire(host: &str, record: &Value, depth: usize) -> Option<Value> {
    if depth >= 1 || str_or(&record["$type"], "") != "app.bsky.embed.record#viewRecord" {
        return None;
    }
    let embeds: Vec<Value> = as_list(&record["embeds"]);
    Some(post_wire_at(host, record, &record["value"], &embeds, depth + 1))
}

/// 投稿ラベルのうち閲覧注意にあたるもの。
fn is_sensitive(labels: &Value) -> bool {
    as_list(labels)
        .iter()
        .any(|l| matches!(str_or(&l["val"], ""), "porn" | "sexual" | "nudity" | "graphic-media" | "!warn"))
}

/// PostView / viewRecord 共通の Post ワイヤ組み立て。
///
/// `view` は `{uri, cid, author, likeCount, replyCount, repostCount, quoteCount,
/// viewer, labels, indexedAt}` を持つ側、`record` は `app.bsky.feed.post`
/// レコード本体(PostView なら `record`、viewRecord なら `value`)。
fn post_wire_at(host: &str, view: &Value, record: &Value, embeds: &[Value], depth: usize) -> Value {
    let uri = str_or(&view["uri"], "");
    remember_cid(uri, &view["cid"]);
    let handle = str_or(&view["author"]["handle"], "");

    let mut media = vec![];
    let mut quote = None;
    let mut link = None;
    for e in embeds {
        embed_into(host, e, &mut media, &mut quote, &mut link, depth);
    }
    if is_sensitive(&view["labels"]) {
        for m in &mut media {
            m["sensitive"] = json!(true);
        }
    }

    let likes = view["likeCount"].as_u64().unwrap_or(0);
    let mut wire = json!({
        "id": uri,
        "author": user_wire(host, &view["author"]),
        "createdAt": record["createdAt"].as_str().or(view["indexedAt"].as_str()).unwrap_or(EPOCH),
        "body": str_or(&record["text"], ""),
        "bodyFormat": "plain",
        "media": media,
        // ホストはアクション id をキーに件数を引く(`post.counts[action.id]`)。
        "counts": {
            "reply": view["replyCount"].as_u64().unwrap_or(0),
            "repost": view["repostCount"].as_u64().unwrap_or(0),
            "favorite": likes,
            "quote": view["quoteCount"].as_u64().unwrap_or(0),
        },
        "visibility": "public",
        "url": post_url(handle, uri),
    });
    if let Some(parent) = record["reply"]["parent"]["uri"].as_str() {
        wire["replyToId"] = json!(parent);
    }
    if let Some(q) = quote {
        wire["quote"] = q;
    }
    if let Some(l) = link {
        wire["linkUrl"] = json!(l);
    }
    if likes > 0 {
        wire["reactions"] = json!({ STAR: likes });
    }
    if view["viewer"]["like"].is_string() {
        wire["myReaction"] = json!(STAR);
    }
    wire
}

/// PostView → Post ワイヤ。
fn post_view_wire(host: &str, pv: &Value) -> Value {
    let embeds: Vec<Value> = if pv["embed"].is_object() { vec![pv["embed"].clone()] } else { vec![] };
    post_wire_at(host, pv, &pv["record"], &embeds, 0)
}

/// FeedViewPost → Post ワイヤ。
///
/// **リポストの展開 (D-100/D-112)**: feed の `reason` が `reasonRepost` なら
/// 中身の投稿へ展開し、外側の id を `<uri>#repost-by-<did>`、`repostedBy` に
/// リポストした人、`repostTargetId` に中身の uri を載せる。ホストはアクションと
/// スレッドをこの id へ読み替える。返信は `reply.parent` を `replyTo` に載せ、
/// ホストの親タイル表示に使う。
fn feed_item_wire(host: &str, item: &Value) -> Value {
    let post = &item["post"];
    let mut wire = post_view_wire(host, post);
    let parent = &item["reply"]["parent"];
    if str_or(&parent["$type"], "") == "app.bsky.feed.defs#postView" {
        let embeds: Vec<Value> = if parent["embed"].is_object() { vec![parent["embed"].clone()] } else { vec![] };
        wire["replyTo"] = post_wire_at(host, parent, &parent["record"], &embeds, 1);
    }
    let reason = &item["reason"];
    if str_or(&reason["$type"], "") == "app.bsky.feed.defs#reasonRepost" {
        let uri = str_or(&post["uri"], "").to_string();
        let by = str_or(&reason["by"]["did"], "");
        wire["id"] = json!(format!("{uri}#repost-by-{by}"));
        if let Some(t) = reason["indexedAt"].as_str() {
            wire["createdAt"] = json!(t);
        }
        wire["repostedBy"] = user_wire(host, &reason["by"]);
        wire["repostTargetId"] = json!(uri);
    }
    wire
}

fn feed_page(host: &str, res: &Value) -> Value {
    let items: Vec<Value> = as_list(&res["feed"]).iter().map(|i| feed_item_wire(host, i)).collect();
    page(items, &res["cursor"])
}

fn users_page(host: &str, list: &Value, cursor: &Value) -> Value {
    let items: Vec<Value> = as_list(list).iter().map(|u| user_wire(host, u)).collect();
    page(items, cursor)
}

// ── リッチテキスト(facets) ─────────────────────────────────────

fn is_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '「' | '（' | '、' | '。'),
    }
}

/// facets のうちネットワーク不要なもの(リンク / タグ)と、メンション候補の
/// `(byteStart, byteEnd, handle)` を返す。バイト範囲は UTF-8。
fn scan_facets(text: &str) -> (Vec<Value>, Vec<(usize, usize, String)>) {
    let mut facets = vec![];
    let mut mentions = vec![];
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let total = text.len();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        let prev = if i == 0 { None } else { Some(chars[i - 1].1) };
        let rest = &text[start..];
        if (rest.starts_with("https://") || rest.starts_with("http://")) && is_boundary(prev) {
            // 空白か引用符まで伸ばし、末尾の句読点を落とす。
            let mut j = i;
            while j < chars.len() && !chars[j].1.is_whitespace() && !matches!(chars[j].1, '<' | '>' | '"' | '「' | '」') {
                j += 1;
            }
            let mut end = if j < chars.len() { chars[j].0 } else { total };
            while end > start && matches!(text[..end].chars().last(), Some('.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '\'' | '。' | '、' | '）')) {
                end -= text[..end].chars().last().map(char::len_utf8).unwrap_or(1);
            }
            let url = &text[start..end];
            if url.len() > "https://".len() {
                facets.push(json!({
                    "index": { "byteStart": start, "byteEnd": end },
                    "features": [{ "$type": "app.bsky.richtext.facet#link", "uri": url }],
                }));
            }
            i = j.max(i + 1);
            continue;
        }
        if (c == '#' || c == '＃') && is_boundary(prev) {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].1.is_alphanumeric() || chars[j].1 == '_') {
                j += 1;
            }
            let end = if j < chars.len() { chars[j].0 } else { total };
            let tag_start = start + c.len_utf8();
            let tag = &text[tag_start..end];
            if !tag.is_empty() && !tag.chars().all(|c| c.is_ascii_digit()) && tag.len() <= 64 {
                facets.push(json!({
                    "index": { "byteStart": start, "byteEnd": end },
                    "features": [{ "$type": "app.bsky.richtext.facet#tag", "tag": tag }],
                }));
            }
            i = j.max(i + 1);
            continue;
        }
        if c == '@' && is_boundary(prev) {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || matches!(chars[j].1, '.' | '-')) {
                j += 1;
            }
            let mut end = if j < chars.len() { chars[j].0 } else { total };
            while end > start + 1 && matches!(text.as_bytes()[end - 1], b'.' | b'-') {
                end -= 1;
            }
            let handle = &text[start + 1..end];
            if is_handle(handle) {
                mentions.push((start, end, handle.to_string()));
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    (facets, mentions)
}

/// `alice.bsky.social` のような妥当なハンドルか(TLD 2 文字以上・空ラベル無し)。
fn is_handle(h: &str) -> bool {
    let labels: Vec<&str> = h.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-'))
        && labels.last().map(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic())).unwrap_or(false)
}

/// 本文から `app.bsky.richtext.facet` を組み立てる。メンションは
/// `com.atproto.identity.resolveHandle` で did に解決し、解決できなければ
/// 素のテキストとして残す(投稿自体は失敗させない)。
fn build_facets(ctx: &Ctx, text: &str) -> Vec<Value> {
    let (mut facets, mentions) = scan_facets(text);
    for (start, end, handle) in mentions {
        if let Ok(res) = xrpc_get(ctx, "com.atproto.identity.resolveHandle", &[("handle", handle.clone())]) {
            if let Some(did) = res["did"].as_str() {
                facets.push(json!({
                    "index": { "byteStart": start, "byteEnd": end },
                    "features": [{ "$type": "app.bsky.richtext.facet#mention", "did": did }],
                }));
            }
        }
    }
    facets.sort_by_key(|f| f["index"]["byteStart"].as_u64().unwrap_or(0));
    facets
}

fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

// ── sns.*: 認証前 ────────────────────────────────────────────────

/// `call("sns.login", {host, identifier, password})` — ホストの資格情報
/// ログイン画面(匿名ワーカー)から呼ばれる。`createSession` → プロフィール。
/// 戻りの `session` をホストが JSON 文字列にして `token` に保存する。
fn sns_login(args: &Value) -> Result<Value, Value> {
    let ctx = Ctx::anonymous(args);
    let identifier = args["identifier"].as_str().map(str::trim).unwrap_or("");
    let password = args["password"].as_str().unwrap_or("");
    if identifier.is_empty() || password.is_empty() {
        return Err(err("D01", "identifier and password are required"));
    }
    let s = xrpc_post(&ctx, "com.atproto.server.createSession", json!({ "identifier": identifier, "password": password }))?;
    let session = Session {
        did: str_or(&s["did"], "").to_string(),
        handle: str_or(&s["handle"], identifier).to_string(),
        access_jwt: str_or(&s["accessJwt"], "").to_string(),
        refresh_jwt: str_or(&s["refreshJwt"], "").to_string(),
    };
    if session.did.is_empty() || session.access_jwt.is_empty() {
        return Err(err("D01", "createSession returned no did / accessJwt"));
    }
    *ctx.session.borrow_mut() = Some(session.clone());
    // 表示名 / アバターは getProfile から。取れなくてもログインは成功させる。
    let me = xrpc_get(&ctx, "app.bsky.actor.getProfile", &[("actor", session.did.clone())])
        .map(|p| user_wire(&ctx.host, &p))
        .unwrap_or_else(|_| user_wire(&ctx.host, &json!({ "did": session.did, "handle": session.handle })));
    Ok(json!({
        "session": { "did": session.did, "handle": session.handle, "accessJwt": session.access_jwt, "refreshJwt": session.refresh_jwt },
        "me": me,
    }))
}

/// `call("sns.inspectServer", {host})` → `describeServer`。**認証前に呼ばれる**。
fn sns_inspect_server(args: &Value) -> Result<Value, Value> {
    let ctx = Ctx::anonymous(args);
    let d = xrpc_get(&ctx, "com.atproto.server.describeServer", &[])?;
    let mut server = json!({
        "host": ctx.host,
        "name": ctx.host,
        "software": "bluesky",
        "registrationOpen": !d["inviteCodeRequired"].as_bool().unwrap_or(false),
    });
    if let Some(t) = d["links"]["termsOfService"].as_str() {
        server["description"] = json!(t);
    }
    Ok(json!({ "server": server }))
}

fn sns_capabilities() -> Result<Value, Value> {
    Ok(json!({ "maxPostLength": MAX_POST_GRAPHEMES }))
}

/// 基本 TL。Bluesky にローカル TL は無いので `home` と Discover だけ宣言する。
fn sns_timelines() -> Result<Value, Value> {
    Ok(json!({ "timelines": [
        { "id": "home",   "label": "フォロー中" },
        { "id": "global", "label": "Discover" },
    ] }))
}

/// アクションバーの宣言 (D-075)。「いいね」は Mastodon サンプルと同じく
/// `Reaction` ジャンル(⭐)で出し、id はホストのお気に入りメニューが引く
/// `favorite` に揃える。引用はネイティブにあるので `Quote` も出す。
fn sns_actions() -> Result<Value, Value> {
    Ok(json!({ "actions": [
        { "id": "reply",    "genreTag": "Reply",    "label": "返信",     "icon": "reply",        "confirmMode": "confirm", "fow": "interstitial", "countSource": "self" },
        { "id": "repost",   "genreTag": "Repost",   "label": "リポスト", "icon": "repeat",       "doneIcon": "repeat_on", "countSource": "self" },
        { "id": "favorite", "genreTag": "Reaction", "label": "いいね",   "icon": "star_border",  "doneIcon": "star",      "countSource": "self", "emoji": STAR },
        { "id": "quote",    "genreTag": "Quote",    "label": "引用",     "icon": "format_quote", "confirmMode": "confirm", "fow": "interstitial", "countSource": "self" },
    ] }))
}

/// `call("sns.trendingTags")` → `getTrendingTopics`。失敗しても空配列
/// (トレンド欄が無いだけで TL を壊さない)。
fn sns_trending_tags(args: &Value) -> Result<Value, Value> {
    let ctx = match Ctx::authed(args) {
        Ok(c) => c,
        Err(_) => Ctx::anonymous(args),
    };
    let tags: Vec<Value> = xrpc_get(&ctx, "app.bsky.unspecced.getTrendingTopics", &[("limit", "20".into())])
        .map(|res| {
            as_list(&res["topics"])
                .iter()
                .filter_map(|t| t["topic"].as_str().map(|s| json!({ "tag": s })))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({ "tags": tags }))
}

// ── sns.*: 認証つき ──────────────────────────────────────────────

fn sns_timeline(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let kind = str_or(&args["kind"], "home");
    let limit = limit_arg(args, 30, 100);
    let cursor = cursor_arg(args);
    match kind {
        "global" | "featured" => {
            let res = xrpc_get(ctx, "app.bsky.feed.getFeed", &[("feed", DISCOVER_FEED.into()), ("limit", limit), ("cursor", cursor)])?;
            Ok(feed_page(&ctx.host, &res))
        }
        k if k.starts_with("tag:") => {
            let res = xrpc_get(ctx, "app.bsky.feed.searchPosts", &[("q", format!("#{}", &k[4..])), ("limit", limit), ("cursor", cursor)])?;
            let items: Vec<Value> = as_list(&res["posts"]).iter().map(|p| post_view_wire(&ctx.host, p)).collect();
            Ok(page(items, &res["cursor"]))
        }
        k if k.starts_with("chat:") => {
            let convo = chat_convo_for(ctx, &k[5..])?;
            let res = chat_get(ctx, "chat.bsky.convo.getMessages", &[("convoId", str_or(&convo["id"], "").into()), ("limit", limit), ("cursor", cursor)])?;
            let items: Vec<Value> = as_list(&res["messages"]).iter().map(|m| message_wire(&ctx.host, m, &convo["members"])).collect();
            Ok(page(items, &res["cursor"]))
        }
        // カスタムフィード (D-194): `feed:<at-uri>` → getFeed。
        k if k.starts_with("feed:") => {
            let res = xrpc_get(ctx, "app.bsky.feed.getFeed", &[("feed", k[5..].to_string()), ("limit", limit), ("cursor", cursor)])?;
            Ok(feed_page(&ctx.host, &res))
        }
        // リスト TL (D-195): `list:<at-uri>` → getListFeed。
        k if k.starts_with("list:") => {
            let res = xrpc_get(ctx, "app.bsky.feed.getListFeed", &[("list", k[5..].to_string()), ("limit", limit), ("cursor", cursor)])?;
            Ok(feed_page(&ctx.host, &res))
        }
        // home / local / social はすべてフォロー中 TL。
        _ => {
            let res = xrpc_get(ctx, "app.bsky.feed.getTimeline", &[("limit", limit), ("cursor", cursor)])?;
            Ok(feed_page(&ctx.host, &res))
        }
    }
}

/// リストのワイヤ (D-195): `{id: at-uri, name, memberIds: [], description?, isPublic}`(purpose が curatelist なら公開リスト)。
fn list_wire(l: &Value) -> Value {
    json!({
        "id": l["uri"],
        "name": str_or(&l["name"], ""),
        "memberIds": [],
        "description": l["description"].as_str(),
        "isPublic": str_or(&l["purpose"], "").ends_with("#curatelist"),
        "memberCount": l["listItemCount"].as_u64(),
    })
}

/// `sns.lists {}` (D-195): 自分の公開リスト(`app.bsky.graph.getLists actor=did`)。
fn sns_lists(ctx: &Ctx) -> Result<Value, Value> {
    let res = xrpc_get(ctx, "app.bsky.graph.getLists", &[("actor", ctx.did()), ("limit", "100".into())])?;
    let lists: Vec<Value> = as_list(&res["lists"]).iter().filter(|l| str_or(&l["purpose"], "").ends_with("#curatelist")).map(list_wire).collect();
    Ok(json!({ "lists": lists }))
}

/// `sns.listInfo {id}` / `sns.listMembers {id}` (D-195): `app.bsky.graph.getList list=<uri>`。
fn sns_list_info(ctx: &Ctx) -> Result<Value, Value> {
    let res = xrpc_get(ctx, "app.bsky.graph.getList", &[("list", str_or(&ctx.args["id"], "").into()), ("limit", "1".into())])?;
    Ok(json!({ "list": list_wire(&res["list"]) }))
}

fn sns_list_members(ctx: &Ctx) -> Result<Value, Value> {
    let res = xrpc_get(ctx, "app.bsky.graph.getList", &[("list", str_or(&ctx.args["id"], "").into()), ("limit", "100".into())])?;
    let users: Vec<Value> = as_list(&res["items"]).iter().map(|i| user_wire(&ctx.host, &i["subject"])).collect();
    Ok(json!({ "users": users }))
}

/// `sns.savedFeeds {}` (D-195): 保存済み / ピン留めのカスタムフィード(`getPreferences` の savedFeedsPrefV2 → `getFeedGenerators`)。
fn sns_saved_feeds(ctx: &Ctx) -> Result<Value, Value> {
    let prefs = xrpc_get(ctx, "app.bsky.actor.getPreferences", &[])?;
    let mut uris: Vec<String> = Vec::new();
    for p in as_list(&prefs["preferences"]) {
        let t = str_or(&p["$type"], "");
        if t == "app.bsky.actor.defs#savedFeedsPrefV2" {
            for item in as_list(&p["items"]) {
                if str_or(&item["type"], "") == "feed" {
                    if let Some(v) = item["value"].as_str() {
                        uris.push(v.to_string());
                    }
                }
            }
        } else if t == "app.bsky.actor.defs#savedFeedsPref" && uris.is_empty() {
            for v in as_list(&p["saved"]) {
                if let Some(u) = v.as_str() {
                    uris.push(u.to_string());
                }
            }
        }
    }
    if uris.is_empty() {
        return Ok(json!({ "feeds": [] }));
    }
    let params: Vec<(&str, String)> = uris.iter().map(|u| ("feeds", u.clone())).collect();
    let res = xrpc_get(ctx, "app.bsky.feed.getFeedGenerators", &params)?;
    let feeds: Vec<Value> = as_list(&res["feeds"])
        .iter()
        .map(|f| json!({ "uri": str_or(&f["uri"], ""), "name": str_or(&f["displayName"], ""), "description": f["description"].as_str(), "likeCount": f["likeCount"].as_i64(), "creator": f["creator"]["handle"].as_str() }))
        .collect();
    Ok(json!({ "feeds": feeds }))
}

/// `sns.saveFeed {uri, on}` (D-227): カスタムフィードを保存 / 保存から外す。`getPreferences` の
/// `savedFeedsPrefV2`(無ければ作る)の items に `{type: feed, value: uri, pinned: false, id}` を足す / 抜き、
/// `putPreferences` で書き戻す。戻りは保存後の `{feeds: [...]}`(`sns.savedFeeds` と同じワイヤ)。
fn sns_save_feed(ctx: &Ctx) -> Result<Value, Value> {
    let uri = str_or(&ctx.args["uri"], "").trim().to_string();
    if uri.is_empty() {
        return Err(err("400", "uri is required"));
    }
    let on = ctx.args["on"].as_bool().unwrap_or(true);
    let prefs = xrpc_get(ctx, "app.bsky.actor.getPreferences", &[])?;
    let mut list = as_list(&prefs["preferences"]);
    let idx = list.iter().position(|p| str_or(&p["$type"], "") == "app.bsky.actor.defs#savedFeedsPrefV2");
    let mut items: Vec<Value> = idx.map(|i| as_list(&list[i]["items"])).unwrap_or_default();
    items.retain(|it| !(str_or(&it["type"], "") == "feed" && str_or(&it["value"], "") == uri));
    if on {
        // id はレコード内で一意ならよい(lexicon は string)。時刻 + 通し番号。
        let id = format!("rasq{}{}", ctx.now().replace(|c: char| !c.is_ascii_digit(), ""), items.len());
        items.push(json!({ "type": "feed", "value": uri, "pinned": false, "id": id }));
    }
    let pref = json!({ "$type": "app.bsky.actor.defs#savedFeedsPrefV2", "items": items });
    match idx {
        Some(i) => list[i] = pref,
        None => list.push(pref),
    }
    xrpc_post(ctx, "app.bsky.actor.putPreferences", json!({ "preferences": list }))?;
    sns_saved_feeds(ctx)
}

/// `sns.feeds {query?, limit?}` (D-194): カスタムフィード(フィードジェネレーター)の検索。
/// `app.bsky.unspecced.getPopularFeedGenerators`(query 付き)→ `{feeds: [{uri, name, description?, likeCount?, creator?}]}`。
/// フィードの URL / at-uri (D-196): `at://<did|handle>/app.bsky.feed.generator/<rkey>` か
/// `https://bsky.app/profile/<did|handle>/feed/<rkey>` → (actor, rkey)。
fn parse_feed_ref(q: &str) -> Option<(String, String)> {
    let rest = if let Some(r) = q.strip_prefix("at://") {
        let mut it = r.split('/');
        let actor = it.next()?.to_string();
        let coll = it.next()?;
        let rkey = it.next()?.to_string();
        if coll != "app.bsky.feed.generator" || rkey.is_empty() { return None; }
        return Some((actor, rkey));
    } else if let Some(i) = q.find("bsky.app/profile/") {
        &q[i + "bsky.app/profile/".len()..]
    } else {
        return None;
    };
    let mut it = rest.split('/');
    let actor = it.next()?.to_string();
    if it.next()? != "feed" { return None; }
    let rkey = it.next()?.split(['?', '#']).next()?.to_string();
    if actor.is_empty() || rkey.is_empty() { return None; }
    Some((actor, rkey))
}

fn feed_wire(f: &Value) -> Value {
    json!({
        "uri": str_or(&f["uri"], ""),
        "name": str_or(&f["displayName"], ""),
        "description": f["description"].as_str(),
        "likeCount": f["likeCount"].as_i64(),
        "creator": f["creator"]["handle"].as_str(),
    })
}

fn sns_feeds(ctx: &Ctx) -> Result<Value, Value> {
    let query = str_or(&ctx.args["query"], "").trim().to_string();
    // URL / at-uri (D-196): そのフィードを 1 件返す(ハンドルは DID に解決)。
    if let Some((actor, rkey)) = parse_feed_ref(&query) {
        let did = if actor.starts_with("did:") {
            actor
        } else {
            let r = xrpc_get(ctx, "com.atproto.identity.resolveHandle", &[("handle", actor)])?;
            str_or(&r["did"], "").to_string()
        };
        let uri = format!("at://{did}/app.bsky.feed.generator/{rkey}");
        let res = xrpc_get(ctx, "app.bsky.feed.getFeedGenerator", &[("feed", uri)])?;
        return Ok(json!({ "feeds": [feed_wire(&res["view"])] }));
    }
    let limit = limit_arg(ctx.args, 20, 100);
    let mut params: Vec<(&str, String)> = vec![("limit", limit)];
    if !query.is_empty() {
        params.push(("query", query));
    }
    let res = xrpc_get(ctx, "app.bsky.unspecced.getPopularFeedGenerators", &params)?;
    let feeds: Vec<Value> = as_list(&res["feeds"]).iter().map(feed_wire).collect();
    Ok(json!({ "feeds": feeds }))
}

fn sns_post(ctx: &Ctx) -> Result<Value, Value> {
    let pv = get_post(ctx, str_or(&ctx.args["id"], ""))?;
    Ok(json!({ "post": post_view_wire(&ctx.host, &pv) }))
}

/// 返信ツリー(`replies`)を深さ優先で平坦化する。
fn flatten_replies(host: &str, replies: &Value, out: &mut Vec<Value>) {
    for r in as_list(replies) {
        if str_or(&r["$type"], "") == "app.bsky.feed.defs#threadViewPost" {
            out.push(post_view_wire(host, &r["post"]));
            flatten_replies(host, &r["replies"], out);
        }
    }
}

/// `call("sns.thread", {id})` → `getPostThread` → Thread ワイヤ。
fn sns_thread(ctx: &Ctx) -> Result<Value, Value> {
    let id = str_or(&ctx.args["id"], "").to_string();
    let res = xrpc_get(ctx, "app.bsky.feed.getPostThread", &[("uri", id.clone()), ("depth", "10".into()), ("parentHeight", "20".into())])?;
    let node = &res["thread"];
    if str_or(&node["$type"], "") != "app.bsky.feed.defs#threadViewPost" {
        return Err(err("404", format!("thread unavailable: {}", str_or(&node["$type"], "?"))));
    }
    // 親は新しい → 古いの順に鎖で繋がっている。古い順にして返す。
    let mut ancestors = vec![];
    let mut cur = &node["parent"];
    while str_or(&cur["$type"], "") == "app.bsky.feed.defs#threadViewPost" {
        ancestors.push(post_view_wire(&ctx.host, &cur["post"]));
        cur = &cur["parent"];
    }
    ancestors.reverse();
    let mut descendants = vec![];
    flatten_replies(&ctx.host, &node["replies"], &mut descendants);
    Ok(json!({ "thread": { "post": post_view_wire(&ctx.host, &node["post"]), "ancestors": ancestors, "descendants": descendants } }))
}

/// `call("sns.performAction", {actionId, postId, args})` (D-075)。
/// いいね / リポストは**レコードの作成・削除**。現在の状態は `getPosts` の
/// `viewer.like` / `viewer.repost`(自分のレコード URI)で見てトグルする。
fn sns_perform_action(ctx: &Ctx) -> Result<Value, Value> {
    let action = str_or(&ctx.args["actionId"], "");
    let post_id = str_or(&ctx.args["postId"], "");
    let inner = &ctx.args["args"];
    let (collection, viewer_key, count_key) = match action {
        "favorite" | "like" => ("app.bsky.feed.like", "like", "likeCount"),
        "repost" => ("app.bsky.feed.repost", "repost", "repostCount"),
        // 返信・引用はコンポーザーを開くだけ(投稿は sns.createPost 経由)。
        "reply" | "quote" => return Ok(json!({ "result": { "done": false, "message": "compose" } })),
        other => return Err(err("C01", format!("unsupported action: {other}"))),
    };
    let current = get_post(ctx, post_id)?;
    let existing = current["viewer"][viewer_key].as_str().map(str::to_string);
    let count = current[count_key].as_u64().unwrap_or(0);
    let undo = inner["undo"].as_bool().unwrap_or(existing.is_some());
    if undo {
        let Some(rec) = existing else {
            return Ok(json!({ "result": { "done": false, "count": count } }));
        };
        delete_record(ctx, collection, rkey_of(&rec))?;
        return Ok(json!({ "result": { "done": false, "count": count.saturating_sub(1) } }));
    }
    if existing.is_some() {
        return Ok(json!({ "result": { "done": true, "count": count } }));
    }
    let subject = json!({ "uri": current["uri"], "cid": current["cid"] });
    create_record(ctx, collection, json!({ "$type": collection, "subject": subject, "createdAt": ctx.now() }))?;
    Ok(json!({ "result": { "done": true, "count": count + 1 } }))
}

/// `sns.uploadMedia` が返した id(`{blob, alt, mimeType}` の JSON)を戻す。
fn decode_media_id(id: &Value) -> Result<Value, Value> {
    let raw = id.as_str().ok_or_else(|| err("D01", "mediaIds must be strings"))?;
    let v: Value = serde_json::from_str(raw).map_err(|_| err("D01", "mediaId is not the {blob, alt, mimeType} JSON from sns.uploadMedia"))?;
    if !v["blob"].is_object() {
        return Err(err("D01", "mediaId lacks blob"));
    }
    Ok(v)
}

/// `draft.mediaIds` → `app.bsky.embed.images` か `app.bsky.embed.video`。
fn media_embed(draft: &Value) -> Result<Option<Value>, Value> {
    let ids = as_list(&draft["mediaIds"]);
    if ids.is_empty() {
        return Ok(None);
    }
    let decoded: Vec<Value> = ids.iter().map(decode_media_id).collect::<Result<_, _>>()?;
    if let Some(video) = decoded.iter().find(|m| str_or(&m["mimeType"], "").starts_with("video/")) {
        return Ok(Some(json!({
            "$type": "app.bsky.embed.video",
            "video": video["blob"],
            "alt": str_or(&video["alt"], ""),
        })));
    }
    let images: Vec<Value> = decoded
        .iter()
        .take(4)
        .map(|m| json!({ "image": m["blob"], "alt": str_or(&m["alt"], "") }))
        .collect();
    Ok(Some(json!({ "$type": "app.bsky.embed.images", "images": images })))
}

/// `settings.compose.lang`(文字列か配列)→ `langs`。既定は日本語。
fn langs_from(settings: &Value) -> Vec<String> {
    match &settings["compose"]["lang"] {
        Value::String(s) if !s.is_empty() => vec![s.clone()],
        Value::Array(a) if !a.is_empty() => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => vec!["ja".to_string()],
    }
}

/// `call("sns.createPost", {draft})` (D-076): `app.bsky.feed.post` レコードを組む。
fn sns_create_post(ctx: &Ctx) -> Result<Value, Value> {
    let draft = &ctx.args["draft"];
    let now = ctx.now();

    // リポストは投稿ではなくレコード。NewPost が repostOfId を持って来たら振り替える。
    if let Some(repost_of) = draft["repostOfId"].as_str() {
        let subject = strong_ref(ctx, repost_of)?;
        create_record(ctx, "app.bsky.feed.repost", json!({ "$type": "app.bsky.feed.repost", "subject": subject, "createdAt": now }))?;
        return Ok(json!({ "post": post_view_wire(&ctx.host, &get_post(ctx, repost_of)?) }));
    }

    let text = str_or(&draft["text"], "");
    let count = grapheme_count(text);
    if count > MAX_POST_GRAPHEMES {
        return Err(err("B02", format!("text is {count} graphemes; Bluesky allows {MAX_POST_GRAPHEMES}")));
    }
    let mut record = json!({
        "$type": "app.bsky.feed.post",
        "text": text,
        "createdAt": now,
        "langs": langs_from(&ctx.args["settings"]),
    });
    let facets = build_facets(ctx, text);
    if !facets.is_empty() {
        record["facets"] = json!(facets);
    }
    if let Some(parent_uri) = draft["replyToId"].as_str() {
        let parent = get_post(ctx, parent_uri)?;
        let parent_ref = json!({ "uri": parent["uri"], "cid": parent["cid"] });
        // root は親が返信ならその root、そうでなければ親自身。
        let root = match &parent["record"]["reply"]["root"] {
            r if r["uri"].is_string() && r["cid"].is_string() => r.clone(),
            _ => parent_ref.clone(),
        };
        record["reply"] = json!({ "root": root, "parent": parent_ref });
    }
    let media = media_embed(draft)?;
    let quote = match draft["quoteOfId"].as_str() {
        Some(q) => Some(json!({ "$type": "app.bsky.embed.record", "record": strong_ref(ctx, q)? })),
        None => None,
    };
    match (media, quote) {
        (Some(m), Some(q)) => record["embed"] = json!({ "$type": "app.bsky.embed.recordWithMedia", "record": q, "media": m }),
        (Some(m), None) => record["embed"] = m,
        (None, Some(q)) => record["embed"] = q,
        (None, None) => {}
    }
    if let Some(cw) = draft["cw"].as_str() {
        if !cw.is_empty() {
            // Bluesky に CW は無い。閲覧注意のセルフラベルとして残す。
            record["labels"] = json!({ "$type": "com.atproto.label.defs#selfLabels", "values": [{ "val": "graphic-media" }] });
        }
    }

    let created = create_record(ctx, "app.bsky.feed.post", record.clone())?;
    let uri = str_or(&created["uri"], "").to_string();
    remember_cid(&uri, &created["cid"]);
    // AppView への反映が遅れて getPosts が空のことがある。その場合は手元の
    // レコードから合成して返す(タイルはすぐ出したい)。
    let wire = match get_post(ctx, &uri) {
        Ok(pv) => post_view_wire(&ctx.host, &pv),
        Err(_) => {
            let me = ctx.session.borrow().clone().unwrap_or(Session { did: String::new(), handle: String::new(), access_jwt: String::new(), refresh_jwt: String::new() });
            let view = json!({ "uri": uri, "cid": created["cid"], "author": { "did": me.did, "handle": me.handle }, "indexedAt": now });
            post_wire_at(&ctx.host, &view, &record, &[], 0)
        }
    };
    Ok(json!({ "post": wire }))
}

fn sns_delete_post(ctx: &Ctx) -> Result<Value, Value> {
    let id = str_or(&ctx.args["id"], "");
    let uri = id.split('#').next().unwrap_or(id);
    if repo_of(uri) != ctx.did() {
        return Err(err("403", "can only delete your own posts"));
    }
    delete_record(ctx, "app.bsky.feed.post", rkey_of(uri))?;
    Ok(json!({}))
}

fn sns_me(ctx: &Ctx) -> Result<Value, Value> {
    let p = xrpc_get(ctx, "app.bsky.actor.getProfile", &[("actor", ctx.did())])?;
    Ok(json!({ "user": user_wire(&ctx.host, &p) }))
}

/// `call("sns.user", {id | handle+userHost?})`。Bluesky のハンドルは完全な
/// ドメインなので `userHost` は使わない(`getProfile` は did でもハンドルでも引ける)。
fn sns_user(ctx: &Ctx) -> Result<Value, Value> {
    let actor = ctx.args["id"].as_str().or(ctx.args["handle"].as_str()).unwrap_or("");
    if actor.is_empty() {
        return Err(err("D01", "id or handle required"));
    }
    let p = xrpc_get(ctx, "app.bsky.actor.getProfile", &[("actor", actor.to_string())])?;
    Ok(json!({ "profile": profile_wire(&ctx.host, &p) }))
}

/// `getProfiles`(25 件ずつ)。
fn get_profiles(ctx: &Ctx, ids: &[String]) -> Result<Vec<Value>, Value> {
    let mut out = vec![];
    for chunk in ids.chunks(BATCH) {
        let params: Vec<(&str, String)> = chunk.iter().map(|u| ("actors", u.clone())).collect();
        let res = xrpc_get(ctx, "app.bsky.actor.getProfiles", &params)?;
        out.extend(as_list(&res["profiles"]));
    }
    Ok(out)
}

fn ids_arg(args: &Value) -> Vec<String> {
    as_list(&args["ids"]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

fn sns_users(ctx: &Ctx) -> Result<Value, Value> {
    let profiles: Vec<Value> = get_profiles(ctx, &ids_arg(ctx.args))?.iter().map(|p| profile_wire(&ctx.host, p)).collect();
    Ok(json!({ "profiles": profiles }))
}

fn sns_relationships(ctx: &Ctx) -> Result<Value, Value> {
    let ids = ids_arg(ctx.args);
    if ids.is_empty() {
        return Ok(json!({ "states": {} }));
    }
    let mut states = Map::new();
    for p in get_profiles(ctx, &ids)? {
        if let Some(did) = p["did"].as_str() {
            states.insert(did.to_string(), follow_state_wire(&p["viewer"]));
        }
    }
    Ok(json!({ "states": states }))
}

/// `call("sns.userPosts", {id, filter, limit, cursor?})` → `getAuthorFeed`。
/// 返信 / 引用の振り分け (D-069) はサーバー側フィルタ + 手元の絞り込み。
fn sns_user_posts(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let filter = str_or(&args["filter"], "posts");
    let api_filter = match filter {
        "withReplies" | "replies" => "posts_with_replies",
        "media" => "posts_with_media",
        _ => "posts_no_replies",
    };
    let res = xrpc_get(
        ctx,
        "app.bsky.feed.getAuthorFeed",
        &[("actor", str_or(&args["id"], "").into()), ("filter", api_filter.into()), ("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))],
    )?;
    let items: Vec<Value> = as_list(&res["feed"])
        .iter()
        .filter(|item| {
            let is_repost = str_or(&item["reason"]["$type"], "") == "app.bsky.feed.defs#reasonRepost";
            match filter {
                "replies" => !is_repost && item["post"]["record"]["reply"].is_object(),
                "quotes" => {
                    !is_repost
                        && matches!(str_or(&item["post"]["embed"]["$type"], ""), "app.bsky.embed.record#view" | "app.bsky.embed.recordWithMedia#view")
                }
                _ => true,
            }
        })
        .map(|i| feed_item_wire(&ctx.host, i))
        .collect();
    Ok(page(items, &res["cursor"]))
}

/// `call("sns.reactions", {postId, …})` → `getLikes`。いいね = ⭐。
fn sns_reactions(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let res = xrpc_get(ctx, "app.bsky.feed.getLikes", &[("uri", str_or(&args["postId"], "").into()), ("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    let items: Vec<Value> = as_list(&res["likes"])
        .iter()
        .map(|l| json!({ "id": l["actor"]["did"], "user": user_wire(&ctx.host, &l["actor"]), "reaction": STAR, "createdAt": l["createdAt"].as_str().or(l["indexedAt"].as_str()) }))
        .collect();
    Ok(page(items, &res["cursor"]))
}

fn sns_reposters(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let res = xrpc_get(ctx, "app.bsky.feed.getRepostedBy", &[("uri", str_or(&args["postId"], "").into()), ("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    Ok(users_page(&ctx.host, &res["repostedBy"], &res["cursor"]))
}

fn sns_quotes(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let res = xrpc_get(ctx, "app.bsky.feed.getQuotes", &[("uri", str_or(&args["postId"], "").into()), ("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    let items: Vec<Value> = as_list(&res["posts"]).iter().map(|p| post_view_wire(&ctx.host, p)).collect();
    Ok(page(items, &res["cursor"]))
}

/// `call("sns.reactedPosts")` (D-103): 自分がいいねした投稿。
fn sns_reacted_posts(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let res = xrpc_get(ctx, "app.bsky.feed.getActorLikes", &[("actor", ctx.did()), ("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    Ok(feed_page(&ctx.host, &res))
}

/// 通知の `reason` → RASQ の `NotificationKind`。
fn notification_kind(reason: &str) -> &'static str {
    match reason {
        "like" | "like-via-repost" => "reaction",
        "repost" | "repost-via-repost" => "repost",
        "reply" => "reply",
        "mention" => "mention",
        "quote" => "quote",
        "follow" => "follow",
        _ => "other",
    }
}

/// `call("sns.notifications", {limit, cursor?})` → `listNotifications`。
///
/// 対象投稿は `getPosts` でまとめて引く(25 件ずつ)。同じ投稿への連続した
/// いいね / リポストは 1 件に畳み、`actors` と `reactionsByActor` に全員を載せる。
fn sns_notifications(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let res = xrpc_get(ctx, "app.bsky.notification.listNotifications", &[("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    let list = as_list(&res["notifications"]);

    // 対象投稿: いいね / リポストは reasonSubject、返信 / メンション / 引用は通知の uri 自身。
    let mut uris: Vec<String> = vec![];
    for n in &list {
        let target = match str_or(&n["reason"], "") {
            "like" | "repost" | "like-via-repost" | "repost-via-repost" => n["reasonSubject"].as_str(),
            "reply" | "mention" | "quote" => n["uri"].as_str(),
            _ => None,
        };
        if let Some(u) = target {
            if !uris.iter().any(|x| x == u) {
                uris.push(u.to_string());
            }
        }
    }
    let posts: HashMap<String, Value> = get_posts(ctx, &uris)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p["uri"].as_str().map(|u| (u.to_string(), post_view_wire(&ctx.host, &p))))
        .collect();

    let mut items: Vec<Value> = vec![];
    for n in &list {
        let reason = str_or(&n["reason"], "").to_string();
        let kind = notification_kind(&reason);
        let subject = str_or(&n["reasonSubject"], "").to_string();
        let actor_did = str_or(&n["author"]["did"], "").to_string();
        let groupable = matches!(kind, "reaction" | "repost") && !subject.is_empty();
        if groupable {
            if let Some(last) = items.last_mut() {
                if last["_reason"] == json!(reason) && last["_subject"] == json!(subject) {
                    if let Some(actors) = last["actors"].as_array_mut() {
                        actors.push(user_wire(&ctx.host, &n["author"]));
                    }
                    if kind == "reaction" {
                        last["reactionsByActor"][&actor_did] = json!(STAR);
                    }
                    if n["isRead"].as_bool() == Some(false) {
                        last["read"] = json!(false);
                    }
                    continue;
                }
            }
        }
        let mut wire = json!({
            "id": n["uri"],
            "kind": kind,
            "createdAt": n["indexedAt"],
            "read": n["isRead"].as_bool().unwrap_or(false),
            "actors": [user_wire(&ctx.host, &n["author"])],
            "_reason": reason,
            "_subject": subject,
        });
        let post = match kind {
            "reaction" | "repost" => posts.get(&subject).cloned(),
            "reply" | "mention" | "quote" => posts.get(str_or(&n["uri"], "")).cloned().or_else(|| {
                // AppView 未反映なら通知に載っているレコードから合成する。
                Some(post_wire_at(&ctx.host, &json!({ "uri": n["uri"], "cid": n["cid"], "author": n["author"], "indexedAt": n["indexedAt"] }), &n["record"], &[], 0))
            }),
            _ => None,
        };
        if let Some(p) = post {
            wire["post"] = p;
        }
        if kind == "reaction" {
            wire["reaction"] = json!(STAR);
            wire["reactionsByActor"] = json!({ actor_did: STAR });
        }
        items.push(wire);
    }
    for item in &mut items {
        if let Some(o) = item.as_object_mut() {
            o.remove("_reason");
            o.remove("_subject");
        }
    }
    Ok(page(items, &res["cursor"]))
}

fn sns_mark_notifications_read(ctx: &Ctx) -> Result<Value, Value> {
    xrpc_post(ctx, "app.bsky.notification.updateSeen", json!({ "seenAt": ctx.now() }))?;
    Ok(json!({}))
}

/// `call("sns.setRelation", {id, relation, on})`。フォロー / ブロックは
/// `app.bsky.graph.*` レコードの作成・削除、ミュートは専用 XRPC。
/// 戻りは書き込み後にプロフィールを読み直した状態。
fn sns_set_relation(ctx: &Ctx) -> Result<Value, Value> {
    let id = str_or(&ctx.args["id"], "").to_string();
    let on = ctx.args["on"].as_bool().unwrap_or(false);
    let relation = str_or(&ctx.args["relation"], "");
    let before = xrpc_get(ctx, "app.bsky.actor.getProfile", &[("actor", id.clone())])?;
    let viewer = &before["viewer"];
    let now = ctx.now();
    match (relation, on) {
        ("following", true) => {
            if !viewer["following"].is_string() {
                create_record(ctx, "app.bsky.graph.follow", json!({ "$type": "app.bsky.graph.follow", "subject": id, "createdAt": now }))?;
            }
        }
        ("following", false) => {
            if let Some(uri) = viewer["following"].as_str() {
                delete_record(ctx, "app.bsky.graph.follow", rkey_of(uri))?;
            }
        }
        ("muted", true) => {
            xrpc_post(ctx, "app.bsky.graph.muteActor", json!({ "actor": id }))?;
        }
        ("muted", false) => {
            xrpc_post(ctx, "app.bsky.graph.unmuteActor", json!({ "actor": id }))?;
        }
        ("blocked", true) => {
            if !viewer["blocking"].is_string() {
                create_record(ctx, "app.bsky.graph.block", json!({ "$type": "app.bsky.graph.block", "subject": id, "createdAt": now }))?;
            }
        }
        ("blocked", false) => {
            if let Some(uri) = viewer["blocking"].as_str() {
                delete_record(ctx, "app.bsky.graph.block", rkey_of(uri))?;
            }
        }
        (other, _) => return Err(err("C01", format!("unknown relation {other:?}"))),
    }
    let after = xrpc_get(ctx, "app.bsky.actor.getProfile", &[("actor", id)])?;
    Ok(json!({ "state": follow_state_wire(&after["viewer"]) }))
}

fn sns_follows(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let following = args["direction"].as_str() == Some("following");
    let (nsid, key) = if following { ("app.bsky.graph.getFollows", "follows") } else { ("app.bsky.graph.getFollowers", "followers") };
    let res = xrpc_get(ctx, nsid, &[("actor", str_or(&args["id"], "").into()), ("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    Ok(users_page(&ctx.host, &res[key], &res["cursor"]))
}

fn sns_user_list(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    let (nsid, key) = match str_or(&args["kind"], "") {
        "muted" => ("app.bsky.graph.getMutes", "mutes"),
        "blocked" => ("app.bsky.graph.getBlocks", "blocks"),
        // Bluesky にフォロー申請は無い。
        other => return Err(err("C01", format!("unknown user list {other:?}"))),
    };
    let res = xrpc_get(ctx, nsid, &[("limit", limit_arg(args, 30, 100)), ("cursor", cursor_arg(args))])?;
    Ok(users_page(&ctx.host, &res[key], &res["cursor"]))
}

fn sns_report_user(ctx: &Ctx) -> Result<Value, Value> {
    let body = json!({
        "reasonType": "com.atproto.moderation.defs#reasonOther",
        "reason": str_or(&ctx.args["comment"], ""),
        "subject": { "$type": "com.atproto.admin.defs#repoRef", "did": ctx.args["id"] },
    });
    xrpc(ctx, "POST", "com.atproto.moderation.createReport", &[], Some(Body::Json(body)), Some(MOD_PROXY))?;
    Ok(json!({}))
}

fn sns_search(ctx: &Ctx) -> Result<Value, Value> {
    let args = ctx.args;
    // ハッシュタグの検索(コンポーザーの `#` の補完、rasq-client D-475)は Bluesky の API に無い。C01 でホストは候補を出さない。
    if args["kind"].as_str() == Some("hashtags") {
        return Err(unsupported("sns.search kind=hashtags"));
    }
    let params = [("q", str_or(&args["q"], "").to_string()), ("limit", limit_arg(args, 25, 100)), ("cursor", cursor_arg(args))];
    if args["kind"].as_str() == Some("users") {
        let res = xrpc_get(ctx, "app.bsky.actor.searchActors", &params)?;
        return Ok(users_page(&ctx.host, &res["actors"], &res["cursor"]));
    }
    let res = xrpc_get(ctx, "app.bsky.feed.searchPosts", &params)?;
    let items: Vec<Value> = as_list(&res["posts"]).iter().map(|p| post_view_wire(&ctx.host, p)).collect();
    Ok(page(items, &res["cursor"]))
}

/// `call("sns.uploadMedia", {bytesB64, filename, mimeType?, alt?})` → `{media}`。
///
/// `uploadBlob` は生バイトを受けるので base64 をそのまま `net.fetch` の
/// `{b64}` に載せる。返る blob ref は投稿レコードに埋め込むまで**どこにも
/// 紐付いていない**(未参照 blob は数分で GC される)。ホストは `media.id`
/// を `mediaIds` で返してくるだけなので、id に `{blob, alt, mimeType}` の
/// JSON を詰めて `sns.createPost` で戻す。URL は無い(空文字)。
fn sns_upload_media(ctx: &Ctx) -> Result<Value, Value> {
    let Some(b64) = ctx.args["bytesB64"].as_str() else {
        return Err(err("D01", "bytesB64 required"));
    };
    let filename = str_or(&ctx.args["filename"], "upload.bin");
    let mime = str_or(&ctx.args["mimeType"], "application/octet-stream").to_string();
    let alt = str_or(&ctx.args["alt"], "");
    let res = xrpc(ctx, "POST", "com.atproto.repo.uploadBlob", &[], Some(Body::Raw { b64: b64.to_string(), mime: mime.clone() }), None)?;
    if !res["blob"].is_object() {
        return Err(err("D01", "uploadBlob returned no blob"));
    }
    let media_type = if mime.starts_with("video/") { "video" } else if mime.starts_with("image/") { "image" } else { "file" };
    let id = json!({ "blob": res["blob"], "alt": alt, "mimeType": mime }).to_string();
    Ok(json!({ "media": {
        "id": id,
        "type": media_type,
        "url": "",
        "alt": alt,
        "fileName": filename,
    } }))
}

// ── DM(Bluesky チャット) ────────────────────────────────────────

fn chat_get(ctx: &Ctx, nsid: &str, params: &[(&str, String)]) -> Result<Value, Value> {
    xrpc(ctx, "GET", nsid, params, None, Some(CHAT_PROXY))
}

fn chat_post(ctx: &Ctx, nsid: &str, body: Value) -> Result<Value, Value> {
    xrpc(ctx, "POST", nsid, &[], Some(Body::Json(body)), Some(CHAT_PROXY))
}

/// 相手の did → 会話(`getConvoForMembers`。無ければ作られる)。
fn chat_convo_for(ctx: &Ctx, peer_did: &str) -> Result<Value, Value> {
    let res = chat_get(ctx, "chat.bsky.convo.getConvoForMembers", &[("members", peer_did.to_string())])?;
    if res["convo"].is_object() {
        Ok(res["convo"].clone())
    } else {
        Err(err("404", "no conversation with that user"))
    }
}

/// チャットメッセージ → 最小の Post ワイヤ(本文 + 送信者 + 時刻)。
fn message_wire(host: &str, m: &Value, members: &Value) -> Value {
    let sender_did = str_or(&m["sender"]["did"], "");
    let author = as_list(members)
        .into_iter()
        .find(|p| p["did"].as_str() == Some(sender_did))
        .unwrap_or_else(|| json!({ "did": sender_did, "handle": sender_did }));
    json!({
        "id": m["id"],
        "author": user_wire(host, &author),
        "createdAt": m["sentAt"],
        "body": str_or(&m["text"], ""),
        "bodyFormat": "plain",
        "media": [],
        "visibility": "specified",
    })
}

/// `call("sns.dmRooms")` (CONTRACT §7)。チャットが使えないアカウント /
/// PDS では C01 を返し、ホストが DM 欄を隠す。
fn sns_dm_rooms(ctx: &Ctx) -> Result<Value, Value> {
    let res = chat_get(ctx, "chat.bsky.convo.listConvos", &[("limit", "50".into())]).map_err(|e| err("C01", format!("chat unavailable: {}", str_or(&e["message"], ""))))?;
    let me = ctx.did();
    let rooms: Vec<Value> = as_list(&res["convos"])
        .iter()
        .filter_map(|c| {
            let members = as_list(&c["members"]);
            let peer = members.iter().find(|p| p["did"].as_str() != Some(me.as_str())).or(members.first())?;
            let mut room = json!({
                "id": c["id"],
                "kind": "chat",
                "peer": user_wire(&ctx.host, peer),
                "unreadCount": c["unreadCount"].as_u64().unwrap_or(0),
            });
            if str_or(&c["lastMessage"]["$type"], "") == "chat.bsky.convo.defs#messageView" {
                room["lastPost"] = message_wire(&ctx.host, &c["lastMessage"], &c["members"]);
            }
            Some(room)
        })
        .collect();
    Ok(json!({ "rooms": rooms }))
}

/// `call("sns.sendDm", {roomKind, peerId, draft})` → `sendMessage`。
fn sns_send_dm(ctx: &Ctx) -> Result<Value, Value> {
    let peer = str_or(&ctx.args["peerId"], "");
    let text = str_or(&ctx.args["draft"]["text"], "");
    if peer.is_empty() || text.is_empty() {
        return Err(err("D01", "peerId and draft.text required"));
    }
    let convo = chat_convo_for(ctx, peer)?;
    let mut message = json!({ "text": text });
    let facets = build_facets(ctx, text);
    if !facets.is_empty() {
        message["facets"] = json!(facets);
    }
    let sent = chat_post(ctx, "chat.bsky.convo.sendMessage", json!({ "convoId": convo["id"], "message": message }))?;
    Ok(json!({ "post": message_wire(&ctx.host, &sent, &convo["members"]) }))
}

// ── boundary の call ディスパッチ (PROTOCOL §3.2) ──────────────────

/// `call("sns.<fn>", args)` の本体。wasm の `call` export とテストが共用する。
pub fn dispatch(fn_name: &str, args: &Value) -> Result<Value, Value> {
    // 認証前でも答えるもの。
    match fn_name {
        "sns.login" => return sns_login(args),
        "sns.inspectServer" => return sns_inspect_server(args),
        "sns.capabilities" => return sns_capabilities(),
        "sns.timelines" => return sns_timelines(),
        "sns.actions" => return sns_actions(),
        "sns.emojis" => return Ok(json!({ "emojis": [] })),
        "sns.trendingTags" => return sns_trending_tags(args),
        _ => {}
    }
    let ctx = Ctx::authed(args);
    // 認証が要るのに token が無いときだけ 401。未知の fn は token に関係なく C01。
    macro_rules! authed {
        ($f:expr) => {
            match &ctx {
                Ok(c) => $f(c),
                Err(e) => Err(e.clone()),
            }
        };
    }
    match fn_name {
        "sns.timeline" => authed!(sns_timeline),
        "sns.feeds" => authed!(sns_feeds),
        "sns.savedFeeds" => authed!(sns_saved_feeds),
        "sns.saveFeed" => authed!(sns_save_feed),
        "sns.lists" => authed!(sns_lists),
        "sns.listInfo" => authed!(sns_list_info),
        "sns.listMembers" => authed!(sns_list_members),
        "sns.post" => authed!(sns_post),
        "sns.thread" => authed!(sns_thread),
        "sns.performAction" => authed!(sns_perform_action),
        "sns.createPost" => authed!(sns_create_post),
        "sns.deletePost" => authed!(sns_delete_post),
        "sns.me" => authed!(sns_me),
        "sns.user" => authed!(sns_user),
        "sns.users" => authed!(sns_users),
        "sns.userPosts" => authed!(sns_user_posts),
        "sns.reactions" => authed!(sns_reactions),
        "sns.reposters" => authed!(sns_reposters),
        "sns.quotes" => authed!(sns_quotes),
        "sns.reactedPosts" => authed!(sns_reacted_posts),
        "sns.notifications" => authed!(sns_notifications),
        "sns.markNotificationsRead" => authed!(sns_mark_notifications_read),
        "sns.relationships" => authed!(sns_relationships),
        "sns.setRelation" => authed!(sns_set_relation),
        "sns.follows" => authed!(sns_follows),
        "sns.userList" => authed!(sns_user_list),
        "sns.reportUser" => authed!(sns_report_user),
        "sns.search" => authed!(sns_search),
        "sns.uploadMedia" => authed!(sns_upload_media),
        "sns.dmRooms" => authed!(sns_dm_rooms),
        "sns.sendDm" => authed!(sns_send_dm),
        // Bluesky に無い / まだ写していないもの。C01 でホストが該当 UI を隠す。
        "sns.favorites" => Err(unsupported("bookmarks (Bluesky has none)")),
        "sns.updateProfile" => Err(unsupported("updateProfile (not yet)")),
        "sns.resolveFollowRequest" => Err(unsupported("follow requests (Bluesky has none)")),
        "sns.createList" | "sns.updateListMember" => Err(unsupported("list editing (not yet)")),
        "sns.clips" | "sns.clipInfo" | "sns.clipNotes" | "sns.createClip" | "sns.deleteClip" | "sns.updateClipNote" => Err(unsupported("clips (Misskey only)")),
        "sns.userPlays" => Err(unsupported("plays (Misskey only)")),
        "sns.applySettings" | "sns.settingsRead" | "sns.settingsWrite" => Err(unsupported("settings pages")),
        other => Err(err("C01", format!("unsupported fn: {other}"))),
    }
}

#[no_mangle]
pub extern "C" fn call(fn_ptr: u32, fn_len: u32, args_ptr: u32, args_len: u32, call_id_ptr: u32, call_id_len: u32) -> u64 {
    let fn_name = read_str(fn_ptr, fn_len);
    let args = read_str(args_ptr, args_len);
    let call_id = read_str(call_id_ptr, call_id_len);
    let args_value: Value = serde_json::from_str(&args).unwrap_or(Value::Null);

    let outcome = dispatch(&fn_name, &args_value);
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
    /// テストは直列に(`CID_CACHE` と host hook がプロセス共有の static なので、並列だと
    /// 別テストの cid / ログが混ざる。CI で `Some("c")` != `Some("cid1")` になった)。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        *super::CID_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let _ = super::take_host_log();
        g
    }
    use super::*;

    const DID: &str = "did:plc:alice";
    const POST: &str = "at://did:plc:bob/app.bsky.feed.post/3kabc";
    const NOW: &str = "2026-09-11T12:00:00.000Z";

    fn token() -> String {
        json!({ "did": DID, "handle": "alice.bsky.social", "accessJwt": "access-1", "refreshJwt": "refresh-1" }).to_string()
    }

    fn args(extra: Value) -> Value {
        let mut a = json!({ "host": "bsky.social", "token": token(), "now": NOW, "settings": {} });
        if let Value::Object(m) = extra {
            for (k, v) in m {
                a[k] = v;
            }
        }
        a
    }

    /// `net.fetch` を XRPC 単位でモックする。`handler(method, nsid, request)` は
    /// `Ok(json)` で 200、`Err((status, json))` でエラー応答。`storage.*` は
    /// `stored` を返す(`storage.get` の `value`)。
    fn mock_xrpc<F>(stored: Value, mut handler: F)
    where
        F: FnMut(&str, &str, &Value) -> Result<Value, (u64, Value)> + 'static,
    {
        set_host_hook(move |fn_name, a| match fn_name {
            "net.fetch" => {
                let url = a["url"].as_str().unwrap_or("");
                let nsid = url.split("/xrpc/").nth(1).unwrap_or("").split('?').next().unwrap_or("").to_string();
                let method = a["method"].as_str().unwrap_or("").to_string();
                let (status, body) = match handler(&method, &nsid, &a) {
                    Ok(b) => (200, b),
                    Err((s, b)) => (s, b),
                };
                json!({ "status": status, "headers": {}, "bodyText": body.to_string() })
            }
            "storage.get" => json!({ "value": stored.clone() }),
            _ => json!({}),
        });
    }

    fn fetches() -> Vec<Value> {
        take_host_log().into_iter().filter(|(f, _)| f == "net.fetch").map(|(_, a)| a).collect()
    }

    fn query_of(req: &Value) -> String {
        req["url"].as_str().unwrap_or("").split('?').nth(1).unwrap_or("").to_string()
    }

    fn body_of(req: &Value) -> Value {
        serde_json::from_str(req["body"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
    }

    fn author(did: &str, handle: &str, name: &str) -> Value {
        json!({ "did": did, "handle": handle, "displayName": name, "avatar": format!("https://cdn.test/{handle}.jpg") })
    }

    fn post_view(uri: &str, cid: &str, text: &str) -> Value {
        json!({
            "uri": uri, "cid": cid,
            "author": author("did:plc:bob", "bob.bsky.social", "ボブ"),
            "record": { "$type": "app.bsky.feed.post", "text": text, "createdAt": "2026-09-10T00:00:00.000Z" },
            "replyCount": 1, "repostCount": 2, "likeCount": 3, "quoteCount": 4,
            "indexedAt": "2026-09-10T00:00:01.000Z",
            "viewer": {},
        })
    }

    // ── ログイン ─────────────────────────────────────────────

    #[test]
    fn login_returns_session_json_and_me() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |method, nsid, req| match (method, nsid) {
            ("POST", "com.atproto.server.createSession") => {
                let b = body_of(req);
                assert_eq!(b["identifier"], "alice.bsky.social");
                assert_eq!(b["password"], "app-pass");
                assert!(req["headers"]["Authorization"].is_null(), "login must be anonymous");
                Ok(json!({ "did": DID, "handle": "alice.bsky.social", "accessJwt": "acc", "refreshJwt": "ref" }))
            }
            ("GET", "app.bsky.actor.getProfile") => {
                assert_eq!(req["headers"]["Authorization"], "Bearer acc", "profile is fetched with the new access token");
                Ok(author(DID, "alice.bsky.social", "アリス"))
            }
            other => panic!("unexpected {other:?}"),
        });
        let v = dispatch("sns.login", &json!({ "host": "bsky.social", "identifier": " alice.bsky.social ", "password": "app-pass" })).unwrap();
        assert_eq!(v["session"], json!({ "did": DID, "handle": "alice.bsky.social", "accessJwt": "acc", "refreshJwt": "ref" }));
        assert_eq!(v["me"]["id"], DID);
        assert_eq!(v["me"]["kind"], "bluesky");
        assert_eq!(v["me"]["instance"], "bsky.social");
        assert_eq!(v["me"]["displayName"], "アリス");
        assert_eq!(v["me"]["avatarUrl"], "https://cdn.test/alice.bsky.social.jpg");
        // 送信先は PDS の xrpc。
        let f = fetches();
        assert_eq!(f[0]["url"], "https://bsky.social/xrpc/com.atproto.server.createSession");
        assert_eq!(f[0]["headers"]["Content-Type"], "application/json");
    }

    #[test]
    fn login_with_wrong_password_is_401() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, _, _| Err((401, json!({ "error": "AuthenticationRequired", "message": "Invalid identifier or password" }))));
        let e = dispatch("sns.login", &json!({ "host": "bsky.social", "identifier": "alice", "password": "nope" })).unwrap_err();
        assert_eq!(e["code"], "401");
        assert_eq!(e["message"], "AuthenticationRequired: Invalid identifier or password");
    }

    #[test]
    fn missing_token_is_401_and_unknown_fn_is_c01() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, _, _| Ok(json!({})));
        let e = dispatch("sns.timeline", &json!({ "host": "bsky.social" })).unwrap_err();
        assert_eq!(e["code"], "401");
        let e = dispatch("sns.somethingNew", &args(json!({}))).unwrap_err();
        assert_eq!(e["code"], "C01");
        // 明示的に未対応のもの。
        assert_eq!(dispatch("sns.favorites", &args(json!({}))).unwrap_err()["code"], "C01");
        assert_eq!(dispatch("sns.createList", &args(json!({ "name": "x" }))).unwrap_err()["code"], "C01");
    }

    #[test]
    fn inspect_server_maps_describe_server() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, _| {
            assert_eq!(nsid, "com.atproto.server.describeServer");
            Ok(json!({ "did": "did:web:bsky.social", "inviteCodeRequired": false, "links": { "termsOfService": "https://bsky.social/about/support/tos" } }))
        });
        let v = dispatch("sns.inspectServer", &json!({ "host": "bsky.social" })).unwrap();
        assert_eq!(v["server"]["software"], "bluesky");
        assert_eq!(v["server"]["name"], "bsky.social");
        assert_eq!(v["server"]["registrationOpen"], true);
        assert_eq!(v["server"]["description"], "https://bsky.social/about/support/tos");
    }

    // ── TL / ワイヤ ──────────────────────────────────────────

    #[test]
    fn timeline_maps_posts_reposts_embeds_and_likes() {
        let _g = test_lock();
        let quoted = json!({
            "$type": "app.bsky.embed.record#viewRecord",
            "uri": "at://did:plc:carol/app.bsky.feed.post/3kq", "cid": "cidq",
            "author": author("did:plc:carol", "carol.test", "キャロル"),
            "value": { "$type": "app.bsky.feed.post", "text": "引用される側", "createdAt": "2026-09-09T00:00:00.000Z" },
            "likeCount": 0,
        });
        let mut liked = post_view(POST, "cid1", "こんにちは");
        liked["viewer"] = json!({ "like": format!("at://{DID}/app.bsky.feed.like/3klike") });
        liked["embed"] = json!({
            "$type": "app.bsky.embed.recordWithMedia#view",
            "media": { "$type": "app.bsky.embed.images#view", "images": [
                { "thumb": "https://cdn.test/t.jpg", "fullsize": "https://cdn.test/f.jpg", "alt": "写真", "aspectRatio": { "width": 400, "height": 300 } }
            ] },
            "record": { "record": quoted },
        });
        liked["record"]["reply"] = json!({ "root": { "uri": "at://root", "cid": "c" }, "parent": { "uri": "at://parent", "cid": "c" } });
        let video = {
            let mut p = post_view("at://did:plc:bob/app.bsky.feed.post/3kvid", "cidv", "動画");
            p["embed"] = json!({ "$type": "app.bsky.embed.video#view", "cid": "blobcid", "playlist": "https://video.test/p.m3u8", "thumbnail": "https://video.test/t.jpg" });
            p["likeCount"] = json!(0);
            p
        };
        let feed = json!({ "feed": [
            { "post": liked },
            { "post": post_view("at://did:plc:bob/app.bsky.feed.post/3kre", "cid2", "リポストされる側"),
              "reason": { "$type": "app.bsky.feed.defs#reasonRepost", "by": author("did:plc:dan", "dan.test", "ダン"), "indexedAt": "2026-09-11T01:00:00.000Z" } },
            { "post": video },
        ], "cursor": "next-1" });
        mock_xrpc(Value::Null, move |method, nsid, req| {
            assert_eq!((method, nsid), ("GET", "app.bsky.feed.getTimeline"));
            assert_eq!(query_of(req), "limit=20&cursor=c0");
            assert_eq!(req["headers"]["Authorization"], "Bearer access-1");
            Ok(feed.clone())
        });
        let v = dispatch("sns.timeline", &args(json!({ "kind": "home", "limit": 20, "cursor": "c0" }))).unwrap();
        let items = v["page"]["items"].as_array().unwrap();
        assert_eq!(v["page"]["nextCursor"], "next-1");
        assert_eq!(items.len(), 3);

        let p = &items[0];
        assert_eq!(p["id"], POST);
        assert_eq!(p["url"], "https://bsky.app/profile/bob.bsky.social/post/3kabc");
        assert_eq!(p["body"], "こんにちは");
        assert_eq!(p["bodyFormat"], "plain");
        assert_eq!(p["createdAt"], "2026-09-10T00:00:00.000Z");
        assert_eq!(p["author"], json!({ "id": "did:plc:bob", "kind": "bluesky", "instance": "bsky.social", "handle": "bob.bsky.social", "displayName": "ボブ", "avatarUrl": "https://cdn.test/bob.bsky.social.jpg" }));
        assert_eq!(p["counts"], json!({ "reply": 1, "repost": 2, "favorite": 3, "quote": 4 }));
        assert_eq!(p["reactions"], json!({ "⭐": 3 }));
        assert_eq!(p["myReaction"], "⭐");
        assert_eq!(p["replyToId"], "at://parent");
        assert_eq!(p["media"][0], json!({ "id": "https://cdn.test/f.jpg", "type": "image", "url": "https://cdn.test/f.jpg", "thumbnailUrl": "https://cdn.test/t.jpg", "alt": "写真", "width": 400, "height": 300 }));
        assert_eq!(p["quote"]["id"], "at://did:plc:carol/app.bsky.feed.post/3kq");
        assert_eq!(p["quote"]["body"], "引用される側");
        assert_eq!(p["quote"]["author"]["displayName"], "キャロル");
        assert!(p["quote"]["reactions"].is_null(), "0 likes → no reactions map");

        // リポストは中身へ展開 (D-100/D-112)。
        let r = &items[1];
        assert_eq!(r["id"], "at://did:plc:bob/app.bsky.feed.post/3kre#repost-by-did:plc:dan");
        assert_eq!(r["repostTargetId"], "at://did:plc:bob/app.bsky.feed.post/3kre");
        assert_eq!(r["repostedBy"]["displayName"], "ダン");
        assert_eq!(r["createdAt"], "2026-09-11T01:00:00.000Z");
        assert_eq!(r["body"], "リポストされる側");
        assert_eq!(r["author"]["displayName"], "ボブ");

        let vd = &items[2];
        assert_eq!(vd["media"][0]["type"], "video");
        assert_eq!(vd["media"][0]["url"], "https://video.test/p.m3u8");
        assert_eq!(vd["media"][0]["thumbnailUrl"], "https://video.test/t.jpg");
        assert!(vd["myReaction"].is_null());

        // cid は strong ref 用にキャッシュされている。
        assert_eq!(cached_cid(POST).as_deref(), Some("cid1"));
        assert_eq!(cached_cid("at://did:plc:carol/app.bsky.feed.post/3kq").as_deref(), Some("cidq"));
    }

    #[test]
    fn global_timeline_is_the_discover_feed() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.feed.getFeed");
            assert!(query_of(req).starts_with(&format!("feed={}", urlencode(DISCOVER_FEED))));
            Ok(json!({ "feed": [] }))
        });
        let v = dispatch("sns.timeline", &args(json!({ "kind": "global" }))).unwrap();
        assert_eq!(v["page"]["items"], json!([]));
        assert!(v["page"]["nextCursor"].is_null());
    }

    #[test]
    fn custom_feed_kind_and_feed_search() {
        let _g = test_lock();
        // `feed:<uri>` → getFeed(そのフィード)。
        mock_xrpc(Value::Null, |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.feed.getFeed");
            assert!(query_of(req).starts_with(&format!("feed={}", urlencode("at://did:plc:x/app.bsky.feed.generator/cats"))));
            Ok(json!({ "feed": [], "cursor": "c2" }))
        });
        let v = dispatch("sns.timeline", &args(json!({ "kind": "feed:at://did:plc:x/app.bsky.feed.generator/cats" }))).unwrap();
        assert_eq!(v["page"]["nextCursor"], "c2");
        // `sns.feeds {query}` → getPopularFeedGenerators(query 付き)→ {uri, name, description, likeCount, creator}。
        mock_xrpc(Value::Null, |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.unspecced.getPopularFeedGenerators");
            assert!(query_of(req).contains("query=cat"));
            Ok(json!({ "feeds": [
                { "uri": "at://did:plc:x/app.bsky.feed.generator/cats", "displayName": "Cats", "description": "ねこ", "likeCount": 12, "creator": { "handle": "cat.bsky.social" } }
            ] }))
        });
        let v = dispatch("sns.feeds", &args(json!({ "query": "cat" }))).unwrap();
        assert_eq!(v["feeds"][0]["uri"], "at://did:plc:x/app.bsky.feed.generator/cats");
        assert_eq!(v["feeds"][0]["name"], "Cats");
        assert_eq!(v["feeds"][0]["likeCount"], 12);
        assert_eq!(v["feeds"][0]["creator"], "cat.bsky.social");
        // query 無しは人気順(パラメーターに query を付けない)。
        mock_xrpc(Value::Null, |_, _, req| {
            assert!(!query_of(req).contains("query="));
            Ok(json!({ "feeds": [] }))
        });
        assert_eq!(dispatch("sns.feeds", &args(json!({}))).unwrap()["feeds"], json!([]));
    }

    #[test]
    fn public_lists_members_list_feed_and_saved_feeds() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, req| match nsid {
            "app.bsky.graph.getLists" => {
                assert!(query_of(req).contains(&format!("actor={}", urlencode(DID))));
                Ok(json!({ "lists": [
                    { "uri": "at://did:plc:me/app.bsky.graph.list/1", "name": "友達", "purpose": "app.bsky.graph.defs#curatelist", "listItemCount": 2, "description": "ともだち" },
                    { "uri": "at://did:plc:me/app.bsky.graph.list/2", "name": "ミュート", "purpose": "app.bsky.graph.defs#modlist" }
                ] }))
            }
            "app.bsky.graph.getList" => Ok(json!({ "list": { "uri": "at://did:plc:me/app.bsky.graph.list/1", "name": "友達", "purpose": "app.bsky.graph.defs#curatelist" }, "items": [ { "uri": "x", "subject": { "did": "did:plc:bob", "handle": "bob.bsky.social", "displayName": "ボブ" } } ] })),
            "app.bsky.feed.getListFeed" => {
                assert!(query_of(req).starts_with(&format!("list={}", urlencode("at://did:plc:me/app.bsky.graph.list/1"))));
                Ok(json!({ "feed": [], "cursor": "n" }))
            }
            "app.bsky.actor.getPreferences" => Ok(json!({ "preferences": [ { "$type": "app.bsky.actor.defs#savedFeedsPrefV2", "items": [ { "type": "feed", "value": "at://did:plc:x/app.bsky.feed.generator/cats", "pinned": true }, { "type": "timeline", "value": "following" } ] } ] })),
            "app.bsky.feed.getFeedGenerators" => {
                assert!(query_of(req).contains("feeds="));
                Ok(json!({ "feeds": [ { "uri": "at://did:plc:x/app.bsky.feed.generator/cats", "displayName": "Cats", "likeCount": 3, "creator": { "handle": "cat.bsky.social" } } ] }))
            }
            other => panic!("unexpected {other}"),
        });
        let v = dispatch("sns.lists", &args(json!({}))).unwrap();
        assert_eq!(v["lists"].as_array().unwrap().len(), 1); // モデレーションリストは出さない
        assert_eq!(v["lists"][0]["id"], "at://did:plc:me/app.bsky.graph.list/1");
        assert_eq!(v["lists"][0]["name"], "友達");
        assert_eq!(v["lists"][0]["isPublic"], true);
        let m = dispatch("sns.listMembers", &args(json!({ "id": "at://did:plc:me/app.bsky.graph.list/1" }))).unwrap();
        assert_eq!(m["users"][0]["handle"], "bob.bsky.social");
        let i = dispatch("sns.listInfo", &args(json!({ "id": "at://did:plc:me/app.bsky.graph.list/1" }))).unwrap();
        assert_eq!(i["list"]["name"], "友達");
        let t = dispatch("sns.timeline", &args(json!({ "kind": "list:at://did:plc:me/app.bsky.graph.list/1" }))).unwrap();
        assert_eq!(t["page"]["nextCursor"], "n");
        let f = dispatch("sns.savedFeeds", &args(json!({}))).unwrap();
        assert_eq!(f["feeds"][0]["name"], "Cats");
        assert_eq!(f["feeds"][0]["creator"], "cat.bsky.social");
    }

    #[test]
    fn feed_url_or_at_uri_resolves_to_one_feed() {
        let _g = test_lock();
        assert_eq!(parse_feed_ref("at://did:plc:x/app.bsky.feed.generator/cats"), Some(("did:plc:x".into(), "cats".into())));
        assert_eq!(parse_feed_ref("https://bsky.app/profile/cat.bsky.social/feed/cats?x=1"), Some(("cat.bsky.social".into(), "cats".into())));
        assert_eq!(parse_feed_ref("ねこ"), None);
        mock_xrpc(Value::Null, |_, nsid, req| match nsid {
            "com.atproto.identity.resolveHandle" => {
                assert!(query_of(req).contains("handle=cat.bsky.social"));
                Ok(json!({ "did": "did:plc:x" }))
            }
            "app.bsky.feed.getFeedGenerator" => {
                assert!(query_of(req).contains(&urlencode("at://did:plc:x/app.bsky.feed.generator/cats")));
                Ok(json!({ "view": { "uri": "at://did:plc:x/app.bsky.feed.generator/cats", "displayName": "Cats", "likeCount": 5, "creator": { "handle": "cat.bsky.social" } } }))
            }
            other => panic!("unexpected {other}"),
        });
        let v = dispatch("sns.feeds", &args(json!({ "query": "https://bsky.app/profile/cat.bsky.social/feed/cats" }))).unwrap();
        assert_eq!(v["feeds"].as_array().unwrap().len(), 1);
        assert_eq!(v["feeds"][0]["name"], "Cats");
    }

    #[test]
    fn thread_flattens_ancestors_oldest_first_and_descendants_depth_first() {
        let _g = test_lock();
        let tvp = |uri: &str, text: &str, parent: Value, replies: Vec<Value>| {
            let mut n = json!({ "$type": "app.bsky.feed.defs#threadViewPost", "post": post_view(uri, "c", text), "replies": replies });
            if !parent.is_null() {
                n["parent"] = parent;
            }
            n
        };
        let thread = tvp(
            POST,
            "本体",
            tvp("at://p1", "親", tvp("at://p0", "祖父", Value::Null, vec![]), vec![]),
            vec![
                tvp("at://r1", "返信1", Value::Null, vec![tvp("at://r1a", "返信1の返信", Value::Null, vec![])]),
                json!({ "$type": "app.bsky.feed.defs#blockedPost", "uri": "at://blocked" }),
                tvp("at://r2", "返信2", Value::Null, vec![]),
            ],
        );
        mock_xrpc(Value::Null, move |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.feed.getPostThread");
            assert_eq!(query_of(req), format!("uri={}&depth=10&parentHeight=20", urlencode(POST)));
            Ok(json!({ "thread": thread.clone() }))
        });
        let v = dispatch("sns.thread", &args(json!({ "id": POST }))).unwrap();
        let t = &v["thread"];
        assert_eq!(t["post"]["body"], "本体");
        let ancestors: Vec<&str> = t["ancestors"].as_array().unwrap().iter().map(|p| p["body"].as_str().unwrap()).collect();
        assert_eq!(ancestors, vec!["祖父", "親"]);
        let descendants: Vec<&str> = t["descendants"].as_array().unwrap().iter().map(|p| p["body"].as_str().unwrap()).collect();
        assert_eq!(descendants, vec!["返信1", "返信1の返信", "返信2"]);
    }

    // ── 投稿 ─────────────────────────────────────────────────

    #[test]
    fn facets_use_utf8_byte_offsets() {
        let _g = test_lock();
        let text = "こんにちは https://example.com/道, #ロケット と @bob.test へ。";
        let (facets, mentions) = scan_facets(text);
        assert_eq!(facets.len(), 2);
        let link = &facets[0];
        let s = link["index"]["byteStart"].as_u64().unwrap() as usize;
        let e = link["index"]["byteEnd"].as_u64().unwrap() as usize;
        assert_eq!(&text[s..e], "https://example.com/道", "trailing comma trimmed, multibyte path kept");
        assert_eq!(s, "こんにちは ".len());
        assert_eq!(link["features"][0]["uri"], "https://example.com/道");
        let tag = &facets[1];
        let s = tag["index"]["byteStart"].as_u64().unwrap() as usize;
        let e = tag["index"]["byteEnd"].as_u64().unwrap() as usize;
        assert_eq!(&text[s..e], "#ロケット");
        assert_eq!(tag["features"][0]["tag"], "ロケット");
        assert_eq!(mentions.len(), 1);
        assert_eq!(&text[mentions[0].0..mentions[0].1], "@bob.test");
        assert_eq!(mentions[0].2, "bob.test");
        // メール風 / 数字だけのタグ / 単語内の @ は拾わない。
        let (f, m) = scan_facets("mail me@example.com #2024 a@b");
        assert!(f.is_empty(), "{f:?}");
        assert!(m.is_empty(), "{m:?}");
    }

    #[test]
    fn create_post_builds_facets_reply_quote_and_images() {
        let _g = test_lock();
        let parent_uri = "at://did:plc:bob/app.bsky.feed.post/3kparent";
        let quote_uri = "at://did:plc:carol/app.bsky.feed.post/3kquote";
        let mut parent = post_view(parent_uri, "cidparent", "親");
        parent["record"]["reply"] = json!({ "root": { "uri": "at://root", "cid": "cidroot" }, "parent": { "uri": "at://mid", "cid": "cidmid" } });
        let quoted = post_view(quote_uri, "cidquote", "引用先");
        mock_xrpc(Value::Null, move |method, nsid, req| match (method, nsid) {
            ("GET", "com.atproto.identity.resolveHandle") => {
                assert_eq!(query_of(req), "handle=bob.test");
                Ok(json!({ "did": "did:plc:bob" }))
            }
            ("GET", "app.bsky.feed.getPosts") => {
                let q = query_of(req);
                if q.contains("3kparent") {
                    Ok(json!({ "posts": [parent.clone()] }))
                } else if q.contains("3kquote") {
                    Ok(json!({ "posts": [quoted.clone()] }))
                } else if q.contains("3knew") {
                    Ok(json!({ "posts": [post_view("at://did:plc:alice/app.bsky.feed.post/3knew", "cidnew", "投稿済み")] }))
                } else {
                    panic!("unexpected getPosts {q}")
                }
            }
            ("POST", "com.atproto.repo.createRecord") => {
                let b = body_of(req);
                assert_eq!(b["repo"], DID);
                assert_eq!(b["collection"], "app.bsky.feed.post");
                let r = &b["record"];
                assert_eq!(r["$type"], "app.bsky.feed.post");
                assert_eq!(r["text"], "@bob.test 見て #ロケット https://example.com");
                assert_eq!(r["createdAt"], NOW);
                assert_eq!(r["langs"], json!(["en"]));
                // facets: mention (0..9), tag, link — byteStart 順。
                let facets = r["facets"].as_array().unwrap();
                assert_eq!(facets.len(), 3);
                assert_eq!(facets[0]["features"][0]["$type"], "app.bsky.richtext.facet#mention");
                assert_eq!(facets[0]["features"][0]["did"], "did:plc:bob");
                assert_eq!(facets[0]["index"], json!({ "byteStart": 0, "byteEnd": 9 }));
                assert_eq!(facets[1]["features"][0]["tag"], "ロケット");
                assert_eq!(facets[1]["index"], json!({ "byteStart": "@bob.test 見て ".len(), "byteEnd": "@bob.test 見て #ロケット".len() }));
                assert_eq!(facets[2]["features"][0]["uri"], "https://example.com");
                // reply: parent の root を引き継ぐ。
                assert_eq!(r["reply"], json!({ "root": { "uri": "at://root", "cid": "cidroot" }, "parent": { "uri": parent_uri, "cid": "cidparent" } }));
                // embed: 画像 + 引用 → recordWithMedia。
                assert_eq!(r["embed"]["$type"], "app.bsky.embed.recordWithMedia");
                assert_eq!(r["embed"]["record"], json!({ "$type": "app.bsky.embed.record", "record": { "uri": quote_uri, "cid": "cidquote" } }));
                assert_eq!(r["embed"]["media"]["$type"], "app.bsky.embed.images");
                assert_eq!(r["embed"]["media"]["images"][0]["alt"], "猫");
                assert_eq!(r["embed"]["media"]["images"][0]["image"]["ref"]["$link"], "bafyimg");
                Ok(json!({ "uri": "at://did:plc:alice/app.bsky.feed.post/3knew", "cid": "cidnew" }))
            }
            other => panic!("unexpected {other:?}"),
        });
        let media_id = json!({ "blob": { "$type": "blob", "ref": { "$link": "bafyimg" }, "mimeType": "image/jpeg", "size": 3 }, "alt": "猫", "mimeType": "image/jpeg" }).to_string();
        let v = dispatch(
            "sns.createPost",
            &args(json!({
                "settings": { "compose": { "lang": "en" } },
                "draft": { "text": "@bob.test 見て #ロケット https://example.com", "replyToId": parent_uri, "quoteOfId": quote_uri, "mediaIds": [media_id] },
            })),
        )
        .unwrap();
        assert_eq!(v["post"]["id"], "at://did:plc:alice/app.bsky.feed.post/3knew");
        assert_eq!(v["post"]["body"], "投稿済み");
    }

    #[test]
    fn create_post_rejects_more_than_300_graphemes_and_defaults_to_japanese() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |method, nsid, req| match (method, nsid) {
            ("POST", "com.atproto.repo.createRecord") => {
                let r = &body_of(req)["record"];
                assert_eq!(r["langs"], json!(["ja"]));
                assert!(r.get("facets").is_none());
                assert!(r.get("reply").is_none());
                assert!(r.get("embed").is_none());
                Ok(json!({ "uri": "at://did:plc:alice/app.bsky.feed.post/3kplain", "cid": "c" }))
            }
            // AppView 未反映 → 手元のレコードから合成する経路。
            ("GET", "app.bsky.feed.getPosts") => Ok(json!({ "posts": [] })),
            other => panic!("unexpected {other:?}"),
        });
        // 301 グラフェム(家族絵文字は 1 グラフェム = 7 コードポイント)。
        let long = "👨‍👩‍👧‍👦".repeat(301);
        assert_eq!(grapheme_count(&long), 301);
        let e = dispatch("sns.createPost", &args(json!({ "draft": { "text": long } }))).unwrap_err();
        assert_eq!(e["code"], "B02");
        let ok = "あ".repeat(300);
        let v = dispatch("sns.createPost", &args(json!({ "draft": { "text": ok } }))).unwrap();
        assert_eq!(v["post"]["id"], "at://did:plc:alice/app.bsky.feed.post/3kplain");
        assert_eq!(v["post"]["author"]["handle"], "alice.bsky.social");
        assert_eq!(v["post"]["createdAt"], NOW);
    }

    #[test]
    fn like_toggles_between_create_and_delete_record() {
        let _g = test_lock();
        let liked = std::rc::Rc::new(std::cell::Cell::new(false));
        let l2 = liked.clone();
        mock_xrpc(Value::Null, move |method, nsid, req| match (method, nsid) {
            ("GET", "app.bsky.feed.getPosts") => {
                let mut p = post_view(POST, "cid1", "x");
                if l2.get() {
                    p["viewer"] = json!({ "like": format!("at://{DID}/app.bsky.feed.like/3klike") });
                    p["likeCount"] = json!(4);
                }
                Ok(json!({ "posts": [p] }))
            }
            ("POST", "com.atproto.repo.createRecord") => {
                let b = body_of(req);
                assert_eq!(b["collection"], "app.bsky.feed.like");
                assert_eq!(b["record"], json!({ "$type": "app.bsky.feed.like", "subject": { "uri": POST, "cid": "cid1" }, "createdAt": NOW }));
                l2.set(true);
                Ok(json!({ "uri": format!("at://{DID}/app.bsky.feed.like/3klike"), "cid": "x" }))
            }
            ("POST", "com.atproto.repo.deleteRecord") => {
                assert_eq!(body_of(req), json!({ "repo": DID, "collection": "app.bsky.feed.like", "rkey": "3klike" }));
                l2.set(false);
                Ok(json!({}))
            }
            other => panic!("unexpected {other:?}"),
        });
        let v = dispatch("sns.performAction", &args(json!({ "actionId": "favorite", "postId": POST, "args": {} }))).unwrap();
        assert_eq!(v["result"], json!({ "done": true, "count": 4 }));
        assert!(liked.get());
        let v = dispatch("sns.performAction", &args(json!({ "actionId": "favorite", "postId": POST, "args": {} }))).unwrap();
        assert_eq!(v["result"], json!({ "done": false, "count": 3 }));
        assert!(!liked.get());
        // undo 明示は現状に関わらず削除方向(未いいねなら何もしない)。
        let v = dispatch("sns.performAction", &args(json!({ "actionId": "favorite", "postId": POST, "args": { "undo": true } }))).unwrap();
        assert_eq!(v["result"]["done"], false);
        // 返信はコンポーザーを開くだけ。
        let v = dispatch("sns.performAction", &args(json!({ "actionId": "reply", "postId": POST, "args": {} }))).unwrap();
        assert_eq!(v["result"]["message"], "compose");
    }

    // ── セッション更新 ───────────────────────────────────────

    #[test]
    fn expired_token_refreshes_once_and_persists_the_new_pair() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |method, nsid, req| match (method, nsid) {
            ("GET", "app.bsky.actor.getProfile") => {
                if req["headers"]["Authorization"] == "Bearer access-1" {
                    Err((400, json!({ "error": "ExpiredToken", "message": "Token has expired" })))
                } else {
                    assert_eq!(req["headers"]["Authorization"], "Bearer access-2");
                    Ok(author(DID, "alice.bsky.social", "アリス"))
                }
            }
            ("POST", "com.atproto.server.refreshSession") => {
                assert_eq!(req["headers"]["Authorization"], "Bearer refresh-1", "refresh uses the refresh token");
                Ok(json!({ "did": DID, "handle": "alice.bsky.social", "accessJwt": "access-2", "refreshJwt": "refresh-2" }))
            }
            other => panic!("unexpected {other:?}"),
        });
        let v = dispatch("sns.me", &args(json!({}))).unwrap();
        assert_eq!(v["user"]["displayName"], "アリス");
        let log = take_host_log();
        let calls: Vec<&str> = log.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(calls, vec!["storage.get", "net.fetch", "net.fetch", "storage.set", "net.fetch"]);
        let set = &log[3].1;
        assert_eq!(set["ns"], "safe");
        assert_eq!(set["key"], format!("session:{DID}"));
        assert_eq!(set["value"], json!({ "accessJwt": "access-2", "refreshJwt": "refresh-2" }));
    }

    #[test]
    fn refresh_failure_surfaces_as_401_without_looping() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, _| match nsid {
            "com.atproto.server.refreshSession" => Err((400, json!({ "error": "ExpiredToken", "message": "refresh expired" }))),
            _ => Err((400, json!({ "error": "ExpiredToken", "message": "Token has expired" }))),
        });
        let e = dispatch("sns.me", &args(json!({}))).unwrap_err();
        assert_eq!(e["code"], "401");
        assert_eq!(fetches().len(), 2, "one call + one refresh attempt, no retry storm");
    }

    #[test]
    fn stored_session_pair_is_preferred_over_the_host_token() {
        let _g = test_lock();
        mock_xrpc(json!({ "accessJwt": "stored-access", "refreshJwt": "stored-refresh" }), |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.actor.getProfile");
            assert_eq!(req["headers"]["Authorization"], "Bearer stored-access");
            Ok(author(DID, "alice.bsky.social", "アリス"))
        });
        dispatch("sns.me", &args(json!({}))).unwrap();
        let get = take_host_log().into_iter().find(|(f, _)| f == "storage.get").unwrap().1;
        assert_eq!(get, json!({ "ns": "safe", "key": format!("session:{DID}") }));
    }

    // ── 通知 / 関係 ──────────────────────────────────────────

    #[test]
    fn notifications_group_consecutive_likes_and_attach_subject_posts() {
        let _g = test_lock();
        let n = |uri: &str, reason: &str, did: &str, subject: Option<&str>, read: bool| {
            let mut v = json!({ "uri": uri, "cid": "c", "author": author(did, &format!("{}.test", &did[8..]), &did[8..]), "reason": reason, "isRead": read, "indexedAt": "2026-09-11T00:00:00.000Z", "record": {} });
            if let Some(s) = subject {
                v["reasonSubject"] = json!(s);
            }
            v
        };
        let list = json!({ "notifications": [
            n("at://did:plc:bob/app.bsky.feed.like/1", "like", "did:plc:bob", Some(POST), true),
            n("at://did:plc:carol/app.bsky.feed.like/2", "like", "did:plc:carol", Some(POST), false),
            n("at://did:plc:dan/app.bsky.feed.repost/3", "repost", "did:plc:dan", Some(POST), false),
            n("at://did:plc:erin/app.bsky.feed.post/4", "reply", "did:plc:erin", Some(POST), false),
            n("at://did:plc:frank/app.bsky.graph.follow/5", "follow", "did:plc:frank", None, false),
            n("at://did:plc:gus/app.bsky.feed.like/6", "like", "did:plc:gus", Some(POST), false),
        ], "cursor": "n-next" });
        mock_xrpc(Value::Null, move |_, nsid, req| match nsid {
            "app.bsky.notification.listNotifications" => Ok(list.clone()),
            "app.bsky.feed.getPosts" => {
                // 対象投稿は 1 回でまとめて引く(重複 uri は 1 つ)。
                let q = query_of(req);
                assert_eq!(q, format!("uris={}&uris={}", urlencode(POST), urlencode("at://did:plc:erin/app.bsky.feed.post/4")));
                let mut reply = post_view("at://did:plc:erin/app.bsky.feed.post/4", "c4", "返信本文");
                reply["author"] = author("did:plc:erin", "erin.test", "erin");
                Ok(json!({ "posts": [post_view(POST, "cid1", "元投稿"), reply] }))
            }
            other => panic!("unexpected {other}"),
        });
        let v = dispatch("sns.notifications", &args(json!({ "limit": 30 }))).unwrap();
        let items = v["page"]["items"].as_array().unwrap();
        assert_eq!(v["page"]["nextCursor"], "n-next");
        assert_eq!(items.len(), 5, "bob+carol collapse; gus is separated by other kinds");
        let g = &items[0];
        assert_eq!(g["kind"], "reaction");
        assert_eq!(g["reaction"], "⭐");
        assert_eq!(g["actors"].as_array().unwrap().len(), 2);
        assert_eq!(g["reactionsByActor"], json!({ "did:plc:bob": "⭐", "did:plc:carol": "⭐" }));
        assert_eq!(g["read"], false, "unread if any member is unread");
        assert_eq!(g["post"]["body"], "元投稿");
        assert!(g.get("_reason").is_none());
        assert_eq!(items[1]["kind"], "repost");
        assert_eq!(items[1]["post"]["id"], POST);
        assert_eq!(items[2]["kind"], "reply");
        assert_eq!(items[2]["post"]["body"], "返信本文");
        assert_eq!(items[3]["kind"], "follow");
        assert!(items[3]["post"].is_null());
        assert_eq!(items[4]["actors"][0]["id"], "did:plc:gus");
        assert_eq!(fetches().len(), 2);
    }

    #[test]
    fn set_relation_follows_and_unfollows_via_graph_records() {
        let _g = test_lock();
        let following = std::rc::Rc::new(std::cell::Cell::new(false));
        let f2 = following.clone();
        mock_xrpc(Value::Null, move |method, nsid, req| match (method, nsid) {
            ("GET", "app.bsky.actor.getProfile") => {
                let mut p = author("did:plc:bob", "bob.test", "ボブ");
                p["viewer"] = if f2.get() { json!({ "following": format!("at://{DID}/app.bsky.graph.follow/3kf"), "followedBy": format!("at://did:plc:bob/app.bsky.graph.follow/x"), "muted": false }) } else { json!({ "muted": false }) };
                Ok(p)
            }
            ("POST", "com.atproto.repo.createRecord") => {
                let b = body_of(req);
                assert_eq!(b["collection"], "app.bsky.graph.follow");
                assert_eq!(b["record"], json!({ "$type": "app.bsky.graph.follow", "subject": "did:plc:bob", "createdAt": NOW }));
                f2.set(true);
                Ok(json!({ "uri": format!("at://{DID}/app.bsky.graph.follow/3kf"), "cid": "c" }))
            }
            ("POST", "com.atproto.repo.deleteRecord") => {
                assert_eq!(body_of(req), json!({ "repo": DID, "collection": "app.bsky.graph.follow", "rkey": "3kf" }));
                f2.set(false);
                Ok(json!({}))
            }
            other => panic!("unexpected {other:?}"),
        });
        let v = dispatch("sns.setRelation", &args(json!({ "id": "did:plc:bob", "relation": "following", "on": true }))).unwrap();
        assert_eq!(v["state"], json!({ "following": true, "followedBy": true, "requested": false, "muted": false, "blocked": false, "blockedBy": false }));
        let v = dispatch("sns.setRelation", &args(json!({ "id": "did:plc:bob", "relation": "following", "on": false }))).unwrap();
        assert_eq!(v["state"]["following"], false);
        assert!(!following.get());
    }

    #[test]
    fn relationships_and_users_batch_get_profiles() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.actor.getProfiles");
            assert_eq!(query_of(req), "actors=did%3Aplc%3Abob&actors=did%3Aplc%3Acarol");
            let mut bob = author("did:plc:bob", "bob.test", "ボブ");
            bob["viewer"] = json!({ "muted": true, "blocking": format!("at://{DID}/app.bsky.graph.block/1") });
            Ok(json!({ "profiles": [bob, author("did:plc:carol", "carol.test", "")] }))
        });
        let v = dispatch("sns.relationships", &args(json!({ "ids": ["did:plc:bob", "did:plc:carol"] }))).unwrap();
        assert_eq!(v["states"]["did:plc:bob"]["muted"], true);
        assert_eq!(v["states"]["did:plc:bob"]["blocked"], true);
        assert_eq!(v["states"]["did:plc:carol"]["following"], false);
        let v = dispatch("sns.users", &args(json!({ "ids": ["did:plc:bob", "did:plc:carol"] }))).unwrap();
        assert_eq!(v["profiles"][1]["user"]["displayName"], "carol.test", "empty displayName falls back to handle");
        assert_eq!(v["profiles"][0]["profileUrl"], "https://bsky.app/profile/bob.test");
    }

    // ── メディア / DM / その他 ───────────────────────────────

    #[test]
    fn upload_media_returns_the_blob_as_a_json_id() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |method, nsid, req| {
            assert_eq!((method, nsid), ("POST", "com.atproto.repo.uploadBlob"));
            assert_eq!(req["headers"]["Content-Type"], "image/png");
            assert_eq!(req["body"], json!({ "b64": "AAEC" }));
            Ok(json!({ "blob": { "$type": "blob", "ref": { "$link": "bafyblob" }, "mimeType": "image/png", "size": 3 } }))
        });
        let v = dispatch("sns.uploadMedia", &args(json!({ "bytesB64": "AAEC", "filename": "a.png", "mimeType": "image/png", "alt": "説明" }))).unwrap();
        let m = &v["media"];
        assert_eq!(m["type"], "image");
        assert_eq!(m["url"], "");
        assert_eq!(m["fileName"], "a.png");
        let id: Value = serde_json::from_str(m["id"].as_str().unwrap()).unwrap();
        assert_eq!(id, json!({ "blob": { "$type": "blob", "ref": { "$link": "bafyblob" }, "mimeType": "image/png", "size": 3 }, "alt": "説明", "mimeType": "image/png" }));
        // その id は createPost の mediaIds でそのまま戻せる。
        let decoded = decode_media_id(&m["id"]).unwrap();
        assert_eq!(decoded["blob"]["ref"]["$link"], "bafyblob");
    }

    #[test]
    fn dm_rooms_use_the_chat_proxy_and_degrade_to_c01() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, req| {
            assert_eq!(nsid, "chat.bsky.convo.listConvos");
            assert_eq!(req["headers"]["atproto-proxy"], CHAT_PROXY);
            Ok(json!({ "convos": [{
                "id": "convo1", "unreadCount": 2,
                "members": [author(DID, "alice.bsky.social", "アリス"), author("did:plc:bob", "bob.test", "ボブ")],
                "lastMessage": { "$type": "chat.bsky.convo.defs#messageView", "id": "m1", "text": "やあ", "sender": { "did": "did:plc:bob" }, "sentAt": "2026-09-11T00:00:00.000Z" },
            }] }))
        });
        let v = dispatch("sns.dmRooms", &args(json!({}))).unwrap();
        let r = &v["rooms"][0];
        assert_eq!(r["id"], "convo1");
        assert_eq!(r["kind"], "chat");
        assert_eq!(r["peer"]["id"], "did:plc:bob");
        assert_eq!(r["unreadCount"], 2);
        assert_eq!(r["lastPost"]["body"], "やあ");
        assert_eq!(r["lastPost"]["author"]["displayName"], "ボブ");

        mock_xrpc(Value::Null, |_, _, _| Err((403, json!({ "error": "Forbidden" }))));
        assert_eq!(dispatch("sns.dmRooms", &args(json!({}))).unwrap_err()["code"], "C01");
    }

    #[test]
    fn trending_tags_never_fail() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, _, _| Err((500, json!({}))));
        assert_eq!(dispatch("sns.trendingTags", &args(json!({}))).unwrap(), json!({ "tags": [] }));
        mock_xrpc(Value::Null, |_, _, _| Ok(json!({ "topics": [{ "topic": "ロケット", "link": "/x" }] })));
        assert_eq!(dispatch("sns.trendingTags", &args(json!({}))).unwrap()["tags"][0]["tag"], "ロケット");
    }

    #[test]
    fn user_posts_filters_replies_and_quotes_locally() {
        let _g = test_lock();
        mock_xrpc(Value::Null, |_, nsid, req| {
            assert_eq!(nsid, "app.bsky.feed.getAuthorFeed");
            let q = query_of(req);
            let mut reply = post_view("at://r", "c", "返信");
            reply["record"]["reply"] = json!({ "parent": { "uri": "at://p", "cid": "c" }, "root": { "uri": "at://p", "cid": "c" } });
            let mut quote = post_view("at://q", "c", "引用");
            quote["embed"] = json!({ "$type": "app.bsky.embed.record#view", "record": { "$type": "app.bsky.embed.record#viewNotFound", "uri": "at://gone" } });
            let plain = post_view("at://n", "c", "普通");
            let filter = q.split('&').find(|p| p.starts_with("filter=")).unwrap_or("").to_string();
            Ok(json!({ "feed": [{ "post": reply }, { "post": quote }, { "post": plain }], "cursor": filter }))
        });
        let bodies = |filter: &str| -> (Vec<String>, String) {
            let v = dispatch("sns.userPosts", &args(json!({ "id": "did:plc:bob", "filter": filter }))).unwrap();
            (v["page"]["items"].as_array().unwrap().iter().map(|p| p["body"].as_str().unwrap().to_string()).collect(), v["page"]["nextCursor"].as_str().unwrap().to_string())
        };
        assert_eq!(bodies("replies"), (vec!["返信".to_string()], "filter=posts_with_replies".to_string()));
        let (q, f) = bodies("quotes");
        assert_eq!(q, vec!["引用".to_string()]);
        assert_eq!(f, "filter=posts_no_replies");
        assert_eq!(bodies("media").1, "filter=posts_with_media");
        assert_eq!(bodies("posts").0.len(), 3);
    }

    #[test]
    fn query_strings_encode_and_drop_empty_values() {
        let _g = test_lock();
        assert_eq!(query_string(&[("limit", "20".into()), ("cursor", String::new())]), "?limit=20");
        assert_eq!(query_string(&[("q", "a b&c".into())]), "?q=a%20b%26c");
        assert_eq!(query_string(&[("uris", "a".into()), ("uris", "b".into())]), "?uris=a&uris=b");
        assert_eq!(rkey_of("at://did:plc:x/app.bsky.feed.post/3k"), "3k");
        assert_eq!(repo_of("at://did:plc:x/app.bsky.feed.post/3k"), "did:plc:x");
        assert!(is_handle("alice.bsky.social"));
        assert!(!is_handle("alice"));
        assert!(!is_handle("alice.b1"));
    }
}
