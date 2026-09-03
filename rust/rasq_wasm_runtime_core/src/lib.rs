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
    /// When true, `net.fetch` is served natively (blocking ureq on the
    /// calling thread) instead of going through the embedder callback —
    /// mobile hosts have no synchronous HTTP bridge (D-023).
    native_fetch: bool,
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
        let mut store = Store::new(&engine, HostState { callback, user_data, native_fetch: false });
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

    #[cfg(test)]
    fn set_native_fetch(&mut self, enabled: bool) {
        self.store.data_mut().native_fetch = enabled;
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

    // Native net.fetch short-circuit (D-023): parse the envelope; when the fn
    // is net.fetch and native fetch is enabled, answer inside the runtime.
    if caller.data().native_fetch {
        if let Ok(req) = serde_json::from_slice::<serde_json::Value>(&request) {
            if req["fn"] == "net.fetch" {
                let response = native_fetch_response(&req);
                return write_response(&mut caller, memory, response.to_string().as_bytes());
            }
        }
    }

    // Native gpu.compute short-circuit (D-081): WGSL compute 専用の狭い
    // GPU 口。常にランタイム内で処理する(Dart 側に同期 GPU ブリッジは
    // 無い)。gpu_ml 権限の審査ゲートはマニフェスト/ストア側 (M4)。
    if let Ok(req) = serde_json::from_slice::<serde_json::Value>(&request) {
        if req["fn"] == "gpu.compute" {
            let response = gpu_compute_response(&req);
            return write_response(&mut caller, memory, response.to_string().as_bytes());
        }
    }

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

    write_response(&mut caller, memory, response.as_bytes())
}

/// Allocates inside the plugin (via its `alloc`), writes [bytes] and returns
/// the packed `(ptr << 32) | len`.
fn write_response(caller: &mut Caller<'_, HostState>, memory: Memory, bytes: &[u8]) -> Result<i64, wasmi::Error> {
    let alloc = caller
        .get_export("alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmi::Error::new("plugin has no exported alloc"))?
        .typed::<u32, u32>(&*caller)
        .map_err(|e| wasmi::Error::new(format!("alloc signature mismatch: {e}")))?;
    let out_ptr = alloc.call(&mut *caller, bytes.len() as u32)?;
    memory
        .write(&mut *caller, out_ptr as usize, bytes)
        .map_err(|e| wasmi::Error::new(format!("host_fn: write response failed: {e}")))?;
    Ok((((out_ptr as u64) << 32) | bytes.len() as u64) as i64)
}

/// Serves one `net.fetch` request with blocking ureq (host-fn schema:
/// args {method, url, headers?, body?} → value {status, headers, bodyText}).
// ---- gpu.compute (D-081) --------------------------------------------------
// WGSL compute 専用の狭い GPU 口: 入力 1 バッファ(binding 0, read-only
// storage)→ 出力 1 バッファ(binding 1, read_write storage)の 1 パス。
// カスタム TL の重いソートやメディア編集フィルターの下地。アダプター無し・
// 検証エラーは E01 で返し、プラグインはクラウドカルクレーターへ逃がす。

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

static GPU: std::sync::OnceLock<Option<GpuCtx>> = std::sync::OnceLock::new();

fn gpu_ctx() -> Option<&'static GpuCtx> {
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()?;
        Some(GpuCtx { device, queue })
    })
    .as_ref()
}

fn gpu_compute_response(req: &serde_json::Value) -> serde_json::Value {
    use base64::Engine as _;
    use wgpu::util::DeviceExt as _;
    let call_id = req["callId"].clone();
    let err = |code: &str, message: String| {
        serde_json::json!({"callId": call_id, "ok": false, "error": {"code": code, "message": message}})
    };
    let args = &req["args"];
    let Some(ctx) = gpu_ctx() else {
        return err("E01", "no gpu adapter — fall back to the cloud calculator (ml.infer)".to_string());
    };
    let wgsl = args["wgsl"].as_str().unwrap_or("");
    if wgsl.is_empty() {
        return err("D01", "gpu.compute: wgsl required".to_string());
    }
    let entry = args["entry"].as_str().unwrap_or("main");
    let input = match base64::engine::general_purpose::STANDARD.decode(args["input"].as_str().unwrap_or("")) {
        Ok(b) => b,
        Err(e) => return err("D01", format!("gpu.compute: bad input base64: {e}")),
    };
    let output_size = args["outputSize"].as_u64().unwrap_or(0);
    if output_size == 0 || output_size > 64 * 1024 * 1024 {
        return err("D01", format!("gpu.compute: outputSize out of range: {output_size}"));
    }
    let wg: Vec<u32> = args["workgroups"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_u64().unwrap_or(1).max(1) as u32).collect())
        .unwrap_or_else(|| vec![1, 1, 1]);
    let (wx, wy, wz) = (
        *wg.first().unwrap_or(&1),
        *wg.get(1).unwrap_or(&1),
        *wg.get(2).unwrap_or(&1),
    );

    let device = &ctx.device;
    // 検証エラー(WGSL の文法・バインド不一致)はエラースコープで拾う。
    device.push_error_scope(wgpu::ErrorFilter::Validation);

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rasq gpu.compute"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let in_bytes: &[u8] = if input.is_empty() { &[0, 0, 0, 0] } else { &input };
    let in_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: in_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: output_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: in_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: out_buf.as_entire_binding() },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: entry,
        compilation_options: Default::default(),
        cache: None,
    });
    if let Some(e) = pollster::block_on(device.pop_error_scope()) {
        return err("E01", format!("gpu.compute: validation failed: {e}"));
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(wx, wy, wz);
    }
    encoder.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, output_size);
    ctx.queue.submit([encoder.finish()]);

    let slice = read_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    match rx.recv() {
        Ok(Ok(())) => {}
        other => return err("E01", format!("gpu.compute: readback failed: {other:?}")),
    }
    let output = base64::engine::general_purpose::STANDARD.encode(slice.get_mapped_range().as_ref());
    serde_json::json!({"callId": call_id, "ok": true, "value": {"output": output, "placement": "device"}})
}

fn native_fetch_response(req: &serde_json::Value) -> serde_json::Value {
    let call_id = req["callId"].clone();
    let args = &req["args"];
    let method = args["method"].as_str().unwrap_or("GET").to_uppercase();
    let url = args["url"].as_str().unwrap_or("");
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "B03", "message": format!("bad url: {url}")}});
    }
    let mut request = ureq::request(&method, url).timeout(std::time::Duration::from_secs(15));
    if let Some(headers) = args["headers"].as_object() {
        for (k, v) in headers {
            if let Some(v) = v.as_str() {
                request = request.set(k, v);
            }
        }
    }
    // ボディは UTF-8 文字列(`body`)か、バイナリの base64(`body.b64`)。
    // host-fn schema の `body: "string? (utf-8) | {b64: string}?"` に対応する。
    // multipart/form-data(メディアアップロード)はプラグインが本体を組み立てて
    // b64 で渡してくる — ランタイムは中身を解釈しない。
    let result = match (&args["body"], args["body"]["b64"].as_str()) {
        (_, Some(b64)) => {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(bytes) => request.send_bytes(&bytes),
                Err(e) => {
                    return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "D01", "message": format!("bad body base64: {e}")}});
                }
            }
        }
        (serde_json::Value::String(body), None) => request.send_string(body),
        _ => request.call(),
    };
    let http_response = match result {
        Ok(r) => r,
        // ureq treats HTTP >= 400 as Err(Status) — still a response for us.
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => {
            return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "B01", "message": format!("fetch failed: {e}")}});
        }
    };
    let status = http_response.status();
    let mut headers = serde_json::Map::new();
    for name in http_response.headers_names() {
        if let Some(v) = http_response.header(&name) {
            headers.insert(name.clone(), serde_json::Value::String(v.to_string()));
        }
    }
    // 応答は UTF-8 なら `bodyText`、そうでなければ `bodyB64`(画像など)。
    // 読めるものは今までどおり文字列で返す — 既存プラグインは無改造で動く。
    let mut body = Vec::new();
    if let Err(e) = std::io::Read::read_to_end(&mut http_response.into_reader(), &mut body) {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "B03", "message": format!("read body failed: {e}")}});
    }
    match String::from_utf8(body) {
        Ok(text) => serde_json::json!({"callId": call_id, "ok": true, "value": {"status": status, "headers": headers, "bodyText": text}}),
        Err(e) => {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(e.as_bytes());
            serde_json::json!({"callId": call_id, "ok": true, "value": {"status": status, "headers": headers, "bodyB64": b64}})
        }
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

/// Enables/disables the runtime-native `net.fetch` backend (D-023). When
/// enabled, `net.fetch` never reaches the embedder callback: it is served by
/// a blocking HTTP client inside the runtime (works on mobile, where the
/// embedder has no synchronous HTTP). The calling thread blocks for the
/// duration of the request — run plugin calls off the UI thread.
#[no_mangle]
pub extern "C" fn rasq_wasm_set_native_fetch(instance: *mut RasqWasmInstance, enabled: bool) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *instance };
    instance.store.data_mut().native_fetch = enabled;
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
            // Canned Misskey local-timeline (2 notes) for the timeline test.
            "net.fetch" => serde_json::json!({
                "status": 200,
                "headers": {},
                "bodyText": serde_json::json!([
                    {
                        "id": "n1",
                        "text": "打ち上げ成功！ $[tada 🎉]",
                        "cw": null,
                        "createdAt": "2026-08-23T12:00:00.000Z",
                        "user": {"username": "douglas", "name": "ダグラス", "host": null, "avatarUrl": "https://example.invalid/a.png"},
                        "files": [{"id": "f1", "type": "image/png", "url": "https://example.invalid/1.png", "thumbnailUrl": null, "isSensitive": false}],
                        "repliesCount": 2, "renoteCount": 5,
                        "reactions": {"👍": 3, "🎉": 4}
                    },
                    {
                        "id": "n2",
                        "text": "CWつき",
                        "cw": "ネタバレ",
                        "createdAt": "2026-08-23T11:00:00.000Z",
                        "user": {"username": "tivard", "name": null, "host": "other.example", "avatarUrl": null},
                        "files": [],
                        "repliesCount": 0, "renoteCount": 0,
                        "reactions": {}
                    }
                ]).to_string()
            }),
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
    fn timeline_rebuilds_the_tree_from_net_fetch() {
        let bytes = std::fs::read(WASM_PATH).expect("build misskey-wasm-sample first");
        let mut instance =
            RasqWasmInstance::load(&bytes, Some(test_host_fn), 0).expect("instantiate");

        // Initial snapshot: sample tile at rev 1.
        let before = instance.snapshot().expect("snapshot");
        assert!(before.contains("post-sample-1"), "{before}");

        let result = instance
            .call("timeline", r#"{"host":"misskey.example","limit":10}"#, "call-t")
            .expect("call");
        assert!(result.contains(r#""ok":true"#), "{result}");
        assert!(result.contains(r#""treeUpdated":true"#), "{result}");
        assert!(result.contains(r#""count":2"#), "{result}");

        let after = instance.snapshot().expect("snapshot 2");
        assert!(after.contains(r#""rev":2"#), "{after}");
        assert!(after.contains("ダグラス"), "{after}");
        assert!(after.contains("post-n1"), "{after}");
        assert!(after.contains("【CW: ネタバレ】"), "{after}");
        assert!(after.contains(r#""instance":"misskey.example""#), "{after}"); // local user → fetch host
        assert!(after.contains(r#""instance":"other.example""#), "{after}"); // remote user → note host
        assert!(after.contains(r#""count":5"#), "{after}"); // renote count in ActionBar states
    }

    #[test]
    fn native_fetch_serves_net_fetch_inside_the_runtime() {
        use std::io::{Read, Write};
        // Mini HTTP server: one request, fixed 2-note timeline response.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = serde_json::json!([
            {"id": "n9", "text": "ネイティブfetchの投稿", "cw": null, "createdAt": "2026-08-25T12:00:00.000Z",
             "user": {"username": "native", "name": "ネイティブ", "host": null, "avatarUrl": null},
             "files": [], "repliesCount": 0, "renoteCount": 1, "reactions": {}}
        ])
        .to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let bytes = std::fs::read(WASM_PATH).expect("build misskey-wasm-sample first");
        // No callback at all: net.fetch must be answered natively.
        let mut instance = RasqWasmInstance::load_plain(&bytes).expect("instantiate");
        instance.set_native_fetch(true);
        let result = instance
            .call(
                "timeline",
                &format!(r#"{{"host":"native.example","baseUrl":"http://127.0.0.1:{port}","limit":5}}"#),
                "call-n",
            )
            .expect("call");
        server.join().unwrap();
        assert!(result.contains(r#""ok":true"#), "{result}");
        assert!(result.contains(r#""count":1"#), "{result}");
        let snapshot = instance.snapshot().expect("snapshot");
        assert!(snapshot.contains("ネイティブfetchの投稿"), "{snapshot}");
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
