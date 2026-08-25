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
        "t": "PostTile", "k": "post-sample-1", "role": "contentBody",
        "p": { "post": "$.post", "variant": "normal" },
        "c": [
            { "t": "RichText", "k": "body", "tag": "main_text", "p": { "source": "$.post.bodyRSL" } },
            {
                "t": "ActionBar", "k": "actions", "role": "actionBar",
                "p": { "actions": "$.actions", "states": "$.actionStates[post.key]" }
            }
        ]
    })
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

/// Demo `patch` (baseRev 1 -> rev 2): only meaningful on the initial tree.
#[no_mangle]
pub extern "C" fn patch() -> u64 {
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
        "t": "ActionBar", "k": "actions", "role": "actionBar",
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

/// `call("timeline", {host?, limit?})`: fetch the local timeline through the
/// host's `net.fetch`, rebuild the tree, bump rev. The host then pulls the
/// new `snapshot()` (result carries `treeUpdated: true`).
fn timeline(args: &Value) -> Result<Value, Value> {
    let host = args["host"].as_str().unwrap_or("misskey.io");
    let limit = args["limit"].as_u64().unwrap_or(10).min(30);
    let response = host_call(
        "net.fetch",
        json!({
            "method": "POST",
            "url": format!("https://{host}/api/notes/local-timeline"),
            "headers": { "Content-Type": "application/json" },
            "body": json!({ "limit": limit }).to_string(),
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
    if status != 200 {
        return Err(json!({ "code": "B03", "message": format!("HTTP {status} from {host}") }));
    }
    let notes: Vec<Value> = serde_json::from_str(value["bodyText"].as_str().unwrap_or("[]"))
        .map_err(|e| json!({ "code": "D01", "message": format!("bad timeline JSON: {e}") }))?;

    let tiles: Vec<Value> = notes.iter().map(|n| post_tile(n, host)).collect();
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
        "openThread" | "openUser" | "openViewer" | "performAction" | "pickReaction" => {
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
