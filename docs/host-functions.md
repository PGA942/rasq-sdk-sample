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
| `gpu.compute` | WGSL compute 1 パス(入力 1 バッファ → 出力 1 バッファ)→ [下](#gpucompute-の権限と上限) | `gpu_ml` | ✅(ネイティブ = ランタイム内 wgpu) |
| `wasm.call` | 子プラグイン呼び出し | 構成許可 | 🔶 未配線 |
| `store.read` | ホスト Store の読み出し | — | 🔶 未配線 |
| `geometry.lookup` | 画面上の矩形検索 | `fow.system` | 🔶(登録簿はホスト側実装済み — `fow.open` の `anchor` から使える) |
| `page.render` | ページ型(slot `page`)のページの RRL を丸ごと差し替える `{token, root, title?}` → [slots.md §2.13](slots.md) | `render` | ✅ v0.18 |
| `page.end` | プラグインが自分で終わる `{token}`(surface `swing` では見終えたことにして次へ)→ [slots.md §2.13](slots.md) | `render` | ✅ v0.19 |
| `shared.set` | shared のサーバー変数を書く `{token, key, value, ifRev?, ttl?}`(`me.` → `@<accountId>.`、`null` で消す。`ttl` = 残す秒数、v0.22) | `shared.session` | ✅ v0.18 |
| `shared.op` | サーバー変数を原子的に `{token, key, op: incr\|append\|remove\|merge, value, ttl?}` | `shared.session` | ✅ v0.18 |
| `shared.call` | 参加者全員(自分も)へ `page.event {type: "call", fn, args, from}` を配る `{token, fn, args}` | `shared.session` | ✅ v0.18 |
| `media.editor.open` | メディア編集画面を SVG モードで開く `{token, format: "svg", aspect?, svg?, title?}` → `page.event {type: "edited", svg}` / `{type: "editCancelled"}` → [slots.md §2.13](slots.md) | `render` | ✅ v0.22 |

## shared(ページ型のサーバー変数 / 関数、v0.18)の例

```rust
use rasq_plugin_kit::host;
host::shared_op(&token, "counter", "incr", json!(1));            // 全員の counter に +1
host::shared_set(&token, "me.vote", json!(2), None);              // 自分の票(サーバーが @<accountId>.vote に書き換える)
host::shared_call(&token, "celebrate", json!({}));                // 全員(自分も)に page.event {type:"call", fn:"celebrate"}
host::page_render(&token, root, None);                            // 受けた page.event {type:"var"} で描き直す
host::shared_set_ttl(&token, "me.paint", json!({"svg": svg}), 86_400); // v0.22: 1 日で Central が消す(var {value: null, reason: "expired"})
host::media_editor_open(&token, "3:4", Some(&svg), None);         // v0.22: SVG の落書き → page.event {type:"edited", svg}
```

- **反映は必ず `page.event {type: "var", key, value, rev, by}` で届く**(自分の書き込みも)。手元で先に値を変えない。
- セッションが無い(solo / 1 人で開いた / `closed` のあと)ときは `E01`。キーの規則と上限は [slots.md §2.13](slots.md)。
- (v0.22)時刻は `page.open` / `page.event` / RRL のイベントの **`now`**(エポックミリ秒)を使う(wasm では `SystemTime::now` がトラップする)。
  期限は `vars` / `var` の `expiresAt`。消えた理由は `var` の `reason`(`expired` / `evicted` / `removedByOwner`)。

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
- **繋げるのはマニフェストの `network` で宣言した先だけ** (v0.12)。`network.hosts`(完全一致 /
  `*.example.com` = サブドメインだけ / `*` = どこでも、開発用)と `network.instance`
  (そのセッションのアカウントのインスタンス。ホストが `args.host` に渡すものと同じ)。
  それ以外のホストは `E01`(`host not permitted`)。宣言が無ければどこにも繋げない
  (fail-closed)— `network` 権限だけでは動かない。
- **リダイレクトは追わない**。許可した先が 302 で別のホストへ飛ばしても付いて行かず、
  3xx がそのまま返る。飛ぶかどうかはプラグインが次の `net.fetch` で決める(その URL も
  また検査される)。
- **1 回の呼び出し(`call` / `snapshot` / `patch`)ごとの予算**(ネイティブ版、セキュリティ確認 2026-10-02 #22):
  通信は 1 回 15 秒までに加えて、1 回の呼び出しの中の**合計の壁時計 30 秒**(通信を待つ時間も含む。通信の上限には
  残り時間が渡る)・**通信 64 回**・**応答の本文の合計 64 MiB** まで。超えるとその呼び出しは `E02` で止まる
  (燃料は通信を待つ間は減らないので、遅いホストへの通信を繰り返しても呼び出しを占有できない)。
  埋込側は `rasq_wasm_set_call_timeout_ms` で壁時計の予算を変えられる。
- **通信の形が控えられる**(ユーザーが同意した端末だけ、ストアへ報告)。形 = host / method /
  path のテンプレート(`/api/users/{hex}/notes`)/ query のキー / ヘッダ名 / JSON 本文のキー構造で、
  **値(トークン・本文・ユーザー名)は端末の外に出ない**。承認済みの版に初めて現れた形があると、
  審査がその版を読み直す。1 つの版に 64 種類を超える形は「形を変え続けている」として旗になる。

## gpu.compute の権限と上限

```rust
let r = host_call("gpu.compute", json!({
    "wgsl": WGSL,                  // @group(0) @binding(0) = 入力(read)、@binding(1) = 出力(read_write)
    "entry": "main",
    "input": b64(&pixels),         // binding 0 の中身
    "outputSize": 4 * n,           // binding 1 のバイト数(4 の倍数)
    "workgroups": [n.div_ceil(64), 1, 1],
}));
// 成功: r["value"]["output"](base64)。失敗: E01 ならクラウドカルクレーター(ml.infer)へ逃がす
```

- **`gpu_ml` 権限が要る**(審査済み)。この fn はランタイムの中で wgpu に直結していて埋込側の host_fn を
  通らないので、門もランタイムにある: 埋込側が `rasq_wasm_set_gpu_compute(.., true)`(Dart では
  `RasqWasmPlugin.fromBytes(.., gpuCompute: true)` / `setGpuCompute`)で開けたインスタンスだけが使え、
  **既定は閉**(`E01` `permission denied`)。埋込側はマニフェストの `gpu_ml` が審査で通っているときだけ開ける。
- 要求は wgpu に渡す**前に**検査する。外れたら `D01`(GPU の初期化もしない):

  | 引数 | 上限 |
  |---|---|
  | `wgsl` | 空でない、256 KiB まで |
  | `entry` | 空でない、256 バイトまで(省略 = `main`) |
  | `input` | base64、デコード後 64 MiB まで(デコードの前に文字数で切る) |
  | `outputSize` | 4 の倍数、4 〜 64 MiB |
  | `workgroups` | `[x, y?, z?]`、各 1 〜 65535 かつ端末の `max_compute_workgroups_per_dimension` 以下、x·y·z は 2^20 まで |

  入力・出力は端末の `max_storage_buffer_binding_size` / `max_buffer_size` も超えられない。
- **1 回 10 秒**(壁時計。前の呼び出しの終わりを待つ時間も含む)。GPU を待つ間は燃料が減らないので別に切る。
  超えたら `E01` で、**そのプロセスでは以後 `gpu.compute` を止める**(投げたシェーダーは取り消せないので、
  無限ループを積み増させない)。デバイス喪失・wgpu 内部の失敗のあとも同じ。
- WGSL の文法エラー・バインドの不一致・`entry` 違いなど wgpu の検証エラーは `E01`(`validation failed`)。
  ホストは落ちない(wgpu の既定の「検証エラーで panic」は差し替えてある)。検証エラーでは止めない。
- 呼び出しは端末全体で 1 本ずつ(直列)。
- Web ビルド(WebGPU に写す想定)の埋込側も同じ門(`gpu_ml`)と同じ上限を守ること。数字はランタイムの
  `MAX_GPU_*` / `GPU_TIMEOUT_MS`(`rust/rasq_wasm_runtime_core/src/lib.rs`)が正典。

## 実行モデルの注意

- `fow.open` などの UI 系ホスト関数は**受理応答**(`{opened:true}`)がすぐ返り、
  実際の表示は UI isolate 側で非同期に行われる。結果・通知は後から
  `call`(`onClosed` / `lifecycle`)で届く。
- ホスト関数の**最中に自分へ再入されることはない**(通知はマイクロタスクで
  呼び出しが解けてから届く)。
