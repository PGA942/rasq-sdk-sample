// rasq-sdk.yml の規則の例 (semgrep --test --config .semgrep/rasq-sdk.yml .semgrep/rasq-sdk.rs)。
// 拾うべき行の前に ruleid、拾ってはいけない行の前に ok の注釈を書く。コンパイルはしない (形だけの Rust)。

// ---- rasq-unsafe-outside-c-abi -------------------------------------------

fn host_fn_impl(raw: *mut c_char) -> String {
    // ruleid: rasq-unsafe-outside-c-abi
    let s = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
    s
}

#[no_mangle]
pub extern "C" fn rasq_wasm_destroy(instance: *mut RasqWasmInstance) {
    if !instance.is_null() {
        // ok: rasq-unsafe-outside-c-abi
        unsafe { drop(Box::from_raw(instance)) };
    }
}

extern "C" fn callback_impl(request: *const c_char) -> *mut c_char {
    // ok: rasq-unsafe-outside-c-abi
    let bytes = unsafe { CStr::from_ptr(request) }.to_bytes();
    rasq_wasm_string_dup(bytes.as_ptr())
}

// ---- rasq-runtime-panic ----------------------------------------------------

fn read_request(request: &[u8]) -> serde_json::Value {
    // ruleid: rasq-runtime-panic
    let req: serde_json::Value = serde_json::from_slice(request).unwrap();
    // ruleid: rasq-runtime-panic
    let fn_name = req["fn"].as_str().expect("fn");
    if fn_name.is_empty() {
        // ruleid: rasq-runtime-panic
        panic!("empty fn");
    }
    // ok: rasq-runtime-panic
    let call_id = req["callId"].as_str().unwrap_or("");
    // ok: rasq-runtime-panic
    let c = CString::new(call_id).unwrap_or_else(|_| c"{}".to_owned());
    drop(c);
    req
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        // ok: rasq-runtime-panic
        let v: serde_json::Value = serde_json::from_str("{}").unwrap();
        // ok: rasq-runtime-panic
        let s = std::fs::read(WASM_PATH).expect("build first");
        // ok: rasq-unsafe-outside-c-abi
        assert_eq!(unsafe { slice::from_raw_parts(p, n) }, &s[..]);
        drop(v);
    }
}

// ---- rasq-guest-len-alloc --------------------------------------------------

fn host_fn_raw(caller: Caller<'_, HostState>, memory: Memory, ptr: u32, len: u32) -> Vec<u8> {
    // ruleid: rasq-guest-len-alloc
    let mut request = vec![0u8; len as usize];
    memory.read(&caller, ptr as usize, &mut request).ok();
    // ruleid: rasq-guest-len-alloc
    let mut out = Vec::with_capacity(len as usize);
    out.extend_from_slice(&request);
    // ok: rasq-guest-len-alloc
    let request = read_guest(&memory, &caller, ptr, len);
    request.unwrap_or_default()
}

// ---- rasq-native-fetch-single-path / rasq-native-fetch-follows-redirects ----

fn native_fetch_response(url: &str, overall: Duration) -> serde_json::Value {
    // ok: rasq-native-fetch-single-path
    // ok: rasq-native-fetch-follows-redirects
    let agent = ureq::builder().timeout(overall).redirects(0).build();
    // ok: rasq-native-fetch-single-path
    // ruleid: rasq-native-fetch-follows-redirects
    let loose = ureq::builder().timeout(overall).build();
    let result = agent.request("GET", url).call();
    match result {
        Ok(r) => serde_json::json!({"status": r.status()}),
        // ok: rasq-native-fetch-single-path
        Err(ureq::Error::Status(code, _)) => serde_json::json!({"status": code, "loose": loose.request("GET", url).call().is_ok()}),
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn gpu_compute_response(url: &str) -> serde_json::Value {
    // ruleid: rasq-native-fetch-single-path
    let agent = ureq::builder().redirects(0).build();
    // ruleid: rasq-native-fetch-single-path
    let quick = ureq::get(url).call();
    serde_json::json!({"ok": agent.get(url).call().is_ok() && quick.is_ok()})
}

// ---- rasq-command-spawn ----------------------------------------------------

fn cargo(root: &Path, args: &[&str]) -> Result<()> {
    // ruleid: rasq-command-spawn
    let status = Command::new("cargo").args(args).current_dir(root).status()?;
    // ruleid: rasq-command-spawn
    let out = std::process::Command::new("rustc").arg("--version").output()?;
    // ok: rasq-command-spawn
    let name = Command::default();
    drop((status, out, name));
    Ok(())
}

// ---- rasq-tar-entry-path-unchecked -----------------------------------------

pub fn extract(bytes: &[u8]) -> Result<Vec<Entry>> {
    let dec = zstd::Decoder::new(bytes)?;
    let mut ar = tar::Archive::new(dec);
    let mut out = Vec::new();
    for e in ar.entries()? {
        let mut e = e?;
        // ok: rasq-tar-entry-path-unchecked
        let path = e.path()?.to_string_lossy().into_owned();
        check_path(&path)?;
        let mut b = Vec::new();
        e.read_to_end(&mut b)?;
        out.push(Entry { path, bytes: b });
    }
    Ok(out)
}

pub fn extract_loose(bytes: &[u8]) -> Result<Vec<Entry>> {
    let mut ar = tar::Archive::new(bytes);
    let mut out = Vec::new();
    for e in ar.entries()? {
        let mut e = e?;
        // ruleid: rasq-tar-entry-path-unchecked
        let path = e.path()?.to_string_lossy().into_owned();
        let mut b = Vec::new();
        e.read_to_end(&mut b)?;
        out.push(Entry { path, bytes: b });
    }
    Ok(out)
}

// ---- rasq-secret-in-output -------------------------------------------------

fn token_set(host: &str, tok: &str, k: &DevKey, a: &Account) {
    // ruleid: rasq-secret-in-output
    println!("保存しました: central {host} → {}", a.token);
    // ruleid: rasq-secret-in-output
    eprintln!("debug: secret = {}", k.secret);
    // ruleid: rasq-secret-in-output
    let line = format!("Authorization: Bearer {token}");
    // ok: rasq-secret-in-output
    println!("保存しました: central {host} → {}(token = {})", credentials::path()?.display(), credentials::mask(&tok));
    // ok: rasq-secret-in-output
    println!("  central {k:<32} token = {:<14} {}", credentials::mask(&a.token), a.updated_at);
    // ok: rasq-secret-in-output
    bail!("Central のトークンがありません: 環境変数 {token_env} か、rasq token set --central {}", publish::host_key(&central));
    // ok: rasq-secret-in-output
    println!("  public = {}\n  指紋 = {}", k.public, key::fingerprint(&k.public));
    drop(line);
}
