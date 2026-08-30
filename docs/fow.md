# FOW — プラグインの拡張面(Function Override Widget)

> 正典: [`PROTOCOL.md`](../PROTOCOL.md) §4(`fow.open` の行)と §3.1(patch の `surface`)。
> 動くデモ: hello プラグインの「パネルを開く」、misskey-wasm-sample のリアクション FOW。

FOW は**ホスト所有のドック面**。プラグインは RRL ツリーを渡して開き、
中のイベントは通常どおり `call` で戻ってくる。ドッキング(カラム移動・
壁付け・ヘッダー/フッター付け・スイッチャー・親子の回避伸縮)は全部ホストの仕事。

## 開く

```rust
host_call("fow.open", json!({
    "id": "hello-panel",                       // プラグインローカル id(再要求 = 中身差し替え)
    "spec": {
        "type": "widget",                      // widget | interstitial | system(権限 fow.<type>)
        "confirmMode": "immediate",
        "initialHeightFactor": 0.3,
        "title": "hello パネル",                // ヘッダー行クローム(タイトル/キャンセル/決定)
        "cancelText": "閉じる",
        "attachTop": false,                     // true = アプリヘッダーに下付き
    },
    "anchor": { "node": "actions-xyz" },        // 任意: 自ドキュメントの実物ノードに張り付く
                                               // (role 付きノードが対象。{"role": "actionBar"} でも可)
    "parentId": null,                           // 親 FOW の id(子は親に追従し、親が伸縮して回避)
    "onClosed": "panelClosed",                  // 閉時に call される関数({id, reason, result?})
    "onCancel": null,                           // 指定時 ✕ は閉じずにこれが call される(確認ポップアップ用)
    "root": { "t": "Column", "k": "panel-root", "c": [ /* RRL */ ] }
}));
```

戻り `{opened: true}` は**受理**(表示は非同期)。エラー: 権限外 type = `E01`、id/root 欠落 = `D01`。

## 更新する(開いたまま差分)

FOW ドキュメントは open 時に rev 1 で始まる独立の系列。`surface` 付き patch を積み、
`call` の戻りで `patch: true` を返すとホストが引き取って適用する:

```rust
*PENDING_PATCH.lock().unwrap() = json!({
    "schema": "rasq.boundary/1", "scope": SCOPE, "type": "patch",
    "surface": "hello-panel", "rev": n + 1, "baseRev": n,
    "ops": [{ "op": "setProp", "target": format!("{SCOPE}:panel-count"), "prop": "text", "value": "3 回" }]
});
Ok(json!({ "patch": true }))
```

rev 不一致・未知 surface は破棄される — 再 `fow.open`(同 id)が resync。

## 閉じる・通知

- 自分から: `fow.close {id, result?, reason?}`
- ユーザーから(✕ / 余白タップ / スイッチャー): `onClosed` に `{id, reason, result?}` が call される
- あわせて `call("lifecycle", {kind: "mount"|"unmount", surface})` も届く(未対応なら C01 でよい)
- `onCancel` を指定すると ✕ で**閉じずに**その関数が呼ばれる — 確認 FOW を自分で開いて
  「破棄 / 続ける」を出し、自分で `fow.close` する(misskey-wasm-sample の `confirmCloseFow` 参照)

## ホスト側の挙動(プラグインは意識しなくてよいが知っておくと便利)

- FOW はカラム中心にドックし、ユーザーは FOW 内ヘッダー/フッターを掴んで
  左右カラム間・上下(ヘッダー付け⇄フッター付け)へ動かせる。壁へ 50px 以上で壁付けピン
- `anchor` 指定はアクションボタン等の実物矩形に張り付く(下付き基本・余白不足で上付き。
  別カラムへ動かされた時点で壁付き表示になり anchor は解除)
- 複数 FOW は重なり、ハンドルクリックの**スイッチャー**(プレビュー付き)で切り替え。
  親を選ぶと子(`parentId` / ownerId)は自動で閉じる
- 親 FOW の中身は `FowChildGap`(ホスト実装の可変 padding)が子の高さぶん場所を空ける —
  プラグインの親 FOW でも将来 RRL ノードとして置けるようにする予定(persistent 包み)
