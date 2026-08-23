//! Native WASM runtime for RASQ hosts (RASQ M3 / D-007).
//!
//! Wraps `wasmi` (a pure-Rust WASM *interpreter*, no JIT) so the same engine
//! can run on iOS, where JIT'd wasm runtimes are not viable. Exposes a small
//! C ABI so it can be linked into a Flutter FFI plugin and driven from Dart.
//! The wire format is `rasq.boundary/1` JSON (see `PROTOCOL.md`); this crate
//! only shuttles bytes across the host<->wasm boundary, it does not
//! interpret the messages.

use std::ffi::{c_char, CStr, CString};
use std::slice;
use wasmi::{Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

pub struct RasqWasmInstance {
    store: Store<()>,
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
    fn load(wasm_bytes: &[u8]) -> Result<Self, RasqWasmError> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)?;
        let mut store = Store::new(&engine, ());
        let linker = Linker::new(&engine);
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

/// Instantiate a wasm module from raw bytes. Returns null on failure.
#[no_mangle]
pub extern "C" fn rasq_wasm_instantiate(bytes_ptr: *const u8, bytes_len: usize) -> *mut RasqWasmInstance {
    let bytes = unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) };
    match RasqWasmInstance::load(bytes) {
        Ok(instance) => Box::into_raw(Box::new(instance)),
        Err(_) => std::ptr::null_mut(),
    }
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
        RasqWasmInstance::load(&bytes).expect("instantiate")
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
