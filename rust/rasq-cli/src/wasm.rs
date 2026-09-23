//! wasm の検査(docs/cli.md §5 の 2〜3): export / import を見て、wasmi で `call("plugin.manifest")` を呼ぶ。
//! ホスト関数はすべて E01 で断る(ホスト無し)。`rasq_wasm_runtime_core` と同じ ABI(PROTOCOL.md §4.1)。

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use wasmi::{Caller, Engine, Linker, Memory, Module, Store, TypedFunc};

pub const REQUIRED_EXPORTS: [&str; 6] = ["memory", "snapshot", "patch", "call", "alloc", "dealloc"];

pub struct Inspection {
    pub exports: Vec<String>,
    pub imports: Vec<(String, String)>,
}

pub fn inspect(bytes: &[u8]) -> Result<Inspection> {
    let engine = Engine::default();
    let module = Module::new(&engine, bytes).context("wasm を読めません")?;
    Ok(Inspection {
        exports: module.exports().map(|e| e.name().to_string()).collect(),
        imports: module.imports().map(|i| (i.module().to_string(), i.name().to_string())).collect(),
    })
}

impl Inspection {
    /// 足りない export と、`rasq.host_fn` 以外の import。
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        for e in REQUIRED_EXPORTS {
            if !self.exports.iter().any(|x| x == e) {
                v.push(format!("export {e} がありません"));
            }
        }
        for (m, n) in &self.imports {
            if !(m == "rasq" && n == "host_fn") {
                v.push(format!("許されない import: {m}.{n}(rasq.host_fn だけ)"));
            }
        }
        v
    }
}

/// ホスト関数の代役の状態: 決め打ちの応答(fn → value)と、呼ばれた記録。
#[derive(Default)]
pub struct HostState {
    pub replies: std::collections::HashMap<String, Value>,
    pub log: Vec<(String, Value)>,
}

pub struct Plugin {
    store: Store<HostState>,
    memory: Memory,
    instance: wasmi::Instance,
}

impl Plugin {
    fn load(bytes: &[u8]) -> Result<Self> {
        Self::load_with(bytes, HostState::default())
    }

    /// `replies` にある fn は `{ok: true, value}` で答え、それ以外は E01(ホスト無し)。
    pub fn load_with(bytes: &[u8], host: HostState) -> Result<Self> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes)?;
        let mut store = Store::new(&engine, host);
        let mut linker = <Linker<HostState>>::new(&engine);
        linker.func_wrap("rasq", "host_fn", host_fn_stub)?;
        let instance = linker.instantiate_and_start(&mut store, &module).context("インスタンス化に失敗")?;
        let memory = instance.get_memory(&store, "memory").ok_or_else(|| anyhow!("export memory がありません"))?;
        Ok(Self { store, memory, instance })
    }

    fn typed<P: wasmi::WasmParams, R: wasmi::WasmResults>(&self, name: &str) -> Result<TypedFunc<P, R>> {
        self.instance.get_typed_func::<P, R>(&self.store, name).map_err(|e| anyhow!("export {name}: {e}"))
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(u32, u32)> {
        let alloc: TypedFunc<u32, u32> = self.typed("alloc")?;
        let ptr = alloc.call(&mut self.store, bytes.len() as u32)?;
        self.memory.write(&mut self.store, ptr as usize, bytes)?;
        Ok((ptr, bytes.len() as u32))
    }

    fn take(&mut self, packed: u64) -> Result<Vec<u8>> {
        let (ptr, len) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
        let buf = read_guest(&self.memory, &self.store, ptr, len)?;
        let dealloc: TypedFunc<(u32, u32), ()> = self.typed("dealloc")?;
        dealloc.call(&mut self.store, (ptr, len))?;
        Ok(buf)
    }

    /// これまでのホスト関数呼び出し `(fn, args)` を取り出す。
    pub fn take_host_log(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.store.data_mut().log)
    }

    /// `snapshot()` / `patch()` の答え(境界メッセージ)。
    pub fn message(&mut self, export: &str) -> Result<Value> {
        let f: TypedFunc<(), u64> = self.typed(export)?;
        let packed = f.call(&mut self.store, ()).with_context(|| format!("{export}() がトラップしました"))?;
        serde_json::from_slice(&self.take(packed)?).with_context(|| format!("{export} の答えが JSON ではありません"))
    }

    pub fn call(&mut self, fn_name: &str, args: &Value, call_id: &str) -> Result<Value> {
        let f = self.write(fn_name.as_bytes())?;
        let a = self.write(args.to_string().as_bytes())?;
        let c = self.write(call_id.as_bytes())?;
        let call: TypedFunc<(u32, u32, u32, u32, u32, u32), u64> = self.typed("call")?;
        let packed = call.call(&mut self.store, (f.0, f.1, a.0, a.1, c.0, c.1)).with_context(|| format!("call({fn_name}) がトラップしました"))?;
        let dealloc: TypedFunc<(u32, u32), ()> = self.typed("dealloc")?;
        for (p, l) in [f, a, c] {
            dealloc.call(&mut self.store, (p, l))?;
        }
        let bytes = self.take(packed)?;
        serde_json::from_slice(&bytes).with_context(|| format!("call({fn_name}) の答えが JSON ではありません"))
    }
}

/// ゲストが指定した `(ptr, len)` を線形メモリから読む。確保より先に範囲を確かめる
/// (`rasq_wasm_runtime_core::read_guest` と同じ。長さは u32 なので、先に確保すると最大 4 GiB を確保しにいく)。
fn read_guest(memory: &Memory, ctx: impl wasmi::AsContext, ptr: u32, len: u32) -> Result<Vec<u8>, wasmi::Error> {
    let (ptr, len) = (ptr as usize, len as usize);
    let size = memory.data_size(&ctx);
    if ptr.checked_add(len).is_none_or(|end| end > size) {
        return Err(wasmi::Error::new(format!("guest buffer out of bounds: ptr {ptr} len {len} (memory {size} bytes)")));
    }
    // 直前で線形メモリの範囲 (ptr + len <= size) を確かめているので、この確保は size 以下。
    let mut buf = vec![0u8; len];
    memory.read(&ctx, ptr, &mut buf)?;
    Ok(buf)
}

/// `rasq.host_fn` の代役: 決め打ちの応答があればそれを、無ければ `E01`(ホスト無し)。呼び出しは記録する。
fn host_fn_stub(mut caller: Caller<'_, HostState>, ptr: u32, len: u32) -> Result<u64, wasmi::Error> {
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).ok_or_else(|| wasmi::Error::new("no memory"))?;
    let req = read_guest(&memory, &caller, ptr, len)?;
    let req: Value = serde_json::from_slice(&req).unwrap_or(Value::Null);
    let call_id = req["callId"].as_str().unwrap_or("").to_string();
    let fn_name = req["fn"].as_str().unwrap_or("").to_string();
    let host = caller.data_mut();
    host.log.push((fn_name.clone(), req["args"].clone()));
    let resp = match host.replies.get(&fn_name) {
        Some(v) => json!({"callId": call_id, "ok": true, "value": v}),
        None => json!({"callId": call_id, "ok": false, "error": {"code": "E01", "message": "rasq: no host"}}),
    }
    .to_string();
    let alloc = caller
        .get_export("alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmi::Error::new("no alloc"))?
        .typed::<u32, u32>(&caller)
        .map_err(|e| wasmi::Error::new(e.to_string()))?;
    let out = alloc.call(&mut caller, resp.len() as u32)?;
    memory.write(&mut caller, out as usize, resp.as_bytes()).map_err(|e| wasmi::Error::new(e.to_string()))?;
    Ok(((out as u64) << 32) | resp.len() as u64)
}

/// `snapshot` を 1 回呼び、`call("plugin.manifest")` の `value` を返す。
pub fn manifest_from_wasm(bytes: &[u8]) -> Result<Value> {
    let mut p = Plugin::load(bytes)?;
    let snapshot: TypedFunc<(), u64> = p.typed("snapshot")?;
    let packed = snapshot.call(&mut p.store, ()).context("snapshot() がトラップしました")?;
    let snap: Value = serde_json::from_slice(&p.take(packed)?).context("snapshot の答えが JSON ではありません")?;
    if snap["schema"] != "rasq.boundary/1" || snap["type"] != "snapshot" {
        bail!("snapshot の答えが rasq.boundary/1 の snapshot ではありません: {}", snap);
    }
    let r = p.call("plugin.manifest", &json!({}), "rasq-check-1")?;
    if r["ok"] != true {
        bail!("call(plugin.manifest) が失敗: {}", r["error"]);
    }
    Ok(r["value"].clone())
}
