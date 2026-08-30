# ホスト関数 — プラグインから使えるホスト機能

> 正典: [`PROTOCOL.md`](../PROTOCOL.md) §4、[`schema/host-fn.schema.json`](../schema/host-fn.schema.json)。

呼び方(v1・同期): import `rasq.host_fn` へ `rasq.hostfn/1` の JSON を渡す。

```rust
let r = host_call("storage.set", json!({"ns": "safe", "key": "greets", "value": 3}));
// r = {"callId": "...", "ok": true, "value": {}} / {"ok": false, "error": {"code", "message"}}
```

権限の無い fn・未知の fn は `{ok:false, error:{code:"E01"|"C01"}}` — **トラップはしない**ので
戻りの `ok` を見るだけでよい。

| fn | 何ができるか | 権限 | 状態 |
|---|---|---|---|
| `storage.get/set/delete` | プラグイン名前空間の KV(`ns: safe|unsafe`) | `storage.safe` / `storage.unsafe` | ✅ |
| `net.fetch` | ホスト代理の HTTP(ランタイム内蔵 rustls、モバイル可) | `network` | ✅ |
| `alert` | 確認ダイアログ(戻り `{confirmed}`) | — | ✅(dev はログ+自動確認) |
| `fow.open` / `fow.close` | FOW(拡張面)を開く/閉じる → [fow.md](fow.md) | `fow.widget` 等 | ✅ |
| `ml.infer` | ML 実行(マニフェスト id 参照、配置はホスト解決) | `gpu_ml` | 🔶 未配線(E01) |
| `wasm.call` | 子プラグイン呼び出し | 構成許可 | 🔶 未配線 |
| `store.read` | ホスト Store の読み出し | — | 🔶 未配線 |
| `geometry.lookup` | 画面上の矩形検索 | `fow.system` | 🔶(登録簿はホスト側実装済み — `fow.open` の `anchor` から使える) |

## storage の例(hello プラグインの `greet`)

```rust
let stored = host_call("storage.get", json!({"ns": "safe", "key": "greets"}));
let n = stored["value"]["value"].as_u64().unwrap_or(0) + 1;
host_call("storage.set", json!({"ns": "safe", "key": "greets", "value": n}));
```

キーは pluginId で自動プレフィックスされ、他プラグインとは衝突しない。

## net.fetch の例(misskey-wasm-sample の共通口)

```rust
let response = host_call("net.fetch", json!({
    "method": "POST",
    "url": "https://misskey.io/api/notes/local-timeline",
    "headers": { "Content-Type": "application/json" },
    "body": json!({ "limit": 10 }).to_string(),
}));
let status = response["value"]["status"].as_u64().unwrap_or(0);
let body_text = response["value"]["bodyText"].as_str().unwrap_or("");
```

- v1 は同期(呼び出し中プラグインはブロック)。ホスト側はワーカー isolate で
  動かすので **UI は止まらない**(非同期 v2 = callId ベースの poll/notify は将来)。
- URL は https/http のみ。エラーは `B01`(到達不能)/ `B03`(不正 URL / HTTP エラー)。

## 実行モデルの注意

- `fow.open` などの UI 系ホスト関数は**受理応答**(`{opened:true}`)がすぐ返り、
  実際の表示は UI isolate 側で非同期に行われる。結果・通知は後から
  `call`(`onClosed` / `lifecycle`)で届く。
- ホスト関数の**最中に自分へ再入されることはない**(通知はマイクロタスクで
  呼び出しが解けてから届く)。
