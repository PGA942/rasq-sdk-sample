//! Minimal WASM plugin sample for RASQ M3 (PROTOCOL.md draft v0.1).
//!
//! Emits `snapshot` / `patch` messages and answers `call` messages, all as
//! `rasq.boundary/1` JSON (wire format v1 per PROTOCOL.md §3). Exported
//! functions return a packed `(ptr << 32) | len` u64 so a single i64 return
//! value can describe a buffer in wasm32 linear memory without multi-value
//! support.

use serde_json::json;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};

const SCOPE: &str = "rasq.official.misskey:misskey:misskey.io:sample:0.1.0:tl-home";

#[link(wasm_import_module = "rasq")]
extern "C" {
    /// Host function import (PROTOCOL.md §4.1): request JSON in, packed
    /// `(ptr << 32) | len` response location out. Synchronous v1.
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

/// Calls a host function (`rasq.hostfn/1`) and returns its parsed response.
fn host_call(fn_name: &str, args: serde_json::Value) -> serde_json::Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
    let bytes = request.as_bytes();
    let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
    let ptr = (packed >> 32) as u32;
    let len = (packed & 0xFFFF_FFFF) as u32;
    // The host wrote the response via our exported `alloc`; reclaim it here.
    let response = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
    serde_json::from_slice(&response).unwrap_or(serde_json::Value::Null)
}

fn pack(mut buf: Vec<u8>) -> u64 {
    // `dealloc` reconstructs this buffer as `Vec::from_raw_parts(ptr, len, len)`,
    // so capacity must equal `len` exactly — `to_string()`/`push_str` growth
    // usually leaves spare capacity, which would make that reconstruction UB.
    buf.shrink_to_fit();
    let len = buf.len() as u64;
    let ptr = buf.as_ptr() as u64;
    mem::forget(buf);
    (ptr << 32) | len
}

/// Reserve `len` bytes in linear memory for the host to write into
/// (used before a `call`), returning the pointer.
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr() as u32;
    mem::forget(buf);
    ptr
}

/// Release a buffer previously returned by `alloc`, `snapshot`, `patch` or
/// `call`.
#[no_mangle]
pub extern "C" fn dealloc(ptr: u32, len: u32) {
    unsafe {
        drop(Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize));
    }
}

/// Initial `snapshot` (rev 1): a single PostTile-shaped RRL tree, following
/// the node shape in `examples/post-tile.rrl.json`.
#[no_mangle]
pub extern "C" fn snapshot() -> u64 {
    let msg = json!({
        "schema": "rasq.boundary/1",
        "scope": SCOPE,
        "type": "snapshot",
        "rev": 1,
        "root": {
            "t": "PostTile", "k": "post-sample-1", "role": "contentBody",
            "p": { "post": "$.post", "variant": "normal" },
            "c": [
                { "t": "RichText", "k": "body", "tag": "main_text", "p": { "source": "$.post.bodyRSL" } },
                {
                    "t": "ActionBar", "k": "actions", "role": "actionBar",
                    "p": { "actions": "$.actions", "states": "$.actionStates[post.key]" }
                }
            ]
        }
    });
    pack(msg.to_string().into_bytes())
}

/// Follow-up `patch` (baseRev 1 -> rev 2): flips the reaction count shown on
/// the sample PostTile, exercising the `setProp` op.
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

/// Answers a host `call { callId, fn, args }` (PROTOCOL.md §3.2) with a
/// `result` message (§3.1). The sample only implements `openThread`, which
/// echoes its args back so the host<->wasm round trip can be verified.
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
    let args_value: serde_json::Value =
        serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);

    let msg = match fn_name.as_str() {
        // Demo of the host-function boundary: storage.set → storage.get and
        // report what came back (the host implements storage.*).
        "storageRoundTrip" => {
            let key = args_value["key"].clone();
            let value = args_value["value"].clone();
            let set = host_call("storage.set", json!({"ns": "safe", "key": key, "value": value}));
            let got = host_call("storage.get", json!({"ns": "safe", "key": args_value["key"]}));
            let ok = set["ok"].as_bool().unwrap_or(false) && got["ok"].as_bool().unwrap_or(false);
            let mut msg = json!({
                "schema": "rasq.boundary/1",
                "scope": SCOPE,
                "type": "result",
                "callId": call_id,
                "ok": ok,
            });
            if ok {
                msg["value"] = json!({"stored": got["value"]["value"]});
            } else {
                msg["error"] = got
                    .get("error")
                    .cloned()
                    .unwrap_or(json!({"code": "E00", "message": "host_fn failed"}));
            }
            msg
        }
        "openThread" => json!({
            "schema": "rasq.boundary/1",
            "scope": SCOPE,
            "type": "result",
            "callId": call_id,
            "ok": true,
            "value": { "echo": args_value }
        }),
        other => json!({
            "schema": "rasq.boundary/1",
            "scope": SCOPE,
            "type": "result",
            "callId": call_id,
            "ok": false,
            "error": { "code": "C01", "message": format!("unsupported fn: {other}") }
        }),
    };
    pack(msg.to_string().into_bytes())
}

fn read_str(ptr: u32, len: u32) -> String {
    unsafe {
        let slice = std::slice::from_raw_parts(ptr as *const u8, len as usize);
        String::from_utf8_lossy(slice).into_owned()
    }
}
