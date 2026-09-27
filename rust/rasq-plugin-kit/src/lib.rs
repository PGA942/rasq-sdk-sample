//! rasq-plugin-kit — RASQ wasm プラグインの共通部品。
//!
//! `PROTOCOL.md` の wasm ABI(v1・同期)をそのまま包む:
//!
//! - [`rasq_plugin!`] マクロが `snapshot` / `patch` / `call` / `alloc` / `dealloc` を export
//! - [`host_call`] でホスト関数(`rasq.hostfn/1`)を呼ぶ(native ビルドではテスト用フックに差し替え可)
//! - [`rrl`] に RRL ノードのビルダー、[`Surface`] に FOW(拡張面)の open / patch / close
//! - [`set_patch`] / [`tree_updated`] で本体ツリーの差分/再取得を頼む
//!
//! プラグイン本体は「`root()` が返す RRL」と「`call(fn, args)` の match」だけ書けばよい。
//! 契約の正典は `docs/slots.md`(スロット契約)と `PROTOCOL.md`。plugin-sample の各リポジトリの `kit/` はこのクレートの複製(D-244i: ここが正典、`rasq init` がワークスペースへ写す)。

use serde_json::{json, Value};
use std::cell::RefCell;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub use serde_json;

// ---------------------------------------------------------------------------
// wasm ABI: 線形メモリの受け渡し(PROTOCOL §4.1)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "rasq")]
extern "C" {
    fn host_fn(ptr: u32, len: u32) -> u64;
}

static HOST_CALL_SEQ: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// native(テスト)でホスト関数を差し替える口: `set_host_hook(|fn, args| ...)`。
    static HOST_HOOK: RefCell<Option<Box<dyn FnMut(&str, Value) -> Value>>> = RefCell::new(None);
    /// native でのホスト関数呼び出し記録(テストが読む)。
    static HOST_LOG: RefCell<Vec<(String, Value)>> = RefCell::new(Vec::new());
}

/// テスト用: ホスト関数の応答を差し替える(戻り値は `value` 部分。`{ok:true,value}` に包まれる)。
pub fn set_host_hook<F: FnMut(&str, Value) -> Value + 'static>(f: F) {
    HOST_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

/// テスト用: これまでのホスト関数呼び出し `(fn, args)` を取り出す(取り出すと空になる)。
pub fn take_host_log() -> Vec<(String, Value)> {
    HOST_LOG.with(|l| mem::take(&mut *l.borrow_mut()))
}

/// ホスト関数(`rasq.hostfn/1`)を呼び、レスポンス JSON(`{callId, ok, value|error}`)を返す。
/// 権限外/未知は `{ok:false, error:{code:"E01"|"C01"}}` — トラップしない。
pub fn host_call(fn_name: &str, args: Value) -> Value {
    let call_id = format!("hf-{}", HOST_CALL_SEQ.fetch_add(1, Ordering::Relaxed));
    #[cfg(target_arch = "wasm32")]
    {
        let request = json!({"schema": "rasq.hostfn/1", "callId": call_id, "fn": fn_name, "args": args}).to_string();
        let bytes = request.as_bytes();
        let packed = unsafe { host_fn(bytes.as_ptr() as u32, bytes.len() as u32) };
        let (ptr, len) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
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

/// ホスト関数の `value`(失敗は Null)。
pub fn host_value(fn_name: &str, args: Value) -> Value {
    let r = host_call(fn_name, args);
    if r["ok"] == true { r["value"].clone() } else { Value::Null }
}

#[doc(hidden)]
pub fn pack(mut buf: Vec<u8>) -> u64 {
    buf.shrink_to_fit();
    let (ptr, len) = (buf.as_ptr() as u64, buf.len() as u64);
    mem::forget(buf);
    (ptr << 32) | len
}

#[doc(hidden)]
pub fn read_str(ptr: u32, len: u32) -> String {
    unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(ptr as *const u8, len as usize)).into_owned() }
}

#[doc(hidden)]
pub fn alloc_impl(len: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr() as u32;
    mem::forget(buf);
    ptr
}

#[doc(hidden)]
pub fn dealloc_impl(ptr: u32, len: u32) {
    unsafe { drop(Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize)) }
}

// ---------------------------------------------------------------------------
// 本体ツリーの rev / 保留 patch
// ---------------------------------------------------------------------------

static REV: AtomicU64 = AtomicU64::new(1);
static PENDING_PATCH: Mutex<Value> = Mutex::new(Value::Null);
static SCOPE: Mutex<String> = Mutex::new(String::new());

#[doc(hidden)]
pub fn set_scope(scope: &str) {
    *SCOPE.lock().unwrap() = scope.to_string();
}

/// instanceScope(`pluginId:accountKey:pluginVersion:instanceId`)。
pub fn scope() -> String {
    SCOPE.lock().unwrap().clone()
}

/// 本体ツリーの現在 rev。
pub fn rev() -> u64 {
    REV.load(Ordering::Relaxed)
}

/// ノード id(`{scope}:{key}`)。patch の target / FOW anchor に使う。
pub fn node_id(key: &str) -> String {
    format!("{}:{key}", scope())
}

/// 本体ツリーへの差分を積み、rev を進める。`call` の戻りにそのまま返す(`{patch:true}`)。
/// `ops` は `setProp` / `insert` / `remove` / `replaceSubtree` / `move`(→ [`rrl::set_prop`] 等)。
pub fn set_patch(ops: Vec<Value>) -> Value {
    let base = REV.fetch_add(1, Ordering::Relaxed);
    *PENDING_PATCH.lock().unwrap() = json!({
        "schema": "rasq.boundary/1", "scope": scope(), "type": "patch",
        "rev": base + 1, "baseRev": base, "ops": ops,
    });
    json!({"patch": true})
}

/// 本体ツリーを作り直したのでホストに snapshot を取り直させる(`{treeUpdated:true}`)。
pub fn tree_updated() -> Value {
    REV.fetch_add(1, Ordering::Relaxed);
    json!({"treeUpdated": true})
}

#[doc(hidden)]
pub fn snapshot_message(root: Value) -> Value {
    json!({"schema": "rasq.boundary/1", "scope": scope(), "type": "snapshot", "rev": rev(), "root": root})
}

#[doc(hidden)]
pub fn take_patch_message() -> Value {
    let pending = mem::replace(&mut *PENDING_PATCH.lock().unwrap(), Value::Null);
    if pending.is_null() {
        let r = rev();
        json!({"schema": "rasq.boundary/1", "scope": scope(), "type": "patch", "rev": r, "baseRev": r, "ops": []})
    } else {
        pending
    }
}

#[doc(hidden)]
pub fn result_message(call_id: &str, outcome: Result<Value, Value>) -> Value {
    let mut msg = json!({"schema": "rasq.boundary/1", "scope": scope(), "type": "result", "callId": call_id, "ok": outcome.is_ok()});
    match outcome {
        Ok(v) => msg["value"] = v,
        Err(e) => msg["error"] = e,
    }
    msg
}

/// `call` の失敗(`{code, message}`)。未対応の関数は `C01`。
pub fn err(code: &str, message: impl Into<String>) -> Value {
    json!({"code": code, "message": message.into()})
}

/// 未対応の関数(ホストは該当 UI を隠す)。
pub fn unsupported(fn_name: &str) -> Value {
    err("C01", format!("unsupported fn: {fn_name}"))
}

// ---------------------------------------------------------------------------
// FOW(拡張面): open / surface patch / close(docs/fow.md)
// ---------------------------------------------------------------------------

/// 1 つの FOW ドキュメント。open で rev 1、patch ごとに進む。
pub struct Surface {
    pub id: &'static str,
    rev: AtomicU64,
}

impl Surface {
    pub const fn new(id: &'static str) -> Self {
        Self { id, rev: AtomicU64::new(0) }
    }

    /// `fow.open`。`spec` は FowSpec(type / title / cancelText / confirmText / initialHeightFactor / attachTop)。
    /// `extra` に `anchor` / `parentId` / `onCancel` を足せる。戻りは受理されたか。
    pub fn open(&self, spec: Value, root: Value, on_closed: Option<&str>, extra: Value) -> bool {
        let mut args = json!({"id": self.id, "spec": spec, "root": root});
        if let Some(f) = on_closed {
            args["onClosed"] = json!(f);
        }
        if let Value::Object(m) = extra {
            for (k, v) in m {
                args[k] = v;
            }
        }
        self.rev.store(1, Ordering::Relaxed);
        host_call("fow.open", args)["value"]["opened"] == true
    }

    /// 開いたままの差分。`call` の戻りに返す(`{patch:true}`)。
    pub fn patch(&self, ops: Vec<Value>) -> Value {
        let base = self.rev.fetch_add(1, Ordering::Relaxed).max(1);
        *PENDING_PATCH.lock().unwrap() = json!({
            "schema": "rasq.boundary/1", "scope": scope(), "type": "patch",
            "surface": self.id, "rev": base + 1, "baseRev": base, "ops": ops,
        });
        json!({"patch": true})
    }

    pub fn close(&self, result: Value) {
        host_call("fow.close", json!({"id": self.id, "result": result}));
    }

    pub fn current_rev(&self) -> u64 {
        self.rev.load(Ordering::Relaxed)
    }
}

/// よく使う FowSpec。
pub fn fow_spec(title: &str, height: f64) -> Value {
    json!({"type": "widget", "confirmMode": "immediate", "initialHeightFactor": height, "title": title, "cancelText": "閉じる"})
}

// ---------------------------------------------------------------------------
// RRL ビルダー(schema/rrl-node.schema.json)
// ---------------------------------------------------------------------------

pub mod rrl {
    use super::*;

    pub fn node(t: &str, key: &str, props: Value, children: Vec<Value>) -> Value {
        let mut n = json!({"t": t, "k": key, "p": props});
        if !children.is_empty() {
            n["c"] = Value::Array(children);
        }
        n
    }
    pub fn text(key: &str, s: impl Into<String>) -> Value {
        node("Text", key, json!({"text": s.into()}), vec![])
    }
    /// 見出し/注釈などスタイル付き Text(ホスト拡張 props: `size` / `bold` / `color`。古いホストは無視)。
    pub fn styled_text(key: &str, s: impl Into<String>, size: f64, bold: bool, color: Option<&str>) -> Value {
        let mut p = json!({"text": s.into(), "size": size, "bold": bold});
        if let Some(c) = color {
            p["color"] = json!(c);
        }
        node("Text", key, p, vec![])
    }
    pub fn icon(key: &str, name: &str) -> Value {
        node("Icon", key, json!({"icon": name}), vec![])
    }
    pub fn image(key: &str, src: &str) -> Value {
        node("Image", key, json!({"src": src}), vec![])
    }
    /// 幅/高さ指定の画像(ホスト拡張 props `width` / `height`)。`src` は URL か `data:` URI。
    pub fn image_sized(key: &str, src: &str, width: f64, height: f64) -> Value {
        node("Image", key, json!({"src": src, "width": width, "height": height}), vec![])
    }
    pub fn button(key: &str, label: &str, on_tap: &str) -> Value {
        let mut n = node("Button", key, json!({"label": label}), vec![]);
        n["on"] = json!({"tap": on_tap});
        n
    }
    pub fn row(key: &str, children: Vec<Value>) -> Value {
        node("Row", key, json!({}), children)
    }
    pub fn column(key: &str, children: Vec<Value>) -> Value {
        node("Column", key, json!({}), children)
    }
    pub fn stack(key: &str, children: Vec<Value>) -> Value {
        node("Stack", key, json!({}), children)
    }
    pub fn scroll(key: &str, horizontal: bool, children: Vec<Value>) -> Value {
        node("Scroll", key, json!({"direction": if horizontal { "horizontal" } else { "vertical" }}), children)
    }
    pub fn spacer(key: &str, size: f64) -> Value {
        node("Spacer", key, json!({"size": size}), vec![])
    }
    /// 矩形(ホスト拡張ノード `Box`: `width` / `height` / `color` / `borderColor` / `borderWidth` / `radius`)。
    /// Stack の子では `left` / `top` で絶対配置(ホスト拡張 props、CONTRACT.md §RRL 拡張)。
    pub fn boxed(key: &str, props: Value, children: Vec<Value>) -> Value {
        node("Box", key, props, children)
    }
    pub fn list_tile(key: &str, title: &str, subtitle: Option<&str>, on_tap: Option<&str>) -> Value {
        let mut p = json!({"title": title});
        if let Some(s) = subtitle {
            p["subtitle"] = json!(s);
        }
        let mut n = node("ListTile", key, p, vec![]);
        if let Some(f) = on_tap {
            n["on"] = json!({"tap": f});
        }
        n
    }
    pub fn slider_tile(key: &str, title: &str, value: f64, min: f64, max: f64, divisions: Option<u32>, unit: Option<&str>, on_change: &str) -> Value {
        let mut p = json!({"title": title, "value": value, "min": min, "max": max});
        if let Some(d) = divisions {
            p["divisions"] = json!(d);
        }
        if let Some(u) = unit {
            p["unit"] = json!(u);
        }
        let mut n = node("SliderListTile", key, p, vec![]);
        n["on"] = json!({"change": on_change});
        n
    }
    pub fn switch_tile(key: &str, title: &str, value: bool, on_change: &str) -> Value {
        let mut n = node("SwitchListTile", key, json!({"title": title, "value": value}), vec![]);
        n["on"] = json!({"change": on_change});
        n
    }
    pub fn rich_text(key: &str, source: &str) -> Value {
        node("RichText", key, json!({"source": source}), vec![])
    }
    /// 子ノードに `left`/`top`(と幅高)を足して Stack 内で絶対配置する。
    pub fn positioned(mut node: Value, left: f64, top: f64) -> Value {
        node["p"]["left"] = json!(left);
        node["p"]["top"] = json!(top);
        node
    }
    /// `on: {<event>: <fn>}` を足す(`drag` / `tap` / `longPress` …)。
    pub fn with_on(mut node: Value, event: &str, fn_name: &str) -> Value {
        if node["on"].is_null() {
            node["on"] = json!({});
        }
        node["on"][event] = json!(fn_name);
        node
    }
    /// 幅・高さ付きの `Stack`(メディアエディターの重ねのルート)。
    pub fn sized_stack(key: &str, width: f64, height: f64, children: Vec<Value>) -> Value {
        node("Stack", key, json!({"width": width, "height": height}), children)
    }
    pub fn with_tag(mut node: Value, tag: &str) -> Value {
        node["tag"] = json!(tag);
        node
    }
    pub fn with_role(mut node: Value, role: &str) -> Value {
        node["role"] = json!(role);
        node
    }

    // ---- patch ops --------------------------------------------------------
    pub fn set_prop(key: &str, prop: &str, value: Value) -> Value {
        json!({"op": "setProp", "target": node_id(key), "prop": prop, "value": value})
    }
    pub fn replace_subtree(key: &str, node: Value) -> Value {
        json!({"op": "replaceSubtree", "target": node_id(key), "node": node})
    }
    pub fn remove(key: &str) -> Value {
        json!({"op": "remove", "target": node_id(key)})
    }
    pub fn insert(parent_key: &str, index: usize, node: Value) -> Value {
        json!({"op": "insert", "target": node_id(parent_key), "index": index, "node": node})
    }
}

// ---------------------------------------------------------------------------
// よく使うホスト関数(CONTRACT.md のスロット契約。無いホストでは E01 が返るだけ)
// ---------------------------------------------------------------------------

pub mod host {
    use super::*;
    pub fn toast(message: &str) -> Value {
        host_call("toast", json!({"message": message}))
    }
    pub fn storage_get(key: &str) -> Value {
        host_value("storage.get", json!({"ns": "safe", "key": key}))["value"].clone()
    }
    pub fn storage_set(key: &str, value: Value) {
        host_call("storage.set", json!({"ns": "safe", "key": key, "value": value}));
    }
    pub fn clipboard_set(text: &str) -> Value {
        host_call("clipboard.set", json!({"text": text}))
    }
    pub fn url_open(url: &str) -> Value {
        host_call("url.open", json!({"url": url}))
    }
    pub fn mute_add(text: &str) -> Value {
        host_call("mute.add", json!({"text": text}))
    }
    pub fn sound_play(kind: &str) -> Value {
        host_call("sound.play", json!({"kind": kind}))
    }
    pub fn astell_share(kind: &str, payload: Value) -> Value {
        host_call("astell.share", json!({"kind": kind, "payload": payload}))
    }
    pub fn composer_media_replace(token: &str, bytes_b64: &str, mime: &str, name: Option<&str>) -> Value {
        let mut a = json!({"token": token, "bytesB64": bytes_b64, "mimeType": mime});
        if let Some(n) = name {
            a["name"] = json!(n);
        }
        host_call("composer.media.replace", a)
    }
    /// `composer.open {token, text, mediaB64?, mimeType?, name?}`(CONTRACT.md §2.8): 選んだアカウントのコンポーザーを
    /// `text` を初期入力して開く。画像を添えるなら `media_b64` + `mime`(+ `name`)。
    pub fn composer_open(token: &str, text: &str, media_b64: Option<&str>, mime: Option<&str>, name: Option<&str>) -> Value {
        let mut a = json!({"token": token, "text": text});
        if let Some(b) = media_b64 {
            a["mediaB64"] = json!(b);
        }
        if let Some(m) = mime {
            a["mimeType"] = json!(m);
        }
        if let Some(n) = name {
            a["name"] = json!(n);
        }
        host_call("composer.open", a)
    }
    /// `composer.media.preview`(v0.5: `width` / `height` = プレビュー画素数、重ねの座標系)。
    pub fn composer_media_preview(token: &str, bytes_b64: &str, mime: &str, size: Option<(u32, u32)>) -> Value {
        let mut a = json!({"token": token, "bytesB64": bytes_b64, "mimeType": mime});
        if let Some((w, h)) = size {
            a["width"] = json!(w);
            a["height"] = json!(h);
        }
        host_call("composer.media.preview", a)
    }
    /// `composer.media.overlay`(v0.5): プレビューの上に重ねる RRL(`root` = `Stack`、`Value::Null` で消す)。
    pub fn composer_media_overlay(token: &str, root: Value) -> Value {
        host_call("composer.media.overlay", json!({"token": token, "root": root}))
    }
    pub fn composer_text_replace(token: &str, start: usize, end: usize, text: &str) -> Value {
        host_call("composer.text.replace", json!({"token": token, "start": start, "end": end, "text": text}))
    }
    pub fn viewer_overlay_set(token: &str, root: Value) -> Value {
        host_call("viewer.overlay.set", json!({"token": token, "root": root}))
    }
    /// `page.render {token, root, title?}`(slot page、v0.18): ページの RRL を丸ごと差し替える。
    pub fn page_render(token: &str, root: Value, title: Option<&str>) -> Value {
        let mut a = json!({"token": token, "root": root});
        if let Some(t) = title {
            a["title"] = json!(t);
        }
        host_call("page.render", a)
    }
    /// `shared.set {token, key, value, ifRev?}`(v0.18、権限 shared.session): サーバー変数を書く(`Value::Null` で消す)。
    /// `me.` で始まるキーはサーバーが `@<accountId>.` に書き換える。反映は `page.event {type: "var"}` で届く。
    pub fn shared_set(token: &str, key: &str, value: Value, if_rev: Option<u64>) -> Value {
        let mut a = json!({"token": token, "key": key, "value": value});
        if let Some(r) = if_rev {
            a["ifRev"] = json!(r);
        }
        host_call("shared.set", a)
    }
    /// `shared.op {token, key, op, value}`(v0.18): `incr` / `append` / `remove` / `merge` をサーバー側で原子的に。
    pub fn shared_op(token: &str, key: &str, op: &str, value: Value) -> Value {
        host_call("shared.op", json!({"token": token, "key": key, "op": op, "value": value}))
    }
    /// `shared.call {token, fn, args}`(v0.18): 参加者全員(自分も)へ `page.event {type: "call", fn, args, from}` を配る。
    pub fn shared_call(token: &str, fn_name: &str, args: Value) -> Value {
        host_call("shared.call", json!({"token": token, "fn": fn_name, "args": args}))
    }
}

// ---------------------------------------------------------------------------
// export を生やすマクロ
// ---------------------------------------------------------------------------

/// wasm の export(`snapshot` / `patch` / `call` / `alloc` / `dealloc`)を生やす。
///
/// ```ignore
/// rasq_plugin_kit::rasq_plugin! {
///     scope: "sample.hello:main",
///     root: root,                 // fn() -> Value(本体ツリー。無ければ `|| serde_json::json!({"t":"Column","k":"root"})`)
///     call: handle,               // fn(&str, Value) -> Result<Value, Value>
/// }
/// ```
#[macro_export]
macro_rules! rasq_plugin {
    (scope: $scope:expr, root: $root:expr, call: $call:expr $(,)?) => {
        #[no_mangle]
        pub extern "C" fn alloc(len: u32) -> u32 {
            $crate::alloc_impl(len)
        }
        #[no_mangle]
        pub extern "C" fn dealloc(ptr: u32, len: u32) {
            $crate::dealloc_impl(ptr, len)
        }
        #[no_mangle]
        pub extern "C" fn snapshot() -> u64 {
            $crate::set_scope($scope);
            let root: $crate::serde_json::Value = ($root)();
            $crate::pack($crate::snapshot_message(root).to_string().into_bytes())
        }
        #[no_mangle]
        pub extern "C" fn patch() -> u64 {
            $crate::set_scope($scope);
            $crate::pack($crate::take_patch_message().to_string().into_bytes())
        }
        #[no_mangle]
        pub extern "C" fn call(fn_ptr: u32, fn_len: u32, args_ptr: u32, args_len: u32, call_id_ptr: u32, call_id_len: u32) -> u64 {
            $crate::set_scope($scope);
            let fn_name = $crate::read_str(fn_ptr, fn_len);
            let args: $crate::serde_json::Value = $crate::serde_json::from_str(&$crate::read_str(args_ptr, args_len)).unwrap_or($crate::serde_json::Value::Null);
            let call_id = $crate::read_str(call_id_ptr, call_id_len);
            let outcome: Result<$crate::serde_json::Value, $crate::serde_json::Value> = ($call)(fn_name.as_str(), args);
            $crate::pack($crate::result_message(&call_id, outcome).to_string().into_bytes())
        }

        /// native テスト用: export と同じ経路で `call` を呼ぶ。
        pub fn call_native(fn_name: &str, args: $crate::serde_json::Value) -> Result<$crate::serde_json::Value, $crate::serde_json::Value> {
            $crate::set_scope($scope);
            ($call)(fn_name, args)
        }
    };
}

/// `plugin.manifest` の答え(CONTRACT.md §1)。
pub fn manifest(id: &str, name: &str, version: &str, kind: &str, description: &str, slots: &[&str], permissions: &[&str]) -> Value {
    json!({
        "schema": "rasq.plugin-manifest/1",
        "id": id, "name": name, "version": version, "kind": kind, "description": description,
        "slots": slots, "permissions": permissions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_and_rev() {
        set_scope("t:main");
        let r0 = rev();
        let v = set_patch(vec![rrl::set_prop("a", "text", json!("x"))]);
        assert_eq!(v, json!({"patch": true}));
        let m = take_patch_message();
        assert_eq!(m["baseRev"], json!(r0));
        assert_eq!(m["rev"], json!(r0 + 1));
        assert_eq!(m["ops"][0]["target"], json!("t:main:a"));
        assert_eq!(take_patch_message()["ops"], json!([]));
    }

    #[test]
    fn surface_patch_sequence() {
        set_scope("t:main");
        set_host_hook(|f, _| if f == "fow.open" { json!({"opened": true}) } else { json!({}) });
        static S: Surface = Surface::new("panel");
        assert!(S.open(fow_spec("p", 0.3), rrl::column("root", vec![]), Some("closed"), json!({})));
        S.patch(vec![]);
        let m = take_patch_message();
        assert_eq!(m["surface"], json!("panel"));
        assert_eq!((m["baseRev"].as_u64().unwrap(), m["rev"].as_u64().unwrap()), (1, 2));
        S.patch(vec![]);
        assert_eq!(take_patch_message()["rev"], json!(3));
        let log = take_host_log();
        assert_eq!(log[0].0, "fow.open");
        assert_eq!(log[0].1["onClosed"], json!("closed"));
    }

    #[test]
    fn rrl_builders() {
        let b = rrl::button("b", "押す", "pressed");
        assert_eq!(b["on"]["tap"], json!("pressed"));
        let p = rrl::positioned(rrl::boxed("x", json!({"width": 10}), vec![]), 3.0, 4.0);
        assert_eq!(p["p"]["left"], json!(3.0));
        assert_eq!(rrl::with_tag(rrl::text("t", "a"), "main_text")["tag"], json!("main_text"));
    }
}
