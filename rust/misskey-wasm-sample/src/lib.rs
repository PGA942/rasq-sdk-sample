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

/// `TimelineKind.id` (CONTRACT §6) → Misskey のエンドポイントと追加パラメータ。
///
/// `list:{id}` と `tag:{tag}` は接頭辞つきの動的 id なので、値を本文へ移す。
/// 未知の id は `home` に倒す(ホストが宣言に無い TL を要求することは
/// 無いが、縮退させておく方が安全)。
fn timeline_endpoint(kind: &str) -> (&'static str, Value) {
    match kind {
        "local" => ("notes/local-timeline", json!({})),
        "social" | "recommended" => ("notes/hybrid-timeline", json!({})),
        "global" => ("notes/global-timeline", json!({})),
        "featured" => ("notes/featured", json!({})),
        k if k.starts_with("list:") => ("notes/user-list-timeline", json!({ "listId": &k[5..] })),
        k if k.starts_with("tag:") => ("notes/search-by-tag", json!({ "tag": &k[4..] })),
        _ => ("notes/timeline", json!({})),
    }
}

/// タイムライン取得(`timeline` と `sns.timeline` で共用)。`kind` 省略時は
/// ローカル TL —— 旧 `timeline`(RRL デモ)の挙動を保つため。
fn fetch_notes(args: &Value, default_kind: &str) -> Result<(Vec<Value>, String), Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let limit = args["limit"].as_u64().unwrap_or(10).min(30);
    let kind = args["kind"].as_str().unwrap_or(default_kind);
    let (endpoint, extra) = timeline_endpoint(kind);
    let mut body = json!({ "limit": limit });
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            body[k] = v.clone();
        }
    }
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    // TL 設定 (D-044): ホストは設定ページの値を毎呼び出しの `settings` に
    // area ごと入れ子で載せてくる。知っているキーだけ読む。
    let tl_settings = &args["settings"]["timeline"];
    if let Some(with_renotes) = tl_settings["withRenotes"].as_bool() {
        body["withRenotes"] = json!(with_renotes);
    }
    if let Some(with_replies) = tl_settings["withReplies"].as_bool() {
        body["withReplies"] = json!(with_replies);
    }
    let notes = misskey_fetch(args, endpoint, body)?;
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
///
/// **純リノート**(text 空・添付なし・投票なし)はホストのマッパーと同じ規則で
/// 中身のノートに展開し、`repostedBy` に外側の投稿者を載せる(D-100:
/// 展開しないと本文もquoteも無い「内容のない投稿」になる)。
fn post_wire(note: &Value, fallback_instance: &str) -> Value {
    post_wire_at(note, fallback_instance, 0)
}

fn post_wire_at(note: &Value, fallback_instance: &str, depth: usize) -> Value {
    let text_empty = note["text"].as_str().is_none_or(|t| t.is_empty());
    let files_empty = note["fileIds"].as_array().is_none_or(|a| a.is_empty());
    let is_pure_renote = note["renote"].is_object() && text_empty && files_empty && note["poll"].is_null();
    if is_pure_renote && depth == 0 {
        // 「◯◯がリノートしました」— id/時刻は外側、内容は中身のノート。
        // repostTargetId (D-112): アクション/スレッド/各欄はこの id(中身の
        // ノート)へ読み替える(公式クライアントの appearNote と同じ)。
        let mut inner = post_wire_at(&note["renote"], fallback_instance, depth + 1);
        let target = inner["id"].clone();
        inner["id"] = note["id"].clone();
        inner["createdAt"] = note["createdAt"].clone();
        inner["repostedBy"] = user_wire(&note["user"], fallback_instance);
        inner["repostTargetId"] = target;
        return inner;
    }
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
    // リアクション絵文字 URL (D-112): チップ表示用。キーはリアクション文字列
    // そのもの。ローカルカスタム(:name@.:)は reactionEmojis に URL が来ない
    // → /emoji/<name>.webp を補完(ホストの D-041 と同じ規則)。
    let mut reaction_emojis = serde_json::Map::new();
    if let Some(re) = note["reactions"].as_object() {
        for k in re.keys() {
            if k.starts_with(':') && k.ends_with("@.:") {
                let name = &k[1..k.len() - 3];
                reaction_emojis.insert(k.clone(), json!(format!("https://{fallback_instance}/emoji/{name}.webp")));
            }
        }
    }
    if let Some(re) = note["reactionEmojis"].as_object() {
        for (k, v) in re {
            if v.is_string() {
                reaction_emojis.insert(format!(":{k}:"), v.clone());
            }
        }
    }
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
        "reactionEmojis": reaction_emojis,
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
    let (notes, host) = fetch_notes(args, "home")?;
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
/// `call("sns.emojis")` (D-111): カスタム絵文字一覧。TL 内の :shortcode: と
/// 絵文字ピッカーにホストが使う。
fn sns_emojis(args: &Value) -> Result<Value, Value> {
    let res = misskey_fetch(args, "emojis", json!({}))?;
    Ok(json!({ "emojis": res["emojis"] }))
}

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

// ── 通知 / 関係 / DM / 検索 (D-134) ────────────────────────────────
//
// ここから下は `SnsSession` 契約 (CONTRACT §5) の残りを boundary へ写した
// 分。ワイヤは PROTOCOL §4.3 に追記してある。返す形はホスト側モデルに
// 1:1 で載るので、`WasmSnsSession` は取り込むだけで済む。

/// `call("sns.inspectServer", {host})` (CONTRACT §2): サーバー検索 /
/// PDS 変更の画面が出す情報。**認証前に呼ばれる**ので token は無い。
fn sns_inspect_server(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let meta = misskey_fetch(args, "meta", json!({ "detail": true }))?;
    Ok(json!({ "server": {
        "host": host,
        "name": meta["name"].as_str().unwrap_or(&host),
        "software": "misskey",
        "version": meta["version"],
        "description": meta["description"],
        "iconUrl": meta["iconUrl"],
        "registrationOpen": meta["disableRegistration"].as_bool().map(|d| !d),
        "maintainer": meta["maintainerName"],
    } }))
}

/// `call("sns.capabilities")`: セッション単位の能力値。**インスタンスごとに
/// 違う**ので、ホストが決め打ちせずプラグインから受け取る(Misskey は
/// 既定 3000 だが、インスタンス設定で変わる)。将来の能力フラグもここへ足す。
fn sns_capabilities(args: &Value) -> Result<Value, Value> {
    // meta が取れなければ Misskey の既定値で答える(ログイン直後に
    // コンポーザーを開いても文字数制限が出せるように)。
    let max = misskey_fetch(args, "meta", json!({ "detail": false }))
        .ok()
        .and_then(|m| m["maxNoteTextLength"].as_u64())
        .unwrap_or(3000);
    Ok(json!({ "maxPostLength": max }))
}

/// `call("sns.settingsRead", {area})` / `sns.settingsWrite` (D-044):
/// `source: "remote"` の設定ページはサーバーが真実なので、KV ではなく
/// ここを通す。Misskey は `account` ページを `i` / `i/update` に写す。
fn sns_settings_read(args: &Value) -> Result<Value, Value> {
    if args["area"].as_str() != Some("account") {
        return Err(json!({ "code": "C01", "message": "no such remote settings area" }));
    }
    let me = misskey_fetch(args, "i", json!({}))?;
    Ok(json!({ "values": {
        "isLocked": me["isLocked"],
        "isExplorable": me["isExplorable"],
        "hideOnlineStatus": me["hideOnlineStatus"],
        "noCrawle": me["noCrawle"],
        "alwaysMarkNsfw": me["alwaysMarkNsfw"],
        "autoAcceptFollowed": me["autoAcceptFollowed"],
    } }))
}

fn sns_settings_write(args: &Value) -> Result<Value, Value> {
    if args["area"].as_str() != Some("account") {
        return Err(json!({ "code": "C01", "message": "no such remote settings area" }));
    }
    let Some(key) = args["key"].as_str() else {
        return Err(json!({ "code": "D01", "message": "key required" }));
    };
    let mut body = json!({});
    body[key] = args["value"].clone();
    misskey_fetch(args, "i/update", body)?;
    // 書き込み後の最新値を返す(サーバーが弾いた変更も見えるように)。
    sns_settings_read(args)
}

/// `call("sns.timelines")`: このアカウントで出せる基本 TL の宣言。
/// ホストはこれをタブに並べる(CONTRACT §6 の id 体系)。SNS ごとに
/// 揃う TL が違うため、ハードコードではなくプラグインが宣言する。
fn sns_timelines(_args: &Value) -> Result<Value, Value> {
    Ok(json!({ "timelines": [
        { "id": "home",   "label": "ホーム" },
        { "id": "social", "label": "ソーシャル" },
        { "id": "local",  "label": "ローカル" },
        { "id": "global", "label": "グローバル" },
        { "id": "featured", "label": "Today" },
    ] }))
}

/// Misskey の通知種別 → RASQ の `NotificationKind`。
/// 未知の種別は `other` に倒す(ホストはアクションタイルとして描く)。
fn notification_kind(t: &str) -> &'static str {
    match t {
        "reaction" => "reaction",
        "renote" => "repost",
        "quote" => "quote",
        "reply" => "reply",
        "mention" => "mention",
        "follow" => "follow",
        "receiveFollowRequest" => "followRequest",
        "followRequestAccepted" => "followAccepted",
        "pollEnded" | "pollVote" => "pollEnded",
        // D-045: サーバー/アプリ発の本文つき通知はシステム通知にまとめる。
        "app" | "roleAssigned" | "achievementEarned" | "login" | "test" => "system",
        _ => "other",
    }
}

/// 1 件の通知 → 通知ワイヤ。`i/notifications-grouped` は `users` /
/// `reactions` を持つので、グループ通知もそのまま畳んで返す。
fn notification_wire(n: &Value, host: &str) -> Value {
    let kind = notification_kind(n["type"].as_str().unwrap_or(""));

    // 単体は `user`、グループは `users`。どちらも actors 配列に均す。
    let mut actors: Vec<Value> = vec![];
    if let Some(list) = n["users"].as_array() {
        actors.extend(list.iter().map(|u| user_wire(u, host)));
    } else if n["user"].is_object() {
        actors.push(user_wire(&n["user"], host));
    }

    // グループリアクション: actor id → リアクション文字列。
    let mut by_actor = serde_json::Map::new();
    if let Some(list) = n["reactions"].as_array() {
        for r in list {
            if let (Some(uid), Some(reaction)) = (r["user"]["id"].as_str(), r["type"].as_str()) {
                by_actor.insert(uid.to_string(), json!(reaction));
            }
        }
    }

    json!({
        "id": n["id"],
        "kind": kind,
        "createdAt": n["createdAt"],
        "read": n["isRead"].as_bool().unwrap_or(false),
        "actors": actors,
        "post": if n["note"].is_object() { post_wire(&n["note"], host) } else { Value::Null },
        "reaction": n["reaction"],
        "reactionsByActor": Value::Object(by_actor),
        "title": n["header"],
        "body": n["body"],
    })
}

/// `call("sns.notifications", {limit, cursor?})`。
/// グループ版が無いインスタンス(404)は素の `i/notifications` に落ちる —
/// CONTRACT §5 の Misskey 参照実装と同じ縮退。
fn sns_notifications(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let mut body = json!({ "limit": args["limit"].as_u64().unwrap_or(20).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = match misskey_fetch(args, "i/notifications-grouped", body.clone()) {
        Ok(v) => v,
        Err(_) => misskey_fetch(args, "i/notifications", body)?,
    };
    let items: Vec<Value> = list.as_array().cloned().unwrap_or_default().iter().map(|n| notification_wire(n, &host)).collect();
    let next = list.as_array().and_then(|a| a.last()).and_then(|n| n["id"].as_str());
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// Misskey の `users/relation` 1 件 → FollowState ワイヤ。
fn follow_state_wire(r: &Value) -> Value {
    json!({
        "following": r["isFollowing"].as_bool().unwrap_or(false),
        "followedBy": r["isFollowed"].as_bool().unwrap_or(false),
        // 申請中は「自分が送った申請」。相手からの申請は通知側で扱う。
        "requested": r["hasPendingFollowRequestFromYou"].as_bool().unwrap_or(false),
        "muted": r["isMuted"].as_bool().unwrap_or(false),
        "blocked": r["isBlocking"].as_bool().unwrap_or(false),
        "blockedBy": r["isBlocked"].as_bool().unwrap_or(false),
    })
}

/// `call("sns.relationships", {ids: [id]})` → `{states: {id: FollowState}}`。
/// `users/relation` は 1 件でもオブジェクト、複数だと配列を返すので均す。
fn sns_relationships(args: &Value) -> Result<Value, Value> {
    let ids: Vec<String> = args["ids"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    if ids.is_empty() {
        return Ok(json!({ "states": {} }));
    }
    let res = misskey_fetch(args, "users/relation", json!({ "userId": ids }))?;
    let rows: Vec<Value> = match res.as_array() {
        Some(a) => a.clone(),
        None => vec![res],
    };
    let mut states = serde_json::Map::new();
    for r in rows {
        if let Some(id) = r["id"].as_str() {
            states.insert(id.to_string(), follow_state_wire(&r));
        }
    }
    Ok(json!({ "states": states }))
}

/// `call("sns.setRelation", {id, relation: following|muted|blocked, on})`
/// → `{state}`。3 つの setter を 1 本にまとめてある(ホスト側で
/// `setFollowing` / `setMuted` / `setBlocked` へ割り振る)。
/// 書き込み後は関係を読み直して**サーバーが真実**の状態を返す —
/// 鍵アカウントへのフォローは `requested` 止まりになるため。
fn sns_set_relation(args: &Value) -> Result<Value, Value> {
    let id = args["id"].as_str().unwrap_or("");
    let on = args["on"].as_bool().unwrap_or(false);
    let endpoint = match (args["relation"].as_str().unwrap_or(""), on) {
        ("following", true) => "following/create",
        ("following", false) => "following/delete",
        ("muted", true) => "mute/create",
        ("muted", false) => "mute/delete",
        ("blocked", true) => "blocking/create",
        ("blocked", false) => "blocking/delete",
        (other, _) => return Err(json!({ "code": "C01", "message": format!("unknown relation {other:?}") })),
    };
    misskey_fetch(args, endpoint, json!({ "userId": id }))?;
    let res = misskey_fetch(args, "users/relation", json!({ "userId": [id] }))?;
    let row = match res.as_array().and_then(|a| a.first()) {
        Some(r) => r.clone(),
        None => res,
    };
    Ok(json!({ "state": follow_state_wire(&row) }))
}

/// `call("sns.follows", {id, direction: followers|following, limit, cursor?})`。
fn sns_follows(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let endpoint = match args["direction"].as_str().unwrap_or("followers") {
        "following" => "users/following",
        _ => "users/followers",
    };
    let mut body = json!({ "userId": args["id"], "limit": args["limit"].as_u64().unwrap_or(20).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, endpoint, body)?;
    let rows: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    // followers は `follower`、following は `followee` にユーザーが入る。
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let u = if r["followee"].is_object() { &r["followee"] } else { &r["follower"] };
            user_wire(u, &host)
        })
        .collect();
    let next = rows.last().and_then(|r| r["id"].as_str());
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// `call("sns.userList", {kind: muted|blocked|followRequests, limit, cursor?})`。
/// 「人の一覧」を 1 本の関数に束ねる — どれも同じ形(user の page)を返す。
fn sns_user_list(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let kind = args["kind"].as_str().unwrap_or("");
    let mut body = json!({ "limit": args["limit"].as_u64().unwrap_or(30).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let (endpoint, field) = match kind {
        "muted" => ("mute/list", "mutee"),
        "blocked" => ("blocking/list", "blockee"),
        "followRequests" => ("following/requests/list", "follower"),
        other => return Err(json!({ "code": "C01", "message": format!("unknown user list {other:?}") })),
    };
    let list = misskey_fetch(args, endpoint, body)?;
    let rows: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let items: Vec<Value> = rows.iter().map(|r| user_wire(&r[field], &host)).collect();
    let next = rows.last().and_then(|r| r["id"].as_str());
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// `call("sns.search", {kind: posts|users, q, limit, cursor?})`。
fn sns_search(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let q = args["q"].as_str().unwrap_or("");
    let limit = args["limit"].as_u64().unwrap_or(20).min(100);
    if args["kind"].as_str() == Some("users") {
        let list = misskey_fetch(args, "users/search", json!({ "query": q, "limit": limit }))?;
        let rows: Vec<Value> = list.as_array().cloned().unwrap_or_default();
        let items: Vec<Value> = rows.iter().map(|u| user_wire(u, &host)).collect();
        // users/search はカーソルを持たない(offset 方式)。
        return Ok(json!({ "page": { "items": items, "nextCursor": Value::Null } }));
    }
    let mut body = json!({ "query": q, "limit": limit });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, "notes/search", body)?;
    let notes: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let next = notes.last().and_then(|n| n["id"].as_str());
    Ok(json!({ "page": { "items": notes.iter().map(|n| post_wire(n, &host)).collect::<Vec<Value>>(), "nextCursor": next } }))
}

/// `call("sns.trendingTags")` → `{tags: [{tag, count?}]}`。
fn sns_trending_tags(args: &Value) -> Result<Value, Value> {
    let list = misskey_fetch(args, "hashtags/trend", json!({}))?;
    let tags: Vec<Value> = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| json!({ "tag": t["tag"], "count": t["usersCount"] }))
        .collect();
    Ok(json!({ "tags": tags }))
}

/// `call("sns.favorites", {limit, cursor?})` → ブックマーク(お気に入り)。
fn sns_favorites(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let mut body = json!({ "limit": args["limit"].as_u64().unwrap_or(20).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, "i/favorites", body)?;
    let rows: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let items: Vec<Value> = rows.iter().map(|f| post_wire(&f["note"], &host)).collect();
    let next = rows.last().and_then(|f| f["id"].as_str());
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// `call("sns.dmRooms")` → `{rooms: [...]}` (CONTRACT §7)。
///
/// 「限定公開がDM代わりの SNS」をまとめる規約どおり、`visibility ==
/// specified` の投稿を**相手ユーザー単位**に束ねて `kind: direct` の
/// ルームにする。ネイティブ Chat (2025.4+) があれば `kind: chat` の
/// ルームを足し、404/403 のインスタンスでは黙って無視する。
fn sns_dm_rooms(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let me = misskey_fetch(args, "i", json!({}))?;
    let my_id = me["id"].as_str().unwrap_or("").to_string();

    // 受け取った限定公開 + 自分が送った限定公開。
    let received = misskey_fetch(args, "notes/mentions", json!({ "visibility": "specified", "limit": 50 })).unwrap_or(json!([]));
    let sent = misskey_fetch(args, "users/notes", json!({ "userId": my_id, "limit": 50, "withReplies": true })).unwrap_or(json!([]));

    // peerId → (最新ノート, 未読数)。Misskey に DM 未読の概念は無いので 0。
    let mut rooms: Vec<(String, Value, Value)> = vec![]; // (peerId, peer, lastPost)
    let mut seen: Vec<String> = vec![];
    let consider = |note: &Value, rooms: &mut Vec<(String, Value, Value)>, seen: &mut Vec<String>| {
        if note["visibility"].as_str() != Some("specified") {
            return;
        }
        // 相手 = 自分以外の可視ユーザー(自分の送信)か、投稿者(受信)。
        let peer_id = if note["user"]["id"].as_str() == Some(my_id.as_str()) {
            note["visibleUserIds"].as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).map(str::to_string)
        } else {
            note["user"]["id"].as_str().map(str::to_string)
        };
        let Some(peer_id) = peer_id else { return };
        if seen.contains(&peer_id) {
            return; // 既に新しい方を採っている(どちらの一覧も新しい順)
        }
        seen.push(peer_id.clone());
        rooms.push((peer_id, note["user"].clone(), post_wire(note, &host)));
    };
    for n in received.as_array().cloned().unwrap_or_default().iter() {
        consider(n, &mut rooms, &mut seen);
    }
    for n in sent.as_array().cloned().unwrap_or_default().iter() {
        consider(n, &mut rooms, &mut seen);
    }

    let mut out: Vec<Value> = rooms
        .iter()
        .map(|(peer_id, peer_raw, last)| {
            // 送信ノートでは `user` が自分なので、相手は last の可視先から引く。
            let peer = if peer_raw["id"].as_str() == Some(my_id.as_str()) {
                json!({ "id": peer_id, "kind": "misskey", "instance": host, "handle": "", "displayName": "" })
            } else {
                user_wire(peer_raw, &host)
            };
            json!({ "id": format!("dm:{peer_id}"), "kind": "direct", "peer": peer, "lastPost": last, "unreadCount": 0 })
        })
        .collect();

    // ネイティブ Chat。使えないインスタンスでは足さない。
    if let Ok(history) = misskey_fetch(args, "chat/history", json!({ "limit": 30 })) {
        for m in history.as_array().cloned().unwrap_or_default().iter() {
            let peer = if m["fromUser"]["id"].as_str() == Some(my_id.as_str()) { &m["toUser"] } else { &m["fromUser"] };
            let Some(peer_id) = peer["id"].as_str() else { continue };
            out.push(json!({
                "id": format!("chat:{peer_id}"),
                "kind": "chat",
                "peer": user_wire(peer, &host),
                "lastPost": Value::Null,
                "unreadCount": if m["isRead"].as_bool().unwrap_or(true) { 0 } else { 1 },
                "preview": m["text"],
            }));
        }
    }
    Ok(json!({ "rooms": out }))
}

/// `call("sns.sendDm", {roomKind: direct|chat, peerId, draft})` → `{post}`。
/// direct は限定公開ノート、chat はネイティブ Chat メッセージ (CONTRACT §7)。
fn sns_send_dm(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let peer = args["peerId"].as_str().unwrap_or("");
    let draft = &args["draft"];
    if args["roomKind"].as_str() == Some("chat") {
        let mut body = json!({ "toUserId": peer, "text": draft["text"] });
        // Chat は画像 1 枚まで (CONTRACT §7)。
        if let Some(first) = draft["mediaIds"].as_array().and_then(|a| a.first()) {
            body["fileId"] = first.clone();
        }
        let sent = misskey_fetch(args, "chat/messages/create-to-user", body)?;
        // Chat メッセージはノートではないので、Post ワイヤへ最小限で写す。
        return Ok(json!({ "post": {
            "id": sent["id"],
            "author": user_wire(&sent["fromUser"], &host),
            "createdAt": sent["createdAt"],
            "body": sent["text"].as_str().unwrap_or(""),
            "bodyFormat": "plain",
            "visibility": "specified",
        } }));
    }
    let mut body = json!({
        "text": draft["text"],
        "visibility": "specified",
        "visibleUserIds": [peer],
    });
    if let Some(cw) = draft["cw"].as_str() {
        body["cw"] = json!(cw);
    }
    if let Some(ids) = draft["mediaIds"].as_array() {
        if !ids.is_empty() {
            body["fileIds"] = json!(ids);
        }
    }
    if let Some(reply) = draft["replyToId"].as_str() {
        body["replyId"] = json!(reply);
    }
    let created = misskey_fetch(args, "notes/create", body)?;
    Ok(json!({ "post": post_wire(&created["createdNote"], &host) }))
}

/// `call("sns.reportUser", {id, comment})` → `{}`。
fn sns_report_user(args: &Value) -> Result<Value, Value> {
    misskey_fetch(args, "users/report-abuse", json!({ "userId": args["id"], "comment": args["comment"] })).map(|_| json!({}))
}

// ── メディア / プロフィール / リスト / クリップ (D-136) ─────────────

/// multipart/form-data の本体を組み立てる。
///
/// `net.fetch` はバイト列を `{b64}` で受けるので(host-fn schema)、
/// **multipart の組み立てはプラグインの仕事**。ホストは中身を解釈しない。
/// 境界に multipart 専用の口を足すより、既にあるバイナリ口に載せる方が
/// 面が増えなくて済む。
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

/// 添付ファイル 1 件 → MediaItem ワイヤ。`post_wire` の中と同じ形。
fn media_wire(file: &Value) -> Value {
    json!({
        "id": file["id"],
        "type": media_type(file["type"].as_str().unwrap_or("")),
        "url": file["url"],
        "thumbnailUrl": file["thumbnailUrl"],
        "sensitive": file["isSensitive"].as_bool().unwrap_or(false),
        "alt": file["comment"],
        "fileName": file["name"],
    })
}

/// `call("sns.uploadMedia", {bytesB64, filename, mimeType?, sensitive?, alt?})`
/// → `{media}`。Misskey は `drive/files/create` が multipart (CONTRACT §5)。
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
    let sensitive = if args["sensitive"].as_bool().unwrap_or(false) { "true" } else { "false" };
    let alt = args["alt"].as_str().unwrap_or("");
    let token = args["token"].as_str().unwrap_or("");
    let boundary = "----rasqUploadBoundary7MA4YWxkTrZu0gW";

    let mut fields: Vec<(&str, &str)> = vec![("i", token), ("isSensitive", sensitive), ("name", filename)];
    if !alt.is_empty() {
        fields.push(("comment", alt));
    }
    let body = multipart_body(boundary, &fields, Some(("file", filename, mime, &bytes)));

    let host = args["host"].as_str().unwrap_or("misskey.io");
    let base = match args["baseUrl"].as_str() {
        Some(b) => b.trim_end_matches('/').to_string(),
        None => format!("https://{host}"),
    };
    let response = host_call(
        "net.fetch",
        json!({
            "method": "POST",
            "url": format!("{base}/api/drive/files/create"),
            "headers": { "Content-Type": format!("multipart/form-data; boundary={boundary}") },
            "body": { "b64": base64::engine::general_purpose::STANDARD.encode(&body) },
        }),
    );
    if response["ok"].as_bool() != Some(true) {
        return Err(response.get("error").cloned().unwrap_or(json!({ "code": "B01", "message": "net.fetch failed" })));
    }
    let status = response["value"]["status"].as_u64().unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(json!({ "code": status.to_string(), "message": format!("HTTP {status} from drive/files/create") }));
    }
    let file: Value = serde_json::from_str(response["value"]["bodyText"].as_str().unwrap_or(""))
        .map_err(|e| json!({ "code": "D01", "message": format!("bad JSON from drive/files/create: {e}") }))?;
    Ok(json!({ "media": media_wire(&file) }))
}

/// `call("sns.updateProfile", {update})` → `{profile}`。
fn sns_update_profile(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let u = &args["update"];
    let mut body = json!({});
    if let Some(v) = u["displayName"].as_str() {
        body["name"] = json!(v);
    }
    if let Some(v) = u["description"].as_str() {
        body["description"] = json!(v);
    }
    if let Some(v) = u["location"].as_str() {
        body["location"] = json!(v);
    }
    if let Some(v) = u["birthday"].as_str() {
        body["birthday"] = json!(v);
    }
    if let Some(v) = u["avatarMediaId"].as_str() {
        body["avatarId"] = json!(v);
    }
    if let Some(v) = u["bannerMediaId"].as_str() {
        body["bannerId"] = json!(v);
    }
    if let Some(fields) = u["fields"].as_array() {
        body["fields"] = json!(fields);
    }
    let me = misskey_fetch(args, "i/update", body)?;
    Ok(json!({ "profile": {
        "user": user_wire(&me, &host),
        "description": me["description"],
        "location": me["location"],
        "fields": me["fields"],
    } }))
}

/// リスト 1 件 → UserListInfo ワイヤ。
fn list_wire(list: &Value) -> Value {
    json!({
        "id": list["id"],
        "name": list["name"],
        "memberIds": list["userIds"],
        "isPublic": list["isPublic"].as_bool().unwrap_or(false),
    })
}

/// `call("sns.lists")` / `sns.listInfo` / `sns.createList` /
/// `sns.updateListMember` (D-136): リスト操作をまとめて実装。
fn sns_lists(args: &Value) -> Result<Value, Value> {
    let list = misskey_fetch(args, "users/lists/list", json!({}))?;
    Ok(json!({ "lists": list.as_array().cloned().unwrap_or_default().iter().map(list_wire).collect::<Vec<Value>>() }))
}

fn sns_list_info(args: &Value) -> Result<Value, Value> {
    let list = misskey_fetch(args, "users/lists/show", json!({ "listId": args["id"] }))?;
    Ok(json!({ "list": list_wire(&list) }))
}

/// リストのメンバーは `userIds` しか返らないので、ユーザーを引き直す。
fn sns_list_members(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let list = misskey_fetch(args, "users/lists/show", json!({ "listId": args["id"] }))?;
    let ids: Vec<Value> = list["userIds"].as_array().cloned().unwrap_or_default();
    if ids.is_empty() {
        return Ok(json!({ "users": [] }));
    }
    let users = misskey_fetch(args, "users/show", json!({ "userIds": ids }))?;
    Ok(json!({ "users": users.as_array().cloned().unwrap_or_default().iter().map(|u| user_wire(u, &host)).collect::<Vec<Value>>() }))
}

fn sns_create_list(args: &Value) -> Result<Value, Value> {
    let list = misskey_fetch(args, "users/lists/create", json!({ "name": args["name"] }))?;
    Ok(json!({ "list": list_wire(&list) }))
}

fn sns_update_list_member(args: &Value) -> Result<Value, Value> {
    let endpoint = if args["add"].as_bool().unwrap_or(false) { "users/lists/push" } else { "users/lists/pull" };
    misskey_fetch(args, endpoint, json!({ "listId": args["listId"], "userId": args["userId"] })).map(|_| json!({}))
}

/// `call("sns.users", {ids})` → `{profiles: [...]}`(ユーザー一括取得)。
fn sns_users(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let ids: Vec<Value> = args["ids"].as_array().cloned().unwrap_or_default();
    if ids.is_empty() {
        return Ok(json!({ "profiles": [] }));
    }
    let users = misskey_fetch(args, "users/show", json!({ "userIds": ids }))?;
    let profiles: Vec<Value> = users
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|u| {
            json!({
                "user": user_wire(u, &host),
                "description": u["description"],
                "followersCount": u["followersCount"],
                "followingCount": u["followingCount"],
                "postsCount": u["notesCount"],
                "isLocked": u["isLocked"],
                "isBot": u["isBot"],
            })
        })
        .collect();
    Ok(json!({ "profiles": profiles }))
}

/// `call("sns.reactedPosts", {limit, cursor?})` (D-103): 自分がリアクションした投稿。
fn sns_reacted_posts(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let mut body = json!({ "limit": args["limit"].as_u64().unwrap_or(20).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    // 自分の id は `i` から。reactions は {id, note, type, createdAt}。
    let me = misskey_fetch(args, "i", json!({}))?;
    body["userId"] = me["id"].clone();
    let list = misskey_fetch(args, "users/reactions", body)?;
    let rows: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let items: Vec<Value> = rows.iter().map(|r| post_wire(&r["note"], &host)).collect();
    let next = rows.last().and_then(|r| r["id"].as_str());
    Ok(json!({ "page": { "items": items, "nextCursor": next } }))
}

/// クリップ (D-032): Misskey ネイティブのクリップ機能。
fn clip_wire(clip: &Value) -> Value {
    json!({
        "id": clip["id"],
        "name": clip["name"],
        "description": clip["description"],
        "isPublic": clip["isPublic"].as_bool().unwrap_or(false),
        "notesCount": clip["notesCount"],
    })
}

fn sns_clips(args: &Value) -> Result<Value, Value> {
    // userId 指定は他ユーザーの公開クリップ、省略は自分の全クリップ。
    let (endpoint, body) = match args["userId"].as_str() {
        Some(id) => ("users/clips", json!({ "userId": id, "limit": 100 })),
        None => ("clips/list", json!({})),
    };
    let list = misskey_fetch(args, endpoint, body)?;
    Ok(json!({ "clips": list.as_array().cloned().unwrap_or_default().iter().map(clip_wire).collect::<Vec<Value>>() }))
}

fn sns_clip_notes(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let mut body = json!({ "clipId": args["clipId"], "limit": args["limit"].as_u64().unwrap_or(20).min(100) });
    if let Some(cursor) = args["cursor"].as_str() {
        body["untilId"] = json!(cursor);
    }
    let list = misskey_fetch(args, "clips/notes", body)?;
    let notes: Vec<Value> = list.as_array().cloned().unwrap_or_default();
    let next = notes.last().and_then(|n| n["id"].as_str());
    Ok(json!({ "page": { "items": notes.iter().map(|n| post_wire(n, &host)).collect::<Vec<Value>>(), "nextCursor": next } }))
}

/// `call("sns.userPlays", {userId})` (D-039): Play(Flash)は RASQ 内では
/// 実行せず**外部リンク**として出す。
fn sns_user_plays(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io").to_string();
    let list = misskey_fetch(args, "users/flashs", json!({ "userId": args["userId"], "limit": 50 }))?;
    let plays: Vec<Value> = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|f| {
            json!({
                "id": f["id"],
                "title": f["title"],
                "summary": f["summary"],
                "url": format!("https://{host}/play/{}", f["id"].as_str().unwrap_or("")),
            })
        })
        .collect();
    Ok(json!({ "plays": plays }))
}

/// `call("timeline", {host?, limit?})`: fetch the local timeline through the
/// host's `net.fetch`, rebuild the tree, bump rev. The host then pulls the
/// new `snapshot()` (result carries `treeUpdated: true`).
fn timeline(args: &Value) -> Result<Value, Value> {
    let (notes, host) = fetch_notes(args, "local")?;
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
        "sns.emojis" => sns_emojis(&args_value),
        // 通知 / 関係 / DM / 検索 (D-134): 残りの SnsSession 契約を写した分。
        "sns.inspectServer" => sns_inspect_server(&args_value),
        "sns.timelines" => sns_timelines(&args_value),
        "sns.capabilities" => sns_capabilities(&args_value),
        "sns.settingsRead" => sns_settings_read(&args_value),
        "sns.settingsWrite" => sns_settings_write(&args_value),
        "sns.notifications" => sns_notifications(&args_value),
        "sns.markNotificationsRead" => misskey_fetch(&args_value, "notifications/mark-all-as-read", json!({})).map(|_| json!({})),
        "sns.relationships" => sns_relationships(&args_value),
        "sns.setRelation" => sns_set_relation(&args_value),
        "sns.follows" => sns_follows(&args_value),
        "sns.userList" => sns_user_list(&args_value),
        "sns.resolveFollowRequest" => {
            let endpoint = if args_value["accept"].as_bool().unwrap_or(false) { "following/requests/accept" } else { "following/requests/reject" };
            misskey_fetch(&args_value, endpoint, json!({ "userId": args_value["id"] })).map(|_| json!({}))
        }
        "sns.reportUser" => sns_report_user(&args_value),
        "sns.search" => sns_search(&args_value),
        "sns.trendingTags" => sns_trending_tags(&args_value),
        "sns.favorites" => sns_favorites(&args_value),
        "sns.dmRooms" => sns_dm_rooms(&args_value),
        "sns.sendDm" => sns_send_dm(&args_value),
        // メディア / プロフィール / リスト / クリップ / Play (D-136)。
        "sns.uploadMedia" => sns_upload_media(&args_value),
        "sns.updateProfile" => sns_update_profile(&args_value),
        "sns.lists" => sns_lists(&args_value),
        "sns.listInfo" => sns_list_info(&args_value),
        "sns.listMembers" => sns_list_members(&args_value),
        "sns.createList" => sns_create_list(&args_value),
        "sns.updateListMember" => sns_update_list_member(&args_value),
        "sns.users" => sns_users(&args_value),
        "sns.reactedPosts" => sns_reacted_posts(&args_value),
        "sns.clips" => sns_clips(&args_value),
        "sns.clipInfo" => misskey_fetch(&args_value, "clips/show", json!({ "clipId": args_value["clipId"] })).map(|c| json!({ "clip": clip_wire(&c) })),
        "sns.clipNotes" => sns_clip_notes(&args_value),
        "sns.createClip" => misskey_fetch(&args_value, "clips/create", json!({ "name": args_value["name"], "description": args_value["description"], "isPublic": args_value["isPublic"] })).map(|c| json!({ "clip": clip_wire(&c) })),
        "sns.deleteClip" => misskey_fetch(&args_value, "clips/delete", json!({ "clipId": args_value["clipId"] })).map(|_| json!({})),
        "sns.updateClipNote" => {
            let endpoint = if args_value["add"].as_bool().unwrap_or(false) { "clips/add-note" } else { "clips/remove-note" };
            misskey_fetch(&args_value, endpoint, json!({ "clipId": args_value["clipId"], "noteId": args_value["postId"] })).map(|_| json!({}))
        }
        "sns.userPlays" => sns_user_plays(&args_value),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// D-100: 純リノートは中身のノートへ展開し repostedBy を載せる
    /// (展開しないと本文も quote も無い「内容のない投稿」になる)。
    #[test]
    fn pure_renote_unwraps_with_reposted_by() {
        let note = json!({
            "id": "outer1",
            "createdAt": "2026-08-31T00:00:00Z",
            "text": null,
            "fileIds": [],
            "poll": null,
            "user": { "id": "u2", "username": "renoter", "name": "リノートする人" },
            "renote": {
                "id": "inner1",
                "createdAt": "2026-08-30T00:00:00Z",
                "text": "中身の本文",
                "fileIds": [],
                "user": { "id": "u1", "username": "author", "name": "元の人" },
            },
        });
        let wire = post_wire(&note, "example.test");
        assert_eq!(wire["id"], "outer1"); // アクションは外側の id へ
        assert_eq!(wire["body"], "中身の本文");
        assert_eq!(wire["author"]["handle"], "author");
        assert_eq!(wire["repostedBy"]["handle"], "renoter");
    }

    /// 引用リノート(text あり)は従来どおり body + quote。
    #[test]
    fn quote_renote_keeps_body_and_quote() {
        let note = json!({
            "id": "q1",
            "text": "これすごい",
            "fileIds": [],
            "user": { "id": "u2", "username": "quoter" },
            "renote": { "id": "inner1", "text": "元ネタ", "user": { "id": "u1", "username": "author" } },
        });
        let wire = post_wire(&note, "example.test");
        assert_eq!(wire["body"], "これすごい");
        assert_eq!(wire["quote"]["body"], "元ネタ");
        assert!(wire["repostedBy"].is_null());
    }

    /// 添付だけのリノート(text 空 + files)は純リノートではない。
    #[test]
    fn renote_with_files_is_not_pure() {
        let note = json!({
            "id": "m1",
            "text": null,
            "fileIds": ["f1"],
            "files": [{ "id": "f1", "url": "https://x.invalid/1.png", "type": "image/png" }],
            "user": { "id": "u2", "username": "quoter" },
            "renote": { "id": "inner1", "text": "元ネタ", "user": { "id": "u1", "username": "author" } },
        });
        let wire = post_wire(&note, "example.test");
        assert_eq!(wire["id"], "m1");
        assert!(wire["repostedBy"].is_null());
        assert_eq!(wire["media"].as_array().unwrap().len(), 1);
    }

    // ── D-134: 通知 / 関係 / TL 種別のマッピング ────────────────────

    /// CONTRACT §6 の TimelineKind.id → Misskey エンドポイント。
    /// `list:` / `tag:` は接頭辞を剥がして本文パラメータへ移す。
    #[test]
    fn timeline_ids_map_to_misskey_endpoints() {
        assert_eq!(timeline_endpoint("home").0, "notes/timeline");
        assert_eq!(timeline_endpoint("local").0, "notes/local-timeline");
        assert_eq!(timeline_endpoint("social").0, "notes/hybrid-timeline");
        assert_eq!(timeline_endpoint("global").0, "notes/global-timeline");
        assert_eq!(timeline_endpoint("featured").0, "notes/featured");

        let (ep, extra) = timeline_endpoint("list:abc123");
        assert_eq!(ep, "notes/user-list-timeline");
        assert_eq!(extra["listId"], "abc123");

        let (ep, extra) = timeline_endpoint("tag:ロケット");
        assert_eq!(ep, "notes/search-by-tag");
        assert_eq!(extra["tag"], "ロケット");

        // 未知の id はホーム TL に倒す(縮退)。
        assert_eq!(timeline_endpoint("relational").0, "notes/timeline");
    }

    #[test]
    fn notification_types_map_onto_the_four_tile_kinds() {
        assert_eq!(notification_kind("renote"), "repost");
        assert_eq!(notification_kind("receiveFollowRequest"), "followRequest");
        assert_eq!(notification_kind("followRequestAccepted"), "followAccepted");
        // D-045: 本文つきのサーバー/アプリ通知はシステム通知へ。
        assert_eq!(notification_kind("achievementEarned"), "system");
        assert_eq!(notification_kind("app"), "system");
        // 知らない種別でも落とさない。
        assert_eq!(notification_kind("brandNewThing"), "other");
    }

    /// グループ通知(`i/notifications-grouped`)は users / reactions を持つ。
    /// actor 配列と actor→リアクションの対応表に畳んで返す。
    #[test]
    fn grouped_reaction_notification_folds_actors_and_reactions() {
        let n = json!({
            "id": "n1",
            "type": "reaction",
            "createdAt": "2026-09-01T00:00:00Z",
            "isRead": false,
            "users": [
                { "id": "u1", "username": "alice" },
                { "id": "u2", "username": "bob" },
            ],
            "reactions": [
                { "type": "🎉", "user": { "id": "u1", "username": "alice" } },
                { "type": "👍", "user": { "id": "u2", "username": "bob" } },
            ],
            "note": { "id": "p1", "text": "本文", "user": { "id": "me", "username": "me" } },
        });
        let wire = notification_wire(&n, "example.test");
        assert_eq!(wire["kind"], "reaction");
        assert_eq!(wire["read"], false);
        assert_eq!(wire["actors"].as_array().unwrap().len(), 2);
        assert_eq!(wire["actors"][0]["handle"], "alice");
        assert_eq!(wire["reactionsByActor"]["u1"], "🎉");
        assert_eq!(wire["reactionsByActor"]["u2"], "👍");
        assert_eq!(wire["post"]["body"], "本文");
    }

    /// 単体通知は `user` 1 件。post が無い種別では null のままにする。
    #[test]
    fn single_follow_notification_has_one_actor_and_no_post() {
        let n = json!({
            "id": "n2",
            "type": "follow",
            "createdAt": "2026-09-01T00:00:00Z",
            "isRead": true,
            "user": { "id": "u9", "username": "newfollower", "name": "新しい人" },
        });
        let wire = notification_wire(&n, "example.test");
        assert_eq!(wire["kind"], "follow");
        assert_eq!(wire["read"], true);
        assert_eq!(wire["actors"].as_array().unwrap().len(), 1);
        assert_eq!(wire["actors"][0]["displayName"], "新しい人");
        assert!(wire["post"].is_null());
        assert!(wire["reactionsByActor"].as_object().unwrap().is_empty());
    }

    /// `users/relation` → FollowState ワイヤ。欠けたフラグは false。
    #[test]
    fn relation_maps_onto_follow_state() {
        let r = json!({
            "id": "u1",
            "isFollowing": true,
            "isFollowed": false,
            "hasPendingFollowRequestFromYou": false,
            "isMuted": true,
            "isBlocking": false,
            "isBlocked": true,
        });
        let wire = follow_state_wire(&r);
        assert_eq!(wire["following"], true);
        assert_eq!(wire["followedBy"], false);
        assert_eq!(wire["muted"], true);
        assert_eq!(wire["blocked"], false);
        assert_eq!(wire["blockedBy"], true);

        // 空オブジェクトでも全部 false で返る(欠けたフラグで落ちない)。
        let empty = follow_state_wire(&json!({}));
        for key in ["following", "followedBy", "requested", "muted", "blocked", "blockedBy"] {
            assert_eq!(empty[key], false, "{key} should default to false");
        }
    }

    /// 鍵アカウントへのフォローは `requested` 止まり — セッターの戻りは
    /// **サーバーを読み直した状態**なので、これがそのまま出る。
    #[test]
    fn pending_follow_request_shows_as_requested_not_following() {
        let wire = follow_state_wire(&json!({ "hasPendingFollowRequestFromYou": true }));
        assert_eq!(wire["requested"], true);
        assert_eq!(wire["following"], false);
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
