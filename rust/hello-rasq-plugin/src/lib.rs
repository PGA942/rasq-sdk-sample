//! hello-rasq-plugin — RASQ プラグインの**最小サンプル** (D-077)。
//!
//! ネットワークを使わず、SDK の各項目を 1 つずつ最小の形で見せる:
//!
//! | 項目 | 入口 |
//! |---|---|
//! | RRL 画面 (snapshot) | [`snapshot`](snapshot) / [`initial_root`](initial_root) |
//! | イベント → call | ボタン `on: {tap: "greet"}` → [`call`](call) の `"greet"` |
//! | ホスト関数 (storage) | `greet` 内の `storage.get/set` |
//! | 差分更新 (patch) | `greet` が本体 patch を積み `patch: true` で引き取らせる |
//! | FOW surface | `"openPanel"` → `fow.open`、`+1` → **surface patch** |
//! | ライフサイクル / 閉時通知 | `"lifecycle"` / `"panelClosed"`(fow.open の onClosed) |
//! | SNS 抽象関数 | `"sns.timeline"` / `"sns.actions"` / `"sns.performAction"`(canned) |
//! | 利用者に見せる文 (i18n) | [`TEXT`] の表を `args.locale`(無ければ `ja`)で引く(PROTOCOL.md §4.6) |
//!
//! ビルド: `cargo build --release --target wasm32-unknown-unknown -p hello-rasq-plugin`
//! 試し方: rasq-client のカタログ →「WASMプラグイン試験 (dev)」で
//! `.wasm のパスからインポート` に `rust/target/wasm32-unknown-unknown/release/hello_rasq_plugin.wasm` を指定。
//! 「境界TL」を押すと sns.timeline の canned 投稿がホストの投稿タイルで出る。
//!
//! 規約の正典は [`PROTOCOL.md`](../../PROTOCOL.md) と [`schema/`](../../schema/)。

use rasq_plugin_kit::i18n;
use serde_json::{json, Value};
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// instanceScope: `pluginId:accountKey:pluginVersion:instanceId`(PROTOCOL §1)。
const SCOPE: &str = "example.hello:misskey:example.test:hello:0.1.0:main";

// ---------------------------------------------------------------------------
// ボイラープレート(全プラグイン共通): wasm32 線形メモリの受け渡し。
// 返り値は `(ptr << 32) | len` に詰めた u64。ホストは `alloc` で書き込み、
// プラグインは `dealloc` で返す(PROTOCOL §4.1)。
// ---------------------------------------------------------------------------

#[link(wasm_import_module = "rasq")]
extern "C" {
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

/// ホスト関数(`rasq.hostfn/1`)を呼んでレスポンス JSON を返す。
fn host_call(fn_name: &str, args: Value) -> Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
    let bytes = request.as_bytes();
    let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
    let (ptr, len) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
    let response = unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
    serde_json::from_slice(&response).unwrap_or(Value::Null)
}

fn pack(mut buf: Vec<u8>) -> u64 {
    buf.shrink_to_fit();
    let (ptr, len) = (buf.as_ptr() as u64, buf.len() as u64);
    mem::forget(buf);
    (ptr << 32) | len
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
    unsafe { drop(Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize)) }
}

fn read_str(ptr: u32, len: u32) -> String {
    unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(ptr as *const u8, len as usize)).into_owned() }
}

// ---------------------------------------------------------------------------
// 状態: 本体ツリーの rev / 挨拶回数 / FOW カウンター / 保留 patch / 通知ログ。
// ---------------------------------------------------------------------------

static REV: AtomicU64 = AtomicU64::new(1);
static GREETS: AtomicU64 = AtomicU64::new(0);
static PANEL_COUNT: AtomicU64 = AtomicU64::new(0);
static PENDING_PATCH: Mutex<Value> = Mutex::new(Value::Null);
static LAST_NOTIFY: Mutex<Value> = Mutex::new(Value::Null);

// ---------------------------------------------------------------------------
// 0. 利用者に見せる文の表(PROTOCOL.md §4.6)。文はリテラルで書かず、キーで引く。
//    言語を足すときは `"en": { … }` を足すだけ(無いキーは ja に落ちる)。
// ---------------------------------------------------------------------------

rasq_plugin_kit::i18n_catalog! {
    pub static TEXT = {
        "ja": {
            "greeting" => "こんにちは、RASQ!",
            "greeting.count" => "こんにちは!({n} 回目)",
            "button.greet" => "挨拶する",
            "button.panel" => "パネルを開く",
            "panel.title" => "hello パネル",
            "panel.close" => "閉じる",
            "panel.count" => "{n} 回",
            "action.wave" => "手を振る",
            "action.received" => "{action} を受け取りました",
        },
    };
}

// ---------------------------------------------------------------------------
// 1. RRL 画面(画面コンストラクタ): snapshot がツリー全体を返す。
//    ノード形式は schema/rrl-node.schema.json、L2 はホスト実装(Props のみ)。
// ---------------------------------------------------------------------------

fn root() -> Value {
    // snapshot() に args は無いので、いちばん新しい call の args.locale(まだ無ければ ja)。
    let loc = i18n::current();
    let t = |k| TEXT.t(&loc, k);
    json!({
        "t": "Column", "k": "root", "c": [
            { "t": "Text", "k": "greeting", "p": { "text": t("greeting") } },
            // on の値は call で受ける自分の関数名(イベント → call 写像)。
            { "t": "Button", "k": "greet-btn", "p": { "label": t("button.greet") }, "on": { "tap": "greet" } },
            { "t": "Button", "k": "panel-btn", "p": { "label": t("button.panel") }, "on": { "tap": "openPanel" } }
        ]
    })
}

#[no_mangle]
pub extern "C" fn snapshot() -> u64 {
    let msg = json!({
        "schema": "rasq.boundary/1", "scope": SCOPE, "type": "snapshot",
        "rev": REV.load(Ordering::Relaxed), "root": root(),
    });
    pack(msg.to_string().into_bytes())
}

/// ホストが `patch: true` を見て引き取りに来る保留パッチ(無ければ空 ops)。
#[no_mangle]
pub extern "C" fn patch() -> u64 {
    let pending = mem::replace(&mut *PENDING_PATCH.lock().unwrap(), Value::Null);
    let msg = if pending.is_null() {
        let rev = REV.load(Ordering::Relaxed);
        json!({"schema": "rasq.boundary/1", "scope": SCOPE, "type": "patch", "rev": rev, "baseRev": rev, "ops": []})
    } else {
        pending
    };
    pack(msg.to_string().into_bytes())
}

// ---------------------------------------------------------------------------
// 2. イベントハンドラー: ホストの `call {fn, args}` に `result` で答える。
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn call(fn_ptr: u32, fn_len: u32, args_ptr: u32, args_len: u32, call_id_ptr: u32, call_id_len: u32) -> u64 {
    let fn_name = read_str(fn_ptr, fn_len);
    let args: Value = serde_json::from_str(&read_str(args_ptr, args_len)).unwrap_or(Value::Null);
    let call_id = read_str(call_id_ptr, call_id_len);
    // ホストは UI の言語を毎回 args.locale に載せる(無ければ ja)。snapshot() のために覚えておく。
    let loc = i18n::set_current(&args);

    let outcome: Result<Value, Value> = match fn_name.as_str() {
        // --- ホスト関数(storage)+ 本体 patch --------------------------
        // 挨拶回数を storage.safe に永続化し、Text ノードを setProp で更新。
        "greet" => {
            let stored = host_call("storage.get", json!({"ns": "safe", "key": "greets"}));
            let before = stored["value"]["value"].as_u64().unwrap_or(0);
            let n = before + 1;
            GREETS.store(n, Ordering::Relaxed);
            host_call("storage.set", json!({"ns": "safe", "key": "greets", "value": n}));
            let base = REV.fetch_add(1, Ordering::Relaxed);
            *PENDING_PATCH.lock().unwrap() = json!({
                "schema": "rasq.boundary/1", "scope": SCOPE, "type": "patch",
                "rev": base + 1, "baseRev": base,
                "ops": [{ "op": "setProp", "target": format!("{SCOPE}:greeting"), "prop": "text", "value": TEXT.f(&loc, "greeting.count", &[("n", &n.to_string())]) }]
            });
            Ok(json!({ "patch": true, "greets": n }))
        }

        // --- FOW surface: fow.open + surface patch ----------------------
        "openPanel" => {
            PANEL_COUNT.store(0, Ordering::Relaxed);
            let r = host_call("fow.open", json!({
                "id": "hello-panel",
                "spec": { "type": "widget", "confirmMode": "immediate", "initialHeightFactor": 0.3, "title": TEXT.t(&loc, "panel.title"), "cancelText": TEXT.t(&loc, "panel.close") },
                "onClosed": "panelClosed",
                "root": { "t": "Column", "k": "panel-root", "c": [
                    { "t": "Text", "k": "panel-count", "p": { "text": TEXT.f(&loc, "panel.count", &[("n", "0")]) } },
                    { "t": "Button", "k": "panel-plus", "p": { "label": "+1" }, "on": { "tap": "panelPlus" } }
                ] }
            }));
            Ok(json!({ "opened": r["value"]["opened"] }))
        }
        "panelPlus" => {
            let n = PANEL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            *PENDING_PATCH.lock().unwrap() = json!({
                "schema": "rasq.boundary/1", "scope": SCOPE, "type": "patch",
                "surface": "hello-panel", "rev": n + 1, "baseRev": n,
                "ops": [{ "op": "setProp", "target": format!("{SCOPE}:panel-count"), "prop": "text", "value": TEXT.f(&loc, "panel.count", &[("n", &n.to_string())]) }]
            });
            Ok(json!({ "patch": true }))
        }

        // --- 閉時通知 / ライフサイクル(未対応でも C01 でよい)-----------
        "panelClosed" | "lifecycle" => {
            *LAST_NOTIFY.lock().unwrap() = json!({ "fn": fn_name, "args": args });
            Ok(json!({}))
        }
        "lastNotify" => Ok(LAST_NOTIFY.lock().unwrap().clone()),

        // --- SNS 抽象関数(SnsSession 契約の boundary 写像)---------------
        // 最小サンプルなのでネットワークは使わず canned な Post ワイヤを返す。
        "sns.timeline" => Ok(json!({ "page": { "items": [
            wire_post("h1", "hello プラグインからの投稿です"),
            wire_post("h2", "Post ワイヤ (rasq.post/1) がそのまま投稿タイルになります"),
        ], "nextCursor": null } })),
        "sns.actions" => Ok(json!({ "actions": [
            { "id": "wave", "genreTag": "Reaction", "label": TEXT.t(&loc, "action.wave"), "icon": "favorite_border", "doneIcon": "favorite" }
        ] })),
        "sns.performAction" => Ok(json!({ "result": { "done": true, "message": TEXT.f(&loc, "action.received", &[("action", &args["actionId"].to_string())]) } })),

        other => Err(json!({ "code": "C01", "message": format!("unsupported fn: {other}") })),
    };

    let mut msg = json!({ "schema": "rasq.boundary/1", "scope": SCOPE, "type": "result", "callId": call_id, "ok": outcome.is_ok() });
    match outcome {
        Ok(value) => msg["value"] = value,
        Err(error) => msg["error"] = error,
    }
    pack(msg.to_string().into_bytes())
}

/// 最小の Post ワイヤ(`rasq.post/1`)。閲覧アカウントのキーはホストが与える。
fn wire_post(id: &str, body: &str) -> Value {
    json!({
        "id": id,
        "author": { "id": "hello", "kind": "misskey", "instance": "example.test", "handle": "hello", "displayName": "hello プラグイン" },
        "createdAt": "2026-08-30T12:00:00.000Z",
        "body": body,
        "bodyFormat": "plain",
        "counts": { "reply": 0, "renote": 0, "reaction": 1 },
        "reactions": { "👋": 1 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文は表から(PROTOCOL.md §4.6)。いまは ja だけなので、どの言語でも ja と同じ文。
    #[test]
    fn ui_text_comes_from_the_table_and_falls_back_to_ja() {
        assert!(TEXT.check().is_empty(), "{:?}", TEXT.check());
        let _ = i18n::take_misses();
        for loc in [json!("en"), json!("zh-Hant"), Value::Null] {
            i18n::set_current(&json!({ "locale": loc }));
            assert_eq!(root()["c"][0]["p"]["text"], "こんにちは、RASQ!");
            assert_eq!(root()["c"][2]["p"]["label"], "パネルを開く");
        }
        assert_eq!(TEXT.f("en", "action.received", &[("action", &json!("wave").to_string())]), "\"wave\" を受け取りました");
        assert!(i18n::take_misses().is_empty());
    }
}
