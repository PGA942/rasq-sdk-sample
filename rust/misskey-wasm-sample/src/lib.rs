//! Misskey WASM plugin sample for RASQ M3 (PROTOCOL.md draft v0.1).
//!
//! Emits `snapshot` / `patch` messages and answers `call` messages, all as
//! `rasq.boundary/1` JSON. Since the host-function boundary landed, this is
//! no longer a hardcoded demo: `call("timeline")` fetches a real Misskey
//! timeline through the host's `net.fetch` and rebuilds the RRL tree from
//! the notes — the ported version of the rasq-client home-TL display.
//!
//! Exported functions return a packed `(ptr << 32) | len` u64 so a single
//! i64 return value can describe a buffer in wasm32 linear memory.

use serde_json::{json, Value};
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const SCOPE: &str = "rasq.official.misskey:misskey:misskey.io:sample:0.1.0:tl-home";

#[link(wasm_import_module = "rasq")]
extern "C" {
    /// Host function import (PROTOCOL.md §4.1): request JSON in, packed
    /// `(ptr << 32) | len` response location out. Synchronous v1.
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

/// The plugin's live tree (rev + RRL root). `snapshot()` serialises it;
/// `call("timeline")` replaces it.
static STATE: Mutex<Option<(u64, Value)>> = Mutex::new(None);

/// Last `fowClosed` notification (`{id, reason, result?}`) — queried back by
/// the host/tests via `call("lastFowClosed")` (D-064 FOW surface demo).
static LAST_FOW_CLOSED: Mutex<Value> = Mutex::new(Value::Null);

/// Last `lifecycle` call (`{kind, surface?}`) — queried via
/// `call("lastLifecycle")` (D-072).
static LAST_LIFECYCLE: Mutex<Value> = Mutex::new(Value::Null);

/// FOW surface 更新デモ (D-073): +1 ボタンのカウンターと、ホストが
/// `patch()` で引き取る保留パッチ(surface 指定)。
static FOW_COUNT: AtomicU64 = AtomicU64::new(0);
static PENDING_PATCH: Mutex<Value> = Mutex::new(Value::Null);

fn with_state<R>(f: impl FnOnce(&mut (u64, Value)) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(|| (1, initial_root()));
    f(state)
}

/// Calls a host function (`rasq.hostfn/1`) and returns its parsed response.
fn host_call(fn_name: &str, args: Value) -> Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
    let bytes = request.as_bytes();
    let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
    let ptr = (packed >> 32) as u32;
    let len = (packed & 0xFFFF_FFFF) as u32;
    // The host wrote the response via our exported `alloc`; reclaim it here.
    let response = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
    serde_json::from_slice(&response).unwrap_or(Value::Null)
}

fn pack(mut buf: Vec<u8>) -> u64 {
    // `dealloc` reconstructs this buffer as `Vec::from_raw_parts(ptr, len, len)`,
    // so capacity must equal `len` exactly.
    buf.shrink_to_fit();
    let len = buf.len() as u64;
    let ptr = buf.as_ptr() as u64;
    mem::forget(buf);
    (ptr << 32) | len
}

/// Reserve `len` bytes in linear memory for the host to write into.
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr() as u32;
    mem::forget(buf);
    ptr
}

/// Release a buffer previously returned by `alloc`, `snapshot`, `patch` or `call`.
#[no_mangle]
pub extern "C" fn dealloc(ptr: u32, len: u32) {
    unsafe {
        drop(Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize));
    }
}

/// Initial tree (rev 1): the bind-based sample PostTile from
/// `examples/post-tile.rrl.json` — shown until `timeline` is called.
fn initial_root() -> Value {
    json!({
        "t": "Column", "k": "root", "c": [
            {
                "t": "PostTile", "k": "post-sample-1", "role": "contentBody",
                "p": { "post": "$.post", "variant": "normal" },
                "c": [
                    { "t": "RichText", "k": "body", "tag": "main_text", "p": { "source": "$.post.bodyRSL" } },
                    {
                        "t": "ActionBar", "k": "actions", "role": "actionBar",
                        "p": { "actions": "$.actions", "states": "$.actionStates[post.key]" }
                    }
                ]
            },
            // D-064: FOW surface デモの入口(タップで fow.open を要求する)。
            { "t": "Button", "k": "fow-demo", "p": { "label": "FOWデモ" }, "on": { "tap": "pickReaction" } }
        ]
    })
}

/// `pickReaction`: リアクションピッカー FOW を `fow.open` で要求する
/// (D-064 FOW の RRL surface 化)。中身は RRL ツリー(絵文字 Button ×3)。
/// ホストが FOW を閉じると `onClosed` に指定した `fowClosed` が呼ばれる。
fn pick_reaction(args: &Value) -> Result<Value, Value> {
    let emoji_button = |key: &str, label: &str| {
        json!({ "t": "Button", "k": key, "p": { "label": label }, "on": { "tap": "reactFromFow" } })
    };
    // 実物ノードへのアンカー (D-072): 呼び出し元(アクションバー)の
    // nodeId 末尾 key に張り付く。ホスト側で解決できなければ通常ドック。
    let anchor_key = args["nodeId"].as_str().and_then(|s| s.rsplit(':').next()).unwrap_or("");
    FOW_COUNT.store(0, Ordering::Relaxed); // surface rev は open で 1 に戻る (D-073)
    let response = host_call(
        "fow.open",
        json!({
            "id": "reaction-picker",
            "anchor": { "node": anchor_key },
            "spec": {
                "type": "widget",
                "confirmMode": "immediate",
                "initialHeightFactor": 0.35,
                "title": "リアクション (wasm)",
                "cancelText": "閉じる"
            },
            "onClosed": "fowClosed",
            // ✕(閉じる)は即クローズせず確認ポップアップ (D-066)。
            "onCancel": "confirmCloseFow",
            "root": {
                "t": "Column", "k": "fow-root", "c": [
                    { "t": "Text", "k": "fow-prompt", "p": { "text": format!("サンプルプラグインの FOW (postId: {})", args["postId"].as_str().unwrap_or("?")) } },
                    { "t": "Row", "k": "fow-emoji", "c": [
                        emoji_button("emoji-like", "👍"),
                        emoji_button("emoji-heart", "❤️"),
                        emoji_button("emoji-tada", "🎉")
                    ] },
                    // surface patch のデモ (D-073): +1 でカウンターだけ差分更新。
                    { "t": "Text", "k": "fow-count", "p": { "text": "タップ 0 回" } },
                    { "t": "Button", "k": "fow-plus", "p": { "label": "+1" }, "on": { "tap": "fowPlus" } }
                ]
            }
        }),
    );
    if response["ok"].as_bool() == Some(true) {
        Ok(json!({ "opened": response["value"]["opened"] }))
    } else {
        Err(response
            .get("error")
            .cloned()
            .unwrap_or(json!({ "code": "E00", "message": "fow.open failed" })))
    }
}

/// Current `snapshot`.
#[no_mangle]
pub extern "C" fn snapshot() -> u64 {
    let msg = with_state(|(rev, root)| {
        json!({
            "schema": "rasq.boundary/1",
            "scope": SCOPE,
            "type": "snapshot",
            "rev": *rev,
            "root": root.clone()
        })
    });
    pack(msg.to_string().into_bytes())
}

/// Pending surface patch (D-073) if any, else the demo `patch`
/// (baseRev 1 -> rev 2, only meaningful on the initial tree).
#[no_mangle]
pub extern "C" fn patch() -> u64 {
    let pending = mem::replace(&mut *PENDING_PATCH.lock().unwrap(), Value::Null);
    if !pending.is_null() {
        return pack(pending.to_string().into_bytes());
    }
    let msg = json!({
        "schema": "rasq.boundary/1",
        "scope": SCOPE,
        "type": "patch",
        "rev": 2,
        "baseRev": 1,
        "ops": [
            { "op": "setProp", "target": format!("{SCOPE}:post-sample-1"), "prop": "reactionCount", "value": 1 }
        ]
    });
    pack(msg.to_string().into_bytes())
}

fn media_type(mime: &str) -> &'static str {
    if mime.starts_with("image/gif") {
        "gif"
    } else if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else {
        "file"
    }
}

/// One Misskey note → a `PostTile` node with **literal props** (no host-store
/// binds: the plugin owns its data; PROTOCOL §2 props are plain values).
fn post_tile(note: &Value, fallback_instance: &str) -> Value {
    let user = &note["user"];
    let username = user["username"].as_str().unwrap_or("?");
    let display = match user["name"].as_str() {
        Some(n) if !n.is_empty() => n,
        _ => username,
    };
    let text = note["text"].as_str().unwrap_or("");
    let body = match note["cw"].as_str() {
        Some(cw) if !cw.is_empty() => format!("【CW: {cw}】\n{text}"),
        _ => text.to_string(),
    };
    let media: Vec<Value> = note["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    let url = f["url"].as_str()?;
                    Some(json!({
                        "id": f["id"],
                        "type": media_type(f["type"].as_str().unwrap_or("")),
                        "url": url,
                        "thumbnailUrl": f["thumbnailUrl"],
                        "sensitive": f["isSensitive"].as_bool().unwrap_or(false),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let reaction_total: i64 = note["reactions"]
        .as_object()
        .map(|m| m.values().filter_map(Value::as_i64).sum())
        .unwrap_or(0);
    let id = note["id"].as_str().unwrap_or("x");

    let mut children = vec![json!({
        "t": "RichText", "k": "body", "tag": "main_text", "p": { "source": body }
    })];
    if !media.is_empty() {
        children.push(json!({
            "t": "MediaGrid", "k": "media", "p": { "items": media }, "on": { "tap": "openViewer" }
        }));
    }
    children.push(json!({
        // key はタイル毎に一意化 (D-072: ジオメトリ登録簿の nodeId 衝突回避
        // — アンカー解決で「その投稿のバー」を特定できるように)。
        "t": "ActionBar", "k": format!("actions-{id}"), "role": "actionBar",
        "p": {
            "actions": [
                { "id": "reply", "label": "返信", "icon": "subdirectory_arrow_left" },
                { "id": "renote", "label": "リノート", "icon": "repeat", "doneIcon": "repeat_on" },
                { "id": "reaction", "label": "リアクション", "icon": "add_reaction_outlined", "doneIcon": "add_reaction" }
            ],
            "states": {
                "reply": { "count": note["repliesCount"] },
                "renote": { "count": note["renoteCount"] },
                "reaction": { "count": reaction_total }
            }
        },
        "on": { "tap": "performAction", "longPress": "pickReaction" }
    }));

    json!({
        "t": "PostTile", "k": format!("post-{id}"), "role": "contentBody",
        "p": {
            "post": {
                "id": id,
                "author": {
                    "displayName": display,
                    "handle": username,
                    "kind": "misskey",
                    "instance": user["host"].as_str().unwrap_or(fallback_instance),
                    "avatarUrl": user["avatarUrl"],
                },
                "createdAt": note["createdAt"],
            },
            "variant": "normal"
        },
        "on": { "tap": "openThread" },
        "c": children
    })
}

/// Misskey API を `net.fetch` 経由で叩く共通口(SNS 抽象関数で共用、D-075)。
/// `args.host`/`args.baseUrl` で接続先、`args.token` があれば `i` を積む。
/// 2xx 以外はエラー、204/空ボディは Null を返す。
fn misskey_fetch(args: &Value, endpoint: &str, mut body: Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io");
    // baseUrl override is for tests / self-hosted instances (default https).
    let base = match args["baseUrl"].as_str() {
        Some(b) => b.trim_end_matches('/').to_string(),
        None => format!("https://{host}"),
    };
    if let Some(token) = args["token"].as_str() {
        body["i"] = json!(token);
    }
    let response = host_call(
        "net.fetch",
        json!({
            "method": "POST",
            "url": format!("{base}/api/{endpoint}"),
            "headers": { "Content-Type": "application/json" },
            "body": body.to_string(),
        }),
    );
    if response["ok"].as_bool() != Some(true) {
        return Err(response
            .get("error")
            .cloned()
            .unwrap_or(json!({ "code": "B01", "message": "net.fetch failed" })));
    }
    let value = &response["value"];
    let status = value["status"].as_u64().unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(json!({ "code": "B03", "message": format!("HTTP {status} from {endpoint}") }));
    }
    let text = value["bodyText"].as_str().unwrap_or("");
    if text.trim().is_empty() {
        return Ok(Value::Null); // 204 等
    }
    serde_json::from_str(text).map_err(|e| json!({ "code": "D01", "message": format!("bad JSON from {endpoint}: {e}") }))
}

/// Misskey local-timeline fetch(`timeline` と `sns.timeline` で共用)。
fn fetch_local_notes(args: &Value) -> Result<(Vec<Value>, String), Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let limit = args["limit"].as_u64().unwrap_or(10).min(30);
    let mut body = json!({ "limit": limit });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let notes = misskey_fetch(args, "notes/local-timeline", body)?;
    Ok((notes.as_array().cloned().unwrap_or_default(), host))
}

/// One Misskey user → `rasq.post/1` の author/user ワイヤ。
fn user_wire(user: &Value, fallback_instance: &str) -> Value {
    let username = user["username"].as_str().unwrap_or("?");
    let display = match user["name"].as_str() {
        Some(n) if !n.is_empty() => n,
        _ => username,
    };
    json!({
        "id": user["id"],
        "kind": "misskey",
        "instance": user["host"].as_str().unwrap_or(fallback_instance),
        "handle": username,
        "displayName": display,
        "avatarUrl": user["avatarUrl"],
        "bio": user["description"],
    })
}

/// One Misskey note → `rasq.post/1` Post ワイヤ (D-074: SnsSession 契約の
/// boundary 写像が返すデータ形。RRL snapshot とは別)。
fn post_wire(note: &Value, fallback_instance: &str) -> Value {
    let user = &note["user"];
    let media: Vec<Value> = note["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    let url = f["url"].as_str()?;
                    Some(json!({
                        "id": f["id"],
                        "type": media_type(f["type"].as_str().unwrap_or("")),
                        "url": url,
                        "thumbnailUrl": f["thumbnailUrl"],
                        "sensitive": f["isSensitive"].as_bool().unwrap_or(false),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let reaction_total: i64 = note["reactions"]
        .as_object()
        .map(|m| m.values().filter_map(Value::as_i64).sum())
        .unwrap_or(0);
    let is_quote = note["renote"].is_object() && note["text"].as_str().is_some_and(|t| !t.is_empty());
    json!({
        "id": note["id"],
        "author": user_wire(user, fallback_instance),
        "createdAt": note["createdAt"],
        "body": note["text"].as_str().unwrap_or(""),
        "bodyFormat": "mfm",
        "cw": note["cw"],
        "media": media,
        // 引用 / リプライ元 (深さ 1 のみ — ワイヤの再帰は host 側 codec が対応済み)。
        "quote": if is_quote { shallow_post_wire(&note["renote"], fallback_instance) } else { Value::Null },
        "replyTo": if note["reply"].is_object() { shallow_post_wire(&note["reply"], fallback_instance) } else { Value::Null },
        "replyToId": note["replyId"],
        "counts": {
            "reply": note["repliesCount"],
            "renote": note["renoteCount"],
            "reaction": reaction_total,
        },
        "reactions": note["reactions"],
        "visibility": note["visibility"],
        "myReaction": note["myReaction"],
    })
}

/// 再帰を 1 段で止める浅い Post ワイヤ(quote/replyTo 用)。
fn shallow_post_wire(note: &Value, fallback_instance: &str) -> Value {
    json!({
        "id": note["id"],
        "author": user_wire(&note["user"], fallback_instance),
        "createdAt": note["createdAt"],
        "body": note["text"].as_str().unwrap_or(""),
        "bodyFormat": "mfm",
        "cw": note["cw"],
        "visibility": note["visibility"],
    })
}

/// `call("sns.timeline", {kind?, limit?, cursor?, host?, baseUrl?})` (D-074):
/// SnsSession 契約の boundary 写像 — Post ワイヤの page を返す。
fn sns_timeline(args: &Value) -> Result<Value, Value> {
    let (notes, host) = fetch_local_notes(args)?;
    let items: Vec<Value> = notes.iter().map(|n| post_wire(n, &host)).collect();
    let next = notes.last().and_then(|n| n["id"].as_str());
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// リアクション 1 位の置き換えタイル (D-082 デモ)。入力は Post ワイヤ。
/// PostTile(role contentBody)を含む = V5 検証を満たす。
fn top_reaction_tile(post: &Value) -> Value {
    let author = &post["author"];
    let id = post["id"].as_str().unwrap_or("x");
    json!({
        "t": "Column", "k": format!("top-{id}"), "c": [
            { "t": "Text", "k": format!("crown-{id}"), "p": { "text": format!("👑 リアクション1位 ({}件)", post["counts"]["reaction"].as_i64().unwrap_or(0)) } },
            {
                "t": "PostTile", "k": format!("post-{id}"), "role": "contentBody",
                "p": {
                    "post": {
                        "id": post["id"],
                        "author": {
                            "displayName": author["displayName"],
                            "handle": author["handle"],
                            "kind": author["kind"],
                            "instance": author["instance"],
                            "avatarUrl": author["avatarUrl"],
                        },
                        "createdAt": post["createdAt"],
                    },
                    "variant": "normal"
                },
                "c": [
                    { "t": "RichText", "k": format!("body-{id}"), "tag": "main_text", "p": { "source": post["body"] } }
                ]
            }
        ]
    })
}

/// `call("sns.thread", {id, host?, baseUrl?})` (D-075): notes/show +
/// conversation + children → Thread ワイヤ(ancestors は古い順)。
fn sns_thread(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let id = args["id"].as_str().unwrap_or("");
    let target = misskey_fetch(args, "notes/show", json!({ "noteId": id }))?;
    let conv = misskey_fetch(args, "notes/conversation", json!({ "noteId": id, "limit": 30 })).unwrap_or(json!([]));
    let children = misskey_fetch(args, "notes/children", json!({ "noteId": id, "limit": 30 })).unwrap_or(json!([]));
    let mut ancestors: Vec<Value> = conv.as_array().cloned().unwrap_or_default().iter().map(|n| post_wire(n, &host)).collect();
    ancestors.reverse(); // Misskey conversation は近い順 → 古い順へ
    let descendants: Vec<Value> = children.as_array().cloned().unwrap_or_default().iter().map(|n| post_wire(n, &host)).collect();
    Ok(json!({ "thread": { "post": post_wire(&target, &host), "ancestors": ancestors, "descendants": descendants } }))
}

/// `call("sns.performAction", {actionId, postId, args, host?, token?})`
/// (D-075): 短押しの中身はプラグインが決める(CONTRACT §5)。
fn sns_perform_action(args: &Value) -> Result<Value, Value> {
    let action = args["actionId"].as_str().unwrap_or("");
    let post_id = args["postId"].as_str().unwrap_or("");
    match action {
        "reaction" => {
            let reaction = args["args"]["reaction"].as_str().unwrap_or("👍");
            misskey_fetch(args, "notes/reactions/create", json!({ "noteId": post_id, "reaction": reaction }))?;
            Ok(json!({ "result": { "done": true } }))
        }
        "renote" => {
            misskey_fetch(args, "notes/create", json!({ "renoteId": post_id }))?;
            Ok(json!({ "result": { "done": true } }))
        }
        other => Err(json!({ "code": "C01", "message": format!("unsupported action: {other}") })),
    }
}

/// `call("sns.createPost", {draft, host?, token?})` (D-076): NewPost ワイヤ →
/// notes/create → 作成された Post ワイヤ。
fn sns_create_post(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let d = &args["draft"];
    let mut body = json!({});
    if d["repostOfId"].is_null() {
        body["text"] = d["text"].clone();
    }
    for (wire, api) in [("cw", "cw"), ("replyToId", "replyId"), ("visibility", "visibility"), ("reactionAcceptance", "reactionAcceptance")] {
        if !d[wire].is_null() {
            body[api] = d[wire].clone();
        }
    }
    if let Some(id) = d["quoteOfId"].as_str().or(d["repostOfId"].as_str()) {
        body["renoteId"] = json!(id);
    }
    if d["localOnly"].as_bool() == Some(true) {
        body["localOnly"] = json!(true);
    }
    if let Some(ids) = d["directToIds"].as_array() {
        body["visibleUserIds"] = json!(ids);
    }
    if let Some(ids) = d["mediaIds"].as_array() {
        body["fileIds"] = json!(ids);
    }
    if d["poll"].is_object() {
        let mut poll = json!({ "choices": d["poll"]["choices"] });
        if d["poll"]["multiple"].as_bool() == Some(true) {
            poll["multiple"] = json!(true);
        }
        if let Some(sec) = d["poll"]["expiresAfterSec"].as_u64() {
            poll["expiredAfter"] = json!(sec * 1000);
        }
        body["poll"] = poll;
    }
    let created = misskey_fetch(args, "notes/create", body)?;
    Ok(json!({ "post": post_wire(&created["createdNote"], &host) }))
}

/// `call("sns.user", {id | handle+host?})` (D-076): users/show → Profile ワイヤ。
fn sns_user(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let body = if let Some(id) = args["id"].as_str() {
        json!({ "userId": id })
    } else {
        let mut b = json!({ "username": args["handle"] });
        if let Some(h) = args["userHost"].as_str() {
            b["host"] = json!(h);
        }
        b
    };
    let u = misskey_fetch(args, "users/show", body)?;
    let username = u["username"].as_str().unwrap_or("?");
    Ok(json!({ "profile": {
        "user": user_wire(&u, &host),
        "bannerUrl": u["bannerUrl"],
        "description": u["description"],
        "followersCount": u["followersCount"],
        "followingCount": u["followingCount"],
        "postsCount": u["notesCount"],
        "createdAt": u["createdAt"],
        "location": u["location"],
        "isLocked": u["isLocked"],
        "isBot": u["isBot"],
        "isSuspended": u["isSuspended"],
        "fields": u["fields"],
        "profileUrl": format!("https://{host}/@{username}"),
    } }))
}

/// `call("sns.userPosts", {id, filter, limit, cursor?})` (D-076): 返信/引用の
/// 振り分け (D-069) 込み。
fn sns_user_posts(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let filter = args["filter"].as_str().unwrap_or("posts");
    let limit = args["limit"].as_u64().unwrap_or(20).min(50);
    let mut body = json!({
        "userId": args["id"],
        "limit": limit,
        "withReplies": filter == "withReplies" || filter == "replies",
        "withRenotes": filter != "media",
    });
    if filter == "media" {
        body["withFiles"] = json!(true);
    }
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, "users/notes", body)?;
    let notes: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let is_quote = |n: &Value| n["renote"].is_object() && n["text"].as_str().is_some_and(|t| !t.is_empty());
    let is_reply = |n: &Value| n["reply"].is_object() || !n["replyId"].is_null();
    let filtered: Vec<&Value> = notes
        .iter()
        .filter(|n| match filter {
            "replies" => is_reply(n),
            "quotes" => is_quote(n),
            "posts" => !is_reply(n) && !is_quote(n),
            _ => true,
        })
        .collect();
    let next = notes.last().and_then(|n| n["id"].as_str());
    Ok(json!({ "page": { "items": filtered.iter().map(|n| post_wire(n, &host)).collect::<Vec<Value>>(), "nextCursor": next } }))
}

/// `call("sns.reactions", {postId, type?, limit, cursor?})` (D-076)。
fn sns_reactions(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let mut body = json!({ "noteId": args["postId"], "limit": args["limit"].as_u64().unwrap_or(30).min(100) });
    if let Some(t) = args["type"].as_str() {
        body["type"] = json!(t);
    }
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, "notes/reactions", body)?;
    let entries: Vec<Value> = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| json!({ "id": r["id"], "user": user_wire(&r["user"], &host), "reaction": r["type"], "createdAt": r["createdAt"] }))
        .collect();
    let next = list.as_array().and_then(|a| a.last()).and_then(|r| r["id"].as_str());
    Ok(json!({ "page": { "items": entries, "nextCursor": next } }))
}

/// `call("sns.reposters" | "sns.quotes", {postId, limit, cursor?})` (D-076)。
fn sns_renotes(args: &Value, quotes: bool) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let mut body = json!({ "noteId": args["postId"], "limit": args["limit"].as_u64().unwrap_or(30).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, "notes/renotes", body)?;
    let notes: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let next = notes.last().and_then(|n| n["id"].as_str());
    let items: Vec<Value> = if quotes {
        notes
            .iter()
            .filter(|n| n["text"].as_str().is_some_and(|t| !t.is_empty()) || n["cw"].is_string())
            .map(|n| post_wire(n, &host))
            .collect()
    } else {
        notes.iter().map(|n| user_wire(&n["user"], &host)).collect()
    };
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// `call("timeline", {host?, limit?})`: fetch the local timeline through the
/// host's `net.fetch`, rebuild the tree, bump rev. The host then pulls the
/// new `snapshot()` (result carries `treeUpdated: true`).
fn timeline(args: &Value) -> Result<Value, Value> {
    let (notes, host) = fetch_local_notes(args)?;
    let tiles: Vec<Value> = notes.iter().map(|n| post_tile(n, &host)).collect();
    let count = tiles.len();
    let root = json!({ "t": "Column", "k": "tl", "c": tiles });
    let rev = with_state(|state| {
        state.0 += 1;
        state.1 = root;
        state.0
    });
    Ok(json!({ "treeUpdated": true, "count": count, "rev": rev, "host": host }))
}

/// Answers a host `call { callId, fn, args }` (PROTOCOL.md §3.2) with a
/// `result` message (§3.1).
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
        "timeline" => timeline(&args_value),
        // Demo of the host-function boundary: storage.set → storage.get.
        "storageRoundTrip" => {
            let set = host_call("storage.set", json!({"ns": "safe", "key": args_value["key"], "value": args_value["value"]}));
            let got = host_call("storage.get", json!({"ns": "safe", "key": args_value["key"]}));
            if set["ok"].as_bool().unwrap_or(false) && got["ok"].as_bool().unwrap_or(false) {
                Ok(json!({ "stored": got["value"]["value"] }))
            } else {
                Err(got.get("error").cloned().unwrap_or(json!({"code": "E00", "message": "host_fn failed"})))
            }
        }
        // SnsSession 契約の boundary 写像 (D-074/D-075, M5d): Post ワイヤを返す。
        "sns.timeline" => sns_timeline(&args_value),
        "sns.thread" => sns_thread(&args_value),
        "sns.performAction" => sns_perform_action(&args_value),
        "sns.createPost" => sns_create_post(&args_value),
        "sns.deletePost" => misskey_fetch(&args_value, "notes/delete", json!({ "noteId": args_value["id"] })).map(|_| json!({})),
        "sns.me" => misskey_fetch(&args_value, "i", json!({})).map(|me| {
            let host = args_value["host"].as_str().unwrap_or("misskey.io");
            json!({ "user": user_wire(&me, host) })
        }),
        "sns.user" => sns_user(&args_value),
        "sns.userPosts" => sns_user_posts(&args_value),
        "sns.reactions" => sns_reactions(&args_value),
        "sns.reposters" => sns_renotes(&args_value, false),
        "sns.quotes" => sns_renotes(&args_value, true),
        // gpu.compute デモ (D-081): f32 配列を GPU(WGSL compute)で 2 倍。
        // カスタム TL の重いソートやメディア編集フィルターの下地 — GPU が
        // 無い環境は E01 で返るので、その時はクラウドカルクレーター
        // (ml.infer)へ逃がすのが規約。
        "gpuDouble" => {
            use base64::Engine as _;
            let values: Vec<f32> = args_value["values"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            if values.is_empty() {
                Err(json!({ "code": "D01", "message": "values required" }))
            } else {
                let bytes: Vec<u8> = values.iter().flat_map(|f| f.to_le_bytes()).collect();
                let wgsl = "@group(0) @binding(0) var<storage, read> input: array<f32>;\n@group(0) @binding(1) var<storage, read_write> output: array<f32>;\n@compute @workgroup_size(64)\nfn main(@builtin(global_invocation_id) id: vec3<u32>) {\n  let i = id.x;\n  if (i < arrayLength(&input)) { output[i] = input[i] * 2.0; }\n}";
                let response = host_call(
                    "gpu.compute",
                    json!({
                        "wgsl": wgsl,
                        "entry": "main",
                        "input": base64::engine::general_purpose::STANDARD.encode(&bytes),
                        "outputSize": bytes.len(),
                        "workgroups": [values.len().div_ceil(64), 1, 1],
                    }),
                );
                if response["ok"].as_bool() == Some(true) {
                    match base64::engine::general_purpose::STANDARD.decode(response["value"]["output"].as_str().unwrap_or("")) {
                        Ok(out) => {
                            let doubled: Vec<f64> = out
                                .chunks_exact(4)
                                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64)
                                .collect();
                            Ok(json!({ "values": doubled, "placement": response["value"]["placement"] }))
                        }
                        Err(e) => Err(json!({ "code": "D01", "message": format!("bad output base64: {e}") })),
                    }
                } else {
                    Err(response.get("error").cloned().unwrap_or(json!({ "code": "E01", "message": "gpu.compute failed" })))
                }
            }
        }
        // タイムラインレンダラー契約 v1 (M5e/D-080): TL プラグイン =
        // 宣言 + ソートアルゴリズム。データ取得はホスト(基礎 TL)。
        "tl.timelines" => Ok(json!({ "timelines": [
            { "id": "reaction-rank", "label": "リアクション順", "base": "local" }
        ] })),
        "tl.sort" => match args_value["id"].as_str() {
            Some("reaction-rank") => {
                let posts = args_value["posts"].as_array().cloned().unwrap_or_default();
                // リアクション合計の降順(同数は元の順を保つ)。
                let mut idx: Vec<(usize, i64)> = posts
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (i, p["counts"]["reaction"].as_i64().unwrap_or(0)))
                    .collect();
                idx.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                let order: Vec<Value> = idx.iter().filter_map(|(i, _)| posts[*i]["id"].as_str().map(|s| json!(s))).collect();
                Ok(json!({ "order": order }))
            }
            other => Err(json!({ "code": "C01", "message": format!("unknown timeline: {other:?}") })),
        },
        // 投稿加工プラグイン (D-083 transform_content): 表示前の本文加工。
        // タイル構造は変えない(構造は tl.customize の領分)。デモ:
        // 「打ち上げ」を含む本文に 🚀 を前置。
        "post.transform" => {
            let posts = args_value["posts"].as_array().cloned().unwrap_or_default();
            let mut patches = serde_json::Map::new();
            for p in &posts {
                if let (Some(id), Some(body)) = (p["id"].as_str(), p["body"].as_str()) {
                    if body.contains("打ち上げ") && !body.starts_with("🚀") {
                        patches.insert(id.to_string(), json!({ "body": format!("🚀 {body}") }));
                    }
                }
            }
            Ok(json!({ "posts": patches }))
        }
        // 投稿カスタマイザ (D-082): 完全置き換えタイル(replace_tile)。
        // 必須ロール contentBody を含めること(V5 検証 — 欠けるとホストが
        // 破棄して通常タイルに縮退する)。デモ: リアクション 1 位に王冠。
        "tl.customize" => match args_value["id"].as_str() {
            Some("reaction-rank") => {
                let posts = args_value["posts"].as_array().cloned().unwrap_or_default();
                let mut tiles = serde_json::Map::new();
                if let Some(top) = posts.first() {
                    if top["counts"]["reaction"].as_i64().unwrap_or(0) > 0 {
                        if let Some(id) = top["id"].as_str() {
                            tiles.insert(id.to_string(), top_reaction_tile(top));
                        }
                    }
                }
                // 上付きバナー行 (D-085): 2 位は通常タイルのまま、RT の
                // 「♻︎ 〇〇がリノートしました」と同スタイルのバッジだけ足す。
                let mut badges = serde_json::Map::new();
                if let Some(second) = posts.get(1) {
                    if let Some(id) = second["id"].as_str() {
                        let n = second["counts"]["reaction"].as_i64().unwrap_or(0);
                        badges.insert(id.to_string(), json!({ "icon": "star", "text": format!("リアクション2位 ({n}件)") }));
                    }
                }
                Ok(json!({ "tiles": tiles, "badges": badges }))
            }
            other => Err(json!({ "code": "C01", "message": format!("unknown timeline: {other:?}") })),
        },
        // アクションバー宣言(register_action と同形の JSON)。
        "sns.actions" => Ok(json!({ "actions": [
            { "id": "reply", "genreTag": "Reply", "label": "返信", "icon": "subdirectory_arrow_left" },
            { "id": "renote", "genreTag": "Repost", "label": "リノート", "icon": "repeat", "doneIcon": "repeat_on" },
            { "id": "reaction", "genreTag": "Reaction", "label": "リアクション", "icon": "add_reaction_outlined", "doneIcon": "add_reaction" }
        ] })),
        "pickReaction" => pick_reaction(&args_value),
        // FOW surface への差分更新デモ (D-073): カウンターを setProp する
        // surface patch を積み、`patch: true` でホストに引き取らせる。
        "fowPlus" => {
            let n = FOW_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            *PENDING_PATCH.lock().unwrap() = json!({
                "schema": "rasq.boundary/1",
                "scope": SCOPE,
                "type": "patch",
                "surface": "reaction-picker",
                "rev": n + 1,
                "baseRev": n,
                "ops": [
                    { "op": "setProp", "target": format!("{SCOPE}:fow-count"), "prop": "text", "value": format!("タップ {n} 回") }
                ]
            });
            Ok(json!({ "patch": true, "count": n }))
        }
        // FOW 内の絵文字 Button のタップ: nodeId 末尾の key で絵文字を特定し、
        // `fow.close`(result = 絵文字)で自分の FOW を閉じる。
        "reactFromFow" => {
            let node_id = args_value["nodeId"].as_str().unwrap_or("");
            let emoji = match node_id.rsplit(':').next().unwrap_or("") {
                "emoji-like" => "👍",
                "emoji-heart" => "❤️",
                "emoji-tada" => "🎉",
                other => other,
            };
            host_call("fow.close", json!({ "id": "reaction-picker", "result": emoji }));
            Ok(json!({ "picked": emoji }))
        }
        // ✕押下(fow.open の onCancel)→ 画面そのままで確認ポップアップ
        // (確認 FOW)を出す (D-066)。ホストはまだ閉じていない。
        "confirmCloseFow" => {
            let response = host_call(
                "fow.open",
                json!({
                    "id": "reaction-picker-confirm",
                    // 子 FOW (D-067): 親(ピッカー)の側に付き、親は子の
                    // 高さぶん持ち上がって内容が被らない。
                    "parentId": "reaction-picker",
                    "spec": {
                        "type": "widget",
                        "confirmMode": "confirm",
                        "initialHeightFactor": 0.22,
                        "title": "閉じますか?"
                    },
                    "root": {
                        "t": "Column", "k": "confirm-root", "c": [
                            { "t": "Text", "k": "confirm-msg", "p": { "text": "リアクションを選ばずに閉じますか?" } },
                            { "t": "Row", "k": "confirm-actions", "c": [
                                { "t": "Button", "k": "confirm-discard", "p": { "label": "破棄" }, "on": { "tap": "discardFow" } },
                                { "t": "Button", "k": "confirm-keep", "p": { "label": "続ける" }, "on": { "tap": "keepFow" } }
                            ] }
                        ]
                    }
                }),
            );
            if response["ok"].as_bool() == Some(true) {
                Ok(json!({ "opened": true }))
            } else {
                // 確認を出せない環境では素直に閉じる。
                host_call("fow.close", json!({ "id": "reaction-picker", "reason": "discard" }));
                Ok(json!({ "opened": false }))
            }
        }
        "discardFow" => {
            host_call("fow.close", json!({ "id": "reaction-picker-confirm" }));
            host_call("fow.close", json!({ "id": "reaction-picker", "reason": "discard" }));
            Ok(json!({}))
        }
        "keepFow" => {
            host_call("fow.close", json!({ "id": "reaction-picker-confirm" }));
            Ok(json!({}))
        }
        // ホストからの FOW クローズ通知(fow.open の onClosed)。
        "fowClosed" => {
            *LAST_FOW_CLOSED.lock().unwrap() = args_value.clone();
            Ok(json!({}))
        }
        "lastFowClosed" => Ok(LAST_FOW_CLOSED.lock().unwrap().clone()),
        // ホストからのライフサイクル (D-072、v1 は call("lifecycle") で届く)。
        "lifecycle" => {
            *LAST_LIFECYCLE.lock().unwrap() = args_value.clone();
            Ok(json!({}))
        }
        "lastLifecycle" => Ok(LAST_LIFECYCLE.lock().unwrap().clone()),
        "openThread" | "openUser" | "openViewer" | "performAction" => {
            Ok(json!({ "echo": { "fn": fn_name, "args": args_value } }))
        }
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

fn read_str(ptr: u32, len: u32) -> String {
    unsafe {
        let slice = std::slice::from_raw_parts(ptr as *const u8, len as usize);
        String::from_utf8_lossy(slice).into_owned()
    }
}
