//! Native WASM runtime for RASQ hosts (RASQ M3 / D-007).
//!
//! Wraps `wasmi` (a pure-Rust WASM *interpreter*, no JIT) so the same engine
//! can run on iOS, where JIT'd wasm runtimes are not viable. Exposes a small
//! C ABI so it can be linked into a Flutter FFI plugin and driven from Dart.
//! The wire format is `rasq.boundary/1` JSON (see `PROTOCOL.md`); this crate
//! only shuttles bytes across the host<->wasm boundary, it does not
//! interpret the messages.

use std::ffi::{c_char, c_void, CStr, CString};
use std::slice;
use wasmi::{Caller, Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

/// Host-function callback provided by the embedder (Dart via FFI).
///
/// `request` is a NUL-terminated `rasq.hostfn/1` JSON string. The returned
/// string must be allocated with [`rasq_wasm_string_dup`] (so this crate can
/// free it); returning null means "no handler" and yields an `E00` error to
/// the plugin.
pub type RasqHostFnCallback = extern "C" fn(request: *const c_char, user_data: *mut c_void) -> *mut c_char;

struct HostState {
    callback: Option<RasqHostFnCallback>,
    user_data: usize,
}

pub struct RasqWasmInstance {
    store: Store<HostState>,
    memory: Memory,
    instance: Instance,
}

#[derive(Debug)]
enum RasqWasmError {
    Wasmi(wasmi::Error),
    MissingExport(&'static str),
    Utf8,
}

impl From<wasmi::Error> for RasqWasmError {
    fn from(e: wasmi::Error) -> Self {
        RasqWasmError::Wasmi(e)
    }
}

impl From<wasmi::errors::MemoryError> for RasqWasmError {
    fn from(e: wasmi::errors::MemoryError) -> Self {
        RasqWasmError::Wasmi(e.into())
    }
}

impl RasqWasmInstance {
    fn load(wasm_bytes: &[u8], callback: Option<RasqHostFnCallback>, user_data: usize) -> Result<Self, RasqWasmError> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)?;
        let mut store = Store::new(&engine, HostState { callback, user_data });
        let mut linker = Linker::new(&engine);
        // PROTOCOL.md §4.1: (import "rasq" "host_fn" (func (param i32 i32) (result i64))).
        // Always provided; without a registered callback it answers E00 so a
        // plugin never traps just because the host has no handler.
        linker
            .func_wrap("rasq", "host_fn", host_fn_impl)
            .map_err(wasmi::Error::from)?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or(RasqWasmError::MissingExport("memory"))?;
        Ok(Self {
            store,
            memory,
            instance,
        })
    }

    #[cfg(test)]
    fn load_plain(wasm_bytes: &[u8]) -> Result<Self, RasqWasmError> {
        Self::load(wasm_bytes, None, 0)
    }

    fn typed_func<Params, Results>(&mut self, name: &'static str) -> Result<TypedFunc<Params, Results>, RasqWasmError>
    where
        Params: wasmi::WasmParams,
        Results: wasmi::WasmResults,
    {
        self.instance
            .get_typed_func::<Params, Results>(&self.store, name)
            .map_err(|_| RasqWasmError::MissingExport(name))
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(u32, u32), RasqWasmError> {
        let alloc: TypedFunc<u32, u32> = self.typed_func("alloc")?;
        let len = bytes.len() as u32;
        let ptr = alloc.call(&mut self.store, len)?;
        self.memory.write(&mut self.store, ptr as usize, bytes)?;
        Ok((ptr, len))
    }

    fn read_packed(&mut self, packed: i64) -> Result<Vec<u8>, RasqWasmError> {
        let packed = packed as u64;
        let ptr = (packed >> 32) as u32;
        let len = (packed & 0xFFFF_FFFF) as u32;
        let mut buf = vec![0u8; len as usize];
        self.memory.read(&mut self.store, ptr as usize, &mut buf)?;
        let dealloc: TypedFunc<(u32, u32), ()> = self.typed_func("dealloc")?;
        dealloc.call(&mut self.store, (ptr, len))?;
        Ok(buf)
    }

    fn snapshot(&mut self) -> Result<String, RasqWasmError> {
        let f: TypedFunc<(), i64> = self.typed_func("snapshot")?;
        let packed = f.call(&mut self.store, ())?;
        let bytes = self.read_packed(packed)?;
        String::from_utf8(bytes).map_err(|_| RasqWasmError::Utf8)
    }

    fn patch(&mut self) -> Result<String, RasqWasmError> {
        let f: TypedFunc<(), i64> = self.typed_func("patch")?;
        let packed = f.call(&mut self.store, ())?;
        let bytes = self.read_packed(packed)?;
        String::from_utf8(bytes).map_err(|_| RasqWasmError::Utf8)
    }

    fn call(&mut self, fn_name: &str, args_json: &str, call_id: &str) -> Result<String, RasqWasmError> {
        let (fn_ptr, fn_len) = self.write_bytes(fn_name.as_bytes())?;
        let (args_ptr, args_len) = self.write_bytes(args_json.as_bytes())?;
        let (call_id_ptr, call_id_len) = self.write_bytes(call_id.as_bytes())?;

        let f: TypedFunc<(u32, u32, u32, u32, u32, u32), i64> = self.typed_func("call")?;
        let packed = f.call(
            &mut self.store,
            (fn_ptr, fn_len, args_ptr, args_len, call_id_ptr, call_id_len),
        )?;

        let dealloc: TypedFunc<(u32, u32), ()> = self.typed_func("dealloc")?;
        dealloc.call(&mut self.store, (fn_ptr, fn_len))?;
        dealloc.call(&mut self.store, (args_ptr, args_len))?;
        dealloc.call(&mut self.store, (call_id_ptr, call_id_len))?;

        let bytes = self.read_packed(packed)?;
        String::from_utf8(bytes).map_err(|_| RasqWasmError::Utf8)
    }
}

/// Implements the `rasq.host_fn` import (PROTOCOL.md §4.1, synchronous v1):
/// read the request JSON from wasm memory → call the embedder's callback →
/// allocate the response inside wasm memory (via the plugin's `alloc`) and
/// return `(ptr << 32) | len`.
fn host_fn_impl(mut caller: Caller<'_, HostState>, ptr: u32, len: u32) -> Result<i64, wasmi::Error> {
    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => return Err(wasmi::Error::new("plugin has no exported memory")),
    };
    let mut request = vec![0u8; len as usize];
    memory
        .read(&caller, ptr as usize, &mut request)
        .map_err(|e| wasmi::Error::new(format!("host_fn: bad request buffer: {e}")))?;

    let response: String = match caller.data().callback {
        None => r#"{"ok":false,"error":{"code":"E00","message":"no host_fn handler registered"}}"#.to_string(),
        Some(callback) => {
            let request_c = CString::new(request)
                .unwrap_or_else(|_| CString::new("{}").unwrap());
            let user_data = caller.data().user_data as *mut c_void;
            let raw = callback(request_c.as_ptr(), user_data);
            if raw.is_null() {
                r#"{"ok":false,"error":{"code":"E00","message":"host_fn handler returned null"}}"#.to_string()
            } else {
                // Ownership: the callback allocated this via rasq_wasm_string_dup.
                let s = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
                unsafe { drop(CString::from_raw(raw)) };
                s
            }
        }
    };

    let bytes = response.as_bytes();
    let alloc = caller
        .get_export("alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmi::Error::new("plugin has no exported alloc"))?
        .typed::<u32, u32>(&caller)
        .map_err(|e| wasmi::Error::new(format!("alloc signature mismatch: {e}")))?;
    let out_ptr = alloc.call(&mut caller, bytes.len() as u32)?;
    memory
        .write(&mut caller, out_ptr as usize, bytes)
        .map_err(|e| wasmi::Error::new(format!("host_fn: write response failed: {e}")))?;
    Ok((((out_ptr as u64) << 32) | bytes.len() as u64) as i64)
}

fn result_to_c_string<T: Into<String>>(result: Result<T, RasqWasmError>) -> *mut c_char {
    let s = match result {
        Ok(v) => v.into(),
        Err(e) => serde_error_json(&e),
    };
    CString::new(s)
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":{\"code\":\"D01\",\"message\":\"nul in output\"}}").unwrap())
        .into_raw()
}

fn serde_error_json(e: &RasqWasmError) -> String {
    let message = match e {
        RasqWasmError::Wasmi(err) => err.to_string(),
        RasqWasmError::MissingExport(name) => format!("missing export: {name}"),
        RasqWasmError::Utf8 => "wasm returned non-utf8 bytes".to_string(),
    };
    format!("{{\"ok\":false,\"error\":{{\"code\":\"C02\",\"message\":{message:?}}}}}")
}

/// Instantiate a wasm module from raw bytes (no host-function handler:
/// `rasq.host_fn` answers E00). Returns null on failure.
#[no_mangle]
pub extern "C" fn rasq_wasm_instantiate(bytes_ptr: *const u8, bytes_len: usize) -> *mut RasqWasmInstance {
    let bytes = unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) };
    match RasqWasmInstance::load(bytes, None, 0) {
        Ok(instance) => Box::into_raw(Box::new(instance)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Instantiate with a host-function handler serving the `rasq.host_fn`
/// import (PROTOCOL.md §4.1). `callback` receives the request JSON and must
/// return a response string allocated with `rasq_wasm_string_dup` (or null
/// for "unhandled"). `user_data` is passed through untouched.
#[no_mangle]
pub extern "C" fn rasq_wasm_instantiate_with_host(
    bytes_ptr: *const u8,
    bytes_len: usize,
    callback: RasqHostFnCallback,
    user_data: *mut c_void,
) -> *mut RasqWasmInstance {
    let bytes = unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) };
    match RasqWasmInstance::load(bytes, Some(callback), user_data as usize) {
        Ok(instance) => Box::into_raw(Box::new(instance)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Copies a NUL-terminated string into this crate's allocator. Host-function
/// callbacks MUST return strings allocated here so the runtime can free them.
#[no_mangle]
pub extern "C" fn rasq_wasm_string_dup(s: *const c_char) -> *mut c_char {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
    CString::new(bytes.to_vec())
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Frees an instance created by `rasq_wasm_instantiate`.
#[no_mangle]
pub extern "C" fn rasq_wasm_destroy(instance: *mut RasqWasmInstance) {
    if !instance.is_null() {
        unsafe {
            drop(Box::from_raw(instance));
        }
    }
}

/// Calls the plugin's `snapshot` export. Result is a `rasq.boundary/1` JSON
/// string owned by the caller; free it with `rasq_wasm_free_string`.
#[no_mangle]
pub extern "C" fn rasq_wasm_snapshot(instance: *mut RasqWasmInstance) -> *mut c_char {
    let instance = unsafe { &mut *instance };
    result_to_c_string(instance.snapshot())
}

/// Calls the plugin's `patch` export. See `rasq_wasm_snapshot`.
#[no_mangle]
pub extern "C" fn rasq_wasm_patch(instance: *mut RasqWasmInstance) -> *mut c_char {
    let instance = unsafe { &mut *instance };
    result_to_c_string(instance.patch())
}

/// Delivers a host `call { fn, args, callId }` (PROTOCOL.md §3.2) to the
/// plugin's `call` export and returns its `result` message.
#[no_mangle]
pub extern "C" fn rasq_wasm_call(
    instance: *mut RasqWasmInstance,
    fn_name: *const c_char,
    args_json: *const c_char,
    call_id: *const c_char,
) -> *mut c_char {
    let instance = unsafe { &mut *instance };
    let fn_name = unsafe { CStr::from_ptr(fn_name) }.to_string_lossy();
    let args_json = unsafe { CStr::from_ptr(args_json) }.to_string_lossy();
    let call_id = unsafe { CStr::from_ptr(call_id) }.to_string_lossy();
    result_to_c_string(instance.call(&fn_name, &args_json, &call_id))
}

/// Frees a string returned by `rasq_wasm_snapshot`/`rasq_wasm_patch`/`rasq_wasm_call`.
#[no_mangle]
pub extern "C" fn rasq_wasm_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WASM_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-unknown-unknown/release/misskey_wasm_sample.wasm"
    );

    fn load() -> RasqWasmInstance {
        let bytes = std::fs::read(WASM_PATH)
            .expect("build misskey-wasm-sample first: cargo build --release --target wasm32-unknown-unknown -p misskey-wasm-sample");
        RasqWasmInstance::load_plain(&bytes).expect("instantiate")
    }

    /// Test host_fn handler: storage.set/get over a process-global map, plus
    /// an echo for everything else. Mirrors what the Dart side will provide.
    extern "C" fn test_host_fn(request: *const c_char, _user_data: *mut c_void) -> *mut c_char {
        use std::collections::HashMap;
        use std::sync::Mutex;
        static STORE: Mutex<Option<HashMap<String, serde_json::Value>>> = Mutex::new(None);

        let req: serde_json::Value =
            serde_json::from_str(&unsafe { CStr::from_ptr(request) }.to_string_lossy()).unwrap();
        let call_id = req["callId"].clone();
        let args = &req["args"];
        let mut store = STORE.lock().unwrap();
        let store = store.get_or_insert_with(HashMap::new);
        let value = match req["fn"].as_str().unwrap_or("") {
            "storage.set" => {
                store.insert(args["key"].as_str().unwrap().to_string(), args["value"].clone());
                serde_json::json!({})
            }
            "storage.get" => serde_json::json!({"value": store.get(args["key"].as_str().unwrap())}),
            other => serde_json::json!({"echo": other}),
        };
        let response = serde_json::json!({"callId": call_id, "ok": true, "value": value});
        let c = CString::new(response.to_string()).unwrap();
        rasq_wasm_string_dup(c.as_ptr())
    }

    #[test]
    fn host_fn_round_trip_via_storage() {
        let bytes = std::fs::read(WASM_PATH).expect("build misskey-wasm-sample first");
        let mut instance =
            RasqWasmInstance::load(&bytes, Some(test_host_fn), 0).expect("instantiate");
        let json = instance
            .call("storageRoundTrip", r#"{"key":"greeting","value":"やあ"}"#, "call-3")
            .expect("call");
        assert!(json.contains(r#""ok":true"#), "{json}");
        assert!(json.contains("やあ"), "{json}");
    }

    #[test]
    fn host_fn_without_handler_reports_e00() {
        let mut instance = load();
        let json = instance
            .call("storageRoundTrip", r#"{"key":"k","value":1}"#, "call-4")
            .expect("call");
        assert!(json.contains("E00"), "{json}");
    }

    #[test]
    fn snapshot_round_trip() {
        let mut instance = load();
        let json = instance.snapshot().expect("snapshot");
        assert!(json.contains("\"type\":\"snapshot\""));
        assert!(json.contains("rasq.boundary/1"));
    }

    #[test]
    fn patch_round_trip() {
        let mut instance = load();
        let json = instance.patch().expect("patch");
        assert!(json.contains("\"type\":\"patch\""));
        assert!(json.contains("\"baseRev\":1"));
    }

    #[test]
    fn call_open_thread_echoes_args() {
        let mut instance = load();
        let json = instance
            .call("openThread", "{\"postId\":\"abc\"}", "call-1")
            .expect("call");
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"callId\":\"call-1\""));
        assert!(json.contains("\"postId\":\"abc\""));
    }

    #[test]
    fn call_unknown_fn_reports_error() {
        let mut instance = load();
        let json = instance.call("noSuchFn", "{}", "call-2").expect("call");
        assert!(json.contains("\"ok\":false"));
    }
}
