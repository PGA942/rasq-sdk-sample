//! Native WASM runtime for RASQ hosts (RASQ M3 / D-007).
//!
//! Wraps `wasmi` (a pure-Rust WASM *interpreter*, no JIT) so the same engine
//! can run on iOS, where JIT'd wasm runtimes are not viable. Exposes a small
//! C ABI so it can be linked into a Flutter FFI plugin and driven from Dart.
//! The wire format is `rasq.boundary/1` JSON (see `PROTOCOL.md`); this crate
//! only shuttles bytes across the host<->wasm boundary, it does not
//! interpret the messages.

// ホストの panic はアプリごと落ちる。プラグイン由来の値 (線形メモリ、host_fn の要求 JSON、wasm の
// 戻り値) を扱う本体では unwrap / expect / panic を使わない (.semgrep/rasq-sdk.yml の
// rasq-runtime-panic と同じ約束。CI の `rust` ジョブが clippy -D warnings で見る。テストは除く)。
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unreachable, clippy::todo, clippy::unimplemented))]
// C ABI (`rasq_wasm_*`) は埋込側 (Dart FFI) から生ポインタで呼ばれる。Rust の呼び出し元は無いので
// `unsafe fn` にしても契約は伝わらず、各関数の doc に書いた所有権と null の扱いがその契約。
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{c_char, c_void, CStr, CString};
use std::slice;
use std::time::Duration;
use wasmi::{
    Caller, Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TrapCode, TypedFunc,
};

// ---- リソース上限 (D-007 ランタイムの続き) --------------------------------
//
// プラグインはサンドボックスの外へは出られない。が、「出られない」ことと
// 「暴れない」ことは別の話で、無限ループ 1 本あれば呼び出し側スレッド
// (アプリではワーカー isolate)は永久に帰ってこない。サンドボックス破りより
// **枯渇**のほうが現実的な攻撃面なので、燃料(命令数)・線形メモリ・
// 壁時計(ネイティブ net.fetch)の 3 本で上限を張る。
//
// 数字はここに集約してある — 端末や審査区分ごとに詰めたくなる値なので、
// 実行時にも `rasq_wasm_set_fuel_per_call` / `rasq_wasm_set_fetch_timeout_ms`
// で上書きできるようにしてある。

// 燃料の実測値(release ビルド・wasmi インタプリタ / plugin-sample dist)。
// 1 呼び出しで焼く燃料は **プラグインの種別ではなく、渡されたピクセル数**で
// 決まる。桁で並べておく:
//
//   プラグイン(宣言 kind)          重い呼び出し              燃料      所要
//   trend_plugin      (trend)       snapshot                  117 K   < 1 ms
//   timeline_plugin   (view)        snapshot                  117 K   < 1 ms
//   post_process_plugin(postProc.)  snapshot                  117 K   < 1 ms
//   media_fullscreen  (mediaViewer) snapshot                  118 K   < 1 ms
//   misskey サンプル   (sns)         timeline(TL 再構築)      2.0 M    ~2 ms
//   media_transform   (action)      media.edit  2048×1536   2.149 G   2.8 s
//   media_process     (action)      media.edit  2048×1536   2.149 G   2.7 s
//   media_process     (action)      media.action(sticker)   0.640 G   0.6 s
//   media_process     (action)      media.commit 2048×1536  1.368 G   3.2 s
//   media_process     (action)      media.edit  4032×3024   8.465 G  12.6 s ←上限外
//
// ここから 2 つ分かる:
//
// 1. **kind では切れない。** 一番重い media_process は `action` を名乗り、
//    一番軽い media_fullscreen が `mediaViewer` である(manifest.schema.json
//    の kind は「どのスロットに刺さるか」であって計算量の宣言ではない)。
//    kind 別テーブルにすると `action` を最重量に合わせる羽目になり、ただの
//    タップ 1 個のアクションプラグインにも数 G の縄を渡すことになる —
//    切る軸そのものが間違っている。切るなら「ビットマップを触るか」で、それは
//    スロット (`composer.media`) と権限 (`replace_media` / `tag_media`) に
//    出ている。下の 2 本の推奨値はその軸。
// 2. **費用は入力画像で決まる。** 同じ 2048×1536 なら 4 つの media 系
//    プラグインが揃って 2.149 G — プラグイン差ではなく画素数の関数。
//    だから「余裕」は実装差ではなく入力サイズの振れに対して取る。

/// 1 回の `call` / `snapshot` / `patch` に与える燃料(wasmi の既定コストでは
/// ほぼ「実行命令数」)の既定値。
///
/// 上限は **契約上の作業サイズ(長辺 ≤ 2048、media.edit がそこへ縮小する)**
/// で測った最悪値 2.149 G の約 2.8 倍。余裕分は入力画像の振れ幅に充ててあり、
/// 素材がおよそ 9 MP までなら通る。それ以上(12 MP のカメラ写真 = 8.465 G)は
/// E02 で落ちる — これは**意図した線**で、インタプリタで 12.6 秒ブロック
/// するのは上限を緩める理由ではなく、ホストが `media.edit` に渡す前に
/// 縮小すべきという話。
///
/// 壁時計で見ると、この環境(release)の実効は画像処理で 0.4〜1.0 G fuel/s
/// なので、上限いっぱいで 6〜15 秒。**目標ではなく天井**である点に注意:
/// 正当な 2048 の編集は 2.7〜3.2 秒で、天井は暴走時にしか触らない。
/// ビットマップを触らないプラグインには [`FUEL_BUDGET_STANDARD`] まで
/// 絞ることを勧める(埋込側が `rasq_wasm_set_fuel_per_call` で上書き)。
const FUEL_PER_CALL: u64 = FUEL_BUDGET_MEDIA;

/// ビットマップを扱うプラグイン(`composer.media` スロット、`replace_media` /
/// `tag_media` 権限)向けの推奨値 = 既定値。上の計算のとおり実測最悪値の
/// 約 2.8 倍。
const FUEL_BUDGET_MEDIA: u64 = 6_000_000_000;

/// それ以外すべての推奨値。ビットマップを触らないプラグインの実測最悪値は
/// misskey サンプルの TL 再構築 2.0 M(スロット系サンプルは 1.0 M 以下)で、
/// 5000 万はそこに 24 倍の余裕。この予算なら暴走しても 0.1 秒台で止まる —
/// 軽い種別にまで 6 G を配ると「上限がある」と言えなくなる。
/// 埋込側が manifest を読んだ時点で `rasq_wasm_set_fuel_per_call` で適用する
/// (この値は `rasq_wasm_fuel_budget_standard()` からも引ける)。
const FUEL_BUDGET_STANDARD: u64 = 50_000_000;

/// ゲスト線形メモリの上限(バイト)。256 MiB = 4096 ページ。
///
/// media_process の実測ピークは契約上の作業サイズ(2048×1536)の
/// `media.commit` で **111 MiB**(base64 の引数 + デコード後 RGBA +
/// フル解像度の合成先が同時に載る)。当初 128 MiB にしていたが、これは
/// 実測ピークの 1.15 倍しかなく、12 MP のカメラ写真では 185 MiB に達して
/// E03 で落ちていた。256 MiB = 実測ピークの約 2.3 倍。
///
/// この値は [`FUEL_PER_CALL`] と揃えてある: 燃料が許すサイズ(〜9 MP)なら
/// メモリ側が先に落ちない、という関係。どちらが先に効くか分からない組は
/// 原因究明が難しくなるので、**燃料のほうが先に当たる**ように取ってある。
///
/// なお wasm の線形メモリは縮まない。12 MP を 1 枚編集したインスタンスは
/// 以後ずっと 185 MiB を抱えるので、埋込側は `media.commit` /
/// `media.cancel` のあとインスタンスを捨てること。
const MAX_MEMORY_BYTES: usize = 256 * 1024 * 1024;

/// テーブル(funcref)要素数の上限。`table.grow` 経由でホスト側の
/// Vec を膨らませる枯渇経路を塞ぐためのもので、通常の `dyn Trait` 相当の
/// 間接呼び出しなら数千要素も使わない。
const MAX_TABLE_ELEMENTS: usize = 100_000;

/// 1 インスタンスが持てる線形メモリ / テーブル / インスタンスの数。セキュリティ確認 2026-10-09 の [5]。
///
/// ABI(PROTOCOL.md §4)は `memory` を 1 本だけ書き出す前提で、Rust の wasm32 も
/// メモリ 1 本・テーブル 1 本(`__indirect_function_table`)しか出さない。wasmi の
/// `Config::default()` は multi-memory が有効で、[`MAX_MEMORY_BYTES`] /
/// [`MAX_TABLE_ELEMENTS`] は **1 本ごと**の上限なので、256 MiB の初期メモリを
/// 何本も宣言すれば合計で数 GiB を確保できた(初期化の確保は燃料の外)。
/// multi-memory を切ったうえで数も 1 に絞り、さらに [`AggregateLimits`] で
/// 合計を見る(どれか 1 つが緩んでも合計の予算は越えない)。
const MAX_MEMORIES: usize = 1;
const MAX_TABLES: usize = 1;
const MAX_INSTANCES: usize = 1;

/// [`StoreLimits`](1 本ごとの上限・数・成長失敗のトラップ)に、ストア全体の
/// **合計**の予算を重ねる `ResourceLimiter`。セキュリティ確認 2026-10-09 の [5]。
///
/// wasmi はインスタンス化の初期確保(`Memory::new` / `Table::new`、`current = 0`)も
/// 実行中の `memory.grow` / `table.grow` も同じ口で問い合わせてくるので、ここで
/// 足し込めばインスタンス化の前(数の検査)・中(初期確保)・後(成長)を一続きで縛れる。
struct AggregateLimits {
    inner: StoreLimits,
    /// 許可した線形メモリの合計(バイト)と、テーブル要素の合計。
    memory_bytes: usize,
    table_elements: usize,
    /// 直前に許可した増分。許可のあと確保そのものが失敗したら(`*_grow_failed`)差し戻す。
    pending_memory: usize,
    pending_table: usize,
}

impl AggregateLimits {
    fn new() -> Self {
        Self {
            inner: StoreLimitsBuilder::new()
                .memory_size(MAX_MEMORY_BYTES)
                .table_elements(MAX_TABLE_ELEMENTS)
                .memories(MAX_MEMORIES)
                .tables(MAX_TABLES)
                .instances(MAX_INSTANCES)
                // 上限に当たった `memory.grow` は -1 ではなくトラップにする。
                // 仕様どおり -1 を返すと、ゲストのアロケータはたいてい
                // null → abort/unreachable で落ちるので、どのみちプラグインは
                // 死ぬ。それなら原因の分かる形(E03)で落としたほうが、
                // 「プラグインのバグ」と「上限に当てた」を運用で切り分けられる。
                .trap_on_grow_failure(true)
                .build(),
            memory_bytes: 0,
            table_elements: 0,
            pending_memory: 0,
            pending_table: 0,
        }
    }
}

/// 合計の予算を越える増分を拒む(`trap_on_grow_failure` と同じくトラップ = E03)。
fn grow_within(total: &mut usize, pending: &mut usize, current: usize, desired: usize, cap: usize) -> Result<bool, wasmi_core::LimiterError> {
    let add = desired.saturating_sub(current);
    let Some(next) = total.checked_add(add).filter(|n| *n <= cap) else {
        return Err(wasmi_core::LimiterError::ResourceLimiterDeniedAllocation);
    };
    *total = next;
    *pending = add;
    Ok(true)
}

impl wasmi::ResourceLimiter for AggregateLimits {
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> Result<bool, wasmi_core::LimiterError> {
        self.pending_memory = 0;
        if !self.inner.memory_growing(current, desired, maximum)? {
            return Ok(false);
        }
        grow_within(&mut self.memory_bytes, &mut self.pending_memory, current, desired, MAX_MEMORY_BYTES)
    }

    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> Result<bool, wasmi_core::LimiterError> {
        self.pending_table = 0;
        if !self.inner.table_growing(current, desired, maximum)? {
            return Ok(false);
        }
        grow_within(&mut self.table_elements, &mut self.pending_table, current, desired, MAX_TABLE_ELEMENTS)
    }

    fn memory_grow_failed(&mut self, _error: &wasmi_core::LimiterError) {
        self.memory_bytes = self.memory_bytes.saturating_sub(std::mem::take(&mut self.pending_memory));
    }

    fn table_grow_failed(&mut self, _error: &wasmi_core::LimiterError) {
        self.table_elements = self.table_elements.saturating_sub(std::mem::take(&mut self.pending_table));
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

/// ネイティブ `net.fetch` (D-023) の壁時計上限(ミリ秒・呼び出し全体)。
///
/// 燃料は純粋な計算しか縛れない。host_fn の中でブロッキング HTTP を待って
/// いる間、ゲストは 1 命令も実行しないので燃料は減らない — つまり遅い
/// (あるいは意図的に引き延ばされた)サーバーは燃料を素通りして呼び出し
/// スレッドを占有できる。従来と同じ 15 秒だが、値を定数へ出して「ここが
/// 壁時計の上限である」ことを明示する。スレッドは足さない:
/// ureq 側に接続/読み/書き/全体のタイムアウトが揃っているので、それを
/// 使い切るほうが構造が増えなくて済む。
const DEFAULT_FETCH_TIMEOUT_MS: u64 = 15_000;

/// 接続確立までの上限。名前解決 + TCP + TLS が 5 秒かかる先は、全体 15 秒を
/// 待たせるより早めに B01 で返したほうがプラグインの再試行が回る。
const FETCH_CONNECT_TIMEOUT_MS: u64 = 5_000;

/// 1 回の read/write が止まってよい上限(slowloris 対策)。全体上限より
/// 短くしておかないと「1 バイトずつ送り続ける」相手に全体上限いっぱい
/// 付き合うことになる。
const FETCH_IO_TIMEOUT_MS: u64 = 10_000;

/// レスポンス本文の読み取り上限(バイト)。`read_to_end` は相手が送るだけ
/// ホスト RAM を食うので、ゲストのメモリ上限とは別に口を絞る。32 MiB は
/// 画像/動画サムネの実寸より十分大きく、TL の JSON なら桁が違う。
const MAX_FETCH_BODY_BYTES: u64 = 32 * 1024 * 1024;

/// 1 回のゲスト呼び出し(`call` / `snapshot` / `patch`)の壁時計の予算(ミリ秒)。セキュリティ確認 2026-10-02 #22。
///
/// [`DEFAULT_FETCH_TIMEOUT_MS`] は通信 **1 回** の上限で、燃料は通信を待つ間は減らない。だから遅い(許可された)ホストへの
/// 通信を繰り返すと、1 回の呼び出しが「通信の回数 × 15 秒」だけ呼び出しスレッドを占有できた。いまは呼び出しの入り口
/// ([`RasqWasmInstance::refuel`])で締め切りを決め、ホスト関数(ネイティブ `net.fetch` / 埋込側の callback)の
/// 入り口ごとに残り時間を確かめ、通信の上限には**残り時間**を渡す。締め切りを過ぎたらトラップ(E02)。
/// 埋込側は `rasq_wasm_set_call_timeout_ms` で変えられる。
const DEFAULT_CALL_WALL_MS: u64 = 30_000;

/// 1 回のゲスト呼び出しで出せるネイティブ `net.fetch` の数。素直なプラグインは 1 呼び出しで数回。
const MAX_NATIVE_FETCHES_PER_CALL: u32 = 64;

/// 1 回のゲスト呼び出しで読むネイティブ `net.fetch` の本文の合計(バイト)。1 回ぶんの [`MAX_FETCH_BODY_BYTES`] とは別に。
const MAX_FETCH_BYTES_PER_CALL: u64 = 64 * 1024 * 1024;

/// 1 回のゲスト呼び出しの予算を使い切った(ホスト関数の入り口で止めた)。トラップとして返し、E02 にする。
#[derive(Debug)]
enum CallBudgetExceeded {
    WallClock(u64),
    Fetches,
    FetchBytes,
}

impl std::fmt::Display for CallBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallBudgetExceeded::WallClock(ms) => write!(f, "plugin exceeded its wall-clock budget ({ms} ms per call, including network waits)"),
            CallBudgetExceeded::Fetches => write!(f, "plugin exceeded its network budget ({MAX_NATIVE_FETCHES_PER_CALL} fetches per call)"),
            CallBudgetExceeded::FetchBytes => write!(f, "plugin exceeded its network budget ({MAX_FETCH_BYTES_PER_CALL} response bytes per call)"),
        }
    }
}

impl wasmi::errors::HostError for CallBudgetExceeded {}

/// 1 インスタンスが控える通信の形の上限。素直なプラグインは API の口の数(数十)で
/// 収まる。超えたら数えるのをやめ、超えたこと自体を 1 度だけ報告する — 形を毎回
/// 少し変えて再審査を浪費させる(または埋もれさせる)プラグインを作れるため。
const MAX_FETCH_SHAPES: usize = 256;

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
    /// 壁時計の上限。ネイティブ `net.fetch` 一回ぶんの全体タイムアウト。
    fetch_timeout_ms: u64,
    /// 1 回のゲスト呼び出しの壁時計の予算([`DEFAULT_CALL_WALL_MS`])。
    call_wall_ms: u64,
    /// いまの呼び出しの締め切り(`refuel` で決める)。
    call_deadline: Option<std::time::Instant>,
    /// いまの呼び出しで出したネイティブ `net.fetch` の数と、読んだ本文の合計。
    call_fetches: u32,
    call_fetch_bytes: u64,
    /// ネイティブ `net.fetch` が繋いでよいホスト(マニフェストの `network.hosts` と、
    /// 埋込側が今のセッションに与えたインスタンスのホスト)。小文字。
    /// **空 = どこにも繋げない**(fail-closed): 埋込側が設定を忘れた時点で
    /// 黙って全開になる既定値にはしない。
    fetch_hosts: Vec<String>,
    /// 通信の形の控え (D-004 の続き、審査の再読み用)。このインスタンスが
    /// 出した `net.fetch` の**値を持たない形**のハッシュ。埋込側が
    /// `rasq_wasm_take_fetch_shapes` で初めて見た形だけを引き取る。
    fetch_shapes_seen: std::collections::BTreeSet<String>,
    fetch_shapes_new: Vec<serde_json::Value>,
    /// 形の数が [`MAX_FETCH_SHAPES`] を超えた(以後は数えない)。それ自体を 1 度だけ報告する。
    fetch_shapes_overflow: bool,
    /// `gpu.compute` (D-081) を使わせるか = マニフェストの `gpu_ml` が審査で通っているか。
    /// この口はランタイム内で短絡して埋込側の権限検査を通らないので、ここで閉じる。
    /// **既定は false**(fail-closed): 埋込側が設定を忘れても全プラグインに GPU を渡さない。
    gpu_compute: bool,
    /// 線形メモリ / テーブルの成長を止める `ResourceLimiter`。`Store` は
    /// ユーザーデータ経由でしか limiter を引けないのでここに同居させる。
    /// 1 本ごとの上限に加えて合計も見る([`AggregateLimits`]、2026-10-09 の [5])。
    limits: AggregateLimits,
}

pub struct RasqWasmInstance {
    store: Store<HostState>,
    memory: Memory,
    instance: Instance,
    /// 「1 回の呼び出し」に配る燃料。呼び出しごとに積み直す(`refuel`)。
    fuel_per_call: u64,
}

#[derive(Debug)]
enum RasqWasmError {
    Wasmi(wasmi::Error),
    MissingExport(&'static str),
    Utf8,
    /// 1 回の呼び出しの壁時計 / 通信の予算を使い切った(遅い通信の繰り返しなど、#22)。E02。
    CallBudget(String),
    /// 燃料切れ = プラグインが暴走した(無限ループ・爆発的な再帰)。
    /// 埋込側が「暴走」と「クラッシュ」を切り分けられるよう、一般の
    /// トラップとは別の口にしてある — 前者はプラグインの無効化/再審査、
    /// 後者は単にそのタイル 1 枚の縮退、と処置が違う (PROTOCOL §3.4)。
    FuelExhausted,
    /// 線形メモリ(またはテーブル)が上限を超えて伸びようとした。
    /// 同じく「クラッシュ」とは別枠。
    MemoryLimit,
}

impl From<wasmi::Error> for RasqWasmError {
    fn from(e: wasmi::Error) -> Self {
        // 枯渇系だけは先に掬い出す。wasmi は燃料切れも成長拒否もトラップ
        // として返してくるので、ここで分類しないと埋込側からは
        // 「プラグインが落ちた」と見分けがつかない。
        if let Some(b) = e.downcast_ref::<CallBudgetExceeded>() {
            return RasqWasmError::CallBudget(b.to_string());
        }
        match e.as_trap_code() {
            Some(TrapCode::OutOfFuel) => return RasqWasmError::FuelExhausted,
            Some(TrapCode::GrowthOperationLimited) => return RasqWasmError::MemoryLimit,
            _ => {}
        }
        // インスタンス化時(まだ実行に入っていない)の拒否はトラップでは
        // なくエラー種別で届く。初期メモリが上限を超えるモジュールがここ。
        match e.kind() {
            wasmi::errors::ErrorKind::Memory(
                wasmi::errors::MemoryError::ResourceLimiterDeniedAllocation,
            )
            | wasmi::errors::ErrorKind::Table(
                wasmi::errors::TableError::ResourceLimiterDeniedAllocation,
            ) => RasqWasmError::MemoryLimit,
            _ => RasqWasmError::Wasmi(e),
        }
    }
}

impl From<wasmi::errors::MemoryError> for RasqWasmError {
    fn from(e: wasmi::errors::MemoryError) -> Self {
        RasqWasmError::Wasmi(e.into())
    }
}

impl RasqWasmInstance {
    fn load(wasm_bytes: &[u8], callback: Option<RasqHostFnCallback>, user_data: usize) -> Result<Self, RasqWasmError> {
        // 燃料計測は翻訳時に計測命令を埋め込む方式なので、Engine を作る前に
        // Config で入れておく必要がある(`Engine::default()` では後から
        // 有効にできない)。
        let mut config = Config::default();
        config.consume_fuel(true);
        // ABI はメモリ 1 本(`memory`)。multi-memory を切って、2 本目を宣言するモジュールは
        // 検証(確保の前)で落とす(2026-10-09 の [5]、数と合計は下の limiter でも見る)。
        config.wasm_multi_memory(false);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm_bytes)?;
        let limits = AggregateLimits::new();
        let mut store = Store::new(
            &engine,
            HostState {
                callback,
                user_data,
                native_fetch: false,
                fetch_timeout_ms: DEFAULT_FETCH_TIMEOUT_MS,
                call_wall_ms: DEFAULT_CALL_WALL_MS,
                call_deadline: None,
                call_fetches: 0,
                call_fetch_bytes: 0,
                fetch_hosts: Vec::new(),
                fetch_shapes_seen: Default::default(),
                fetch_shapes_new: Vec::new(),
                fetch_shapes_overflow: false,
                gpu_compute: false,
                limits,
            },
        );
        store.limiter(|state| &mut state.limits);
        // start セクション / instantiate もゲストコードなので、給油前だと
        // 「燃料 0」で即死する。ロード時にも 1 回分積んでおく。
        store.set_fuel(FUEL_PER_CALL)?;
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
            fuel_per_call: FUEL_PER_CALL,
        })
    }

    /// 呼び出しの入り口で燃料を積み直す。
    ///
    /// ロード時に一度だけ積む方式だと、長生きしたインスタンスほど使える
    /// 計算量が減っていき、正当なプラグインがいつか理由もなく落ちる
    /// (しかも「何回目の呼び出しで落ちるか」は端末の履歴依存になる)。
    /// 予算はあくまで「1 呼び出しあたり」で、呼び出しをまたいで繰り越さない。
    fn refuel(&mut self) -> Result<(), RasqWasmError> {
        self.store.set_fuel(self.fuel_per_call)?;
        // 壁時計の予算も呼び出しごと(燃料は通信を待つ間は減らないので、別に締め切りを持つ、#22)。
        let st = self.store.data_mut();
        st.call_deadline = Some(std::time::Instant::now() + Duration::from_millis(st.call_wall_ms.max(1)));
        st.call_fetches = 0;
        st.call_fetch_bytes = 0;
        Ok(())
    }

    #[cfg(test)]
    fn load_plain(wasm_bytes: &[u8]) -> Result<Self, RasqWasmError> {
        Self::load(wasm_bytes, None, 0)
    }

    #[cfg(test)]
    fn set_native_fetch(&mut self, enabled: bool) {
        self.store.data_mut().native_fetch = enabled;
    }

    #[cfg(test)]
    fn set_fetch_hosts(&mut self, hosts: &[&str]) {
        self.store.data_mut().fetch_hosts = hosts.iter().map(|h| h.to_ascii_lowercase()).collect();
    }

    #[cfg(test)]
    fn set_gpu_compute(&mut self, enabled: bool) {
        self.store.data_mut().gpu_compute = enabled;
    }

    #[cfg(test)]
    fn take_fetch_shapes(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.store.data_mut().fetch_shapes_new)
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
        let buf = read_guest(&self.memory, &self.store, ptr, len)?;
        let dealloc: TypedFunc<(u32, u32), ()> = self.typed_func("dealloc")?;
        dealloc.call(&mut self.store, (ptr, len))?;
        Ok(buf)
    }

    fn snapshot(&mut self) -> Result<String, RasqWasmError> {
        self.refuel()?;
        let f: TypedFunc<(), i64> = self.typed_func("snapshot")?;
        let packed = f.call(&mut self.store, ())?;
        let bytes = self.read_packed(packed)?;
        String::from_utf8(bytes).map_err(|_| RasqWasmError::Utf8)
    }

    fn patch(&mut self) -> Result<String, RasqWasmError> {
        self.refuel()?;
        let f: TypedFunc<(), i64> = self.typed_func("patch")?;
        let packed = f.call(&mut self.store, ())?;
        let bytes = self.read_packed(packed)?;
        String::from_utf8(bytes).map_err(|_| RasqWasmError::Utf8)
    }

    fn call(&mut self, fn_name: &str, args_json: &str, call_id: &str) -> Result<String, RasqWasmError> {
        // 引数の書き込み(ゲストの `alloc`)から結果の読み出しまでが 1 回の
        // 呼び出し。同じ予算で回す — `alloc` の中で無限ループしても止まる。
        self.refuel()?;
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

/// ゲストが指定した `(ptr, len)` を線形メモリから読む。**確保より先に範囲を確かめる**:
/// `len` はゲストが自由に決める u32 なので、先に `vec![0; len]` すると最大 4 GiB を
/// 確保しにいき、線形メモリの範囲外 (wasmi の `read` が失敗する) と分かる前にホストが
/// abort する。ゲスト由来の長さでの確保はここ 1 箇所だけ (.semgrep の rasq-guest-len-alloc)。
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

/// Implements the `rasq.host_fn` import (PROTOCOL.md §4.1, synchronous v1):
/// read the request JSON from wasm memory → call the embedder's callback →
/// allocate the response inside wasm memory (via the plugin's `alloc`) and
/// return `(ptr << 32) | len`.
fn host_fn_impl(mut caller: Caller<'_, HostState>, ptr: u32, len: u32) -> Result<i64, wasmi::Error> {
    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => return Err(wasmi::Error::new("plugin has no exported memory")),
    };
    let request = read_guest(&memory, &caller, ptr, len).map_err(|e| wasmi::Error::new(format!("host_fn: bad request buffer: {e}")))?;

    // 呼び出しの締め切りを過ぎていたら、ホストの仕事(通信・埋込側の callback)を始めずに止める (#22)。
    let remaining = remaining_call_time(caller.data())?;

    // 通信の形の控え: ネイティブ経路でも埋込側の経路でも、要求はここを通る。
    if let Ok(req) = serde_json::from_slice::<serde_json::Value>(&request) {
        if req["fn"] == "net.fetch" {
            if let Some((hash, shape)) = fetch_shape(&req["args"]) {
                let st = caller.data_mut();
                if st.fetch_shapes_seen.contains(&hash) {
                    // 既知
                } else if st.fetch_shapes_seen.len() >= MAX_FETCH_SHAPES {
                    if !st.fetch_shapes_overflow {
                        st.fetch_shapes_overflow = true;
                        st.fetch_shapes_new.push(serde_json::json!({"overflow": true}));
                    }
                } else {
                    st.fetch_shapes_seen.insert(hash.clone());
                    st.fetch_shapes_new.push(serde_json::json!({"hash": hash, "shape": shape}));
                }
            }
        }
    }

    // Native net.fetch short-circuit (D-023): parse the envelope; when the fn
    // is net.fetch and native fetch is enabled, answer inside the runtime.
    if caller.data().native_fetch {
        if let Ok(req) = serde_json::from_slice::<serde_json::Value>(&request) {
            if req["fn"] == "net.fetch" {
                let st = caller.data_mut();
                if st.call_fetches >= MAX_NATIVE_FETCHES_PER_CALL {
                    return Err(wasmi::Error::host(CallBudgetExceeded::Fetches));
                }
                st.call_fetches += 1;
                // 1 回の上限と、呼び出しの残り時間の短いほう。
                let timeout_ms = st.fetch_timeout_ms.min(remaining.as_millis().max(1) as u64);
                let body_budget = MAX_FETCH_BYTES_PER_CALL.saturating_sub(st.call_fetch_bytes);
                let hosts = st.fetch_hosts.clone();
                let (response, body_bytes) = native_fetch_with_budget(&req, timeout_ms, &hosts, body_budget);
                let st = caller.data_mut();
                st.call_fetch_bytes = st.call_fetch_bytes.saturating_add(body_bytes);
                if st.call_fetch_bytes > MAX_FETCH_BYTES_PER_CALL {
                    return Err(wasmi::Error::host(CallBudgetExceeded::FetchBytes));
                }
                // 待っているあいだに締め切りを過ぎたら、ゲストへ返さずに止める。
                remaining_call_time(caller.data())?;
                return write_response(&mut caller, memory, response.to_string().as_bytes());
            }
        }
    }

    // Native gpu.compute short-circuit (D-081): WGSL compute 専用の狭い
    // GPU 口。常にランタイム内で処理する(Dart 側に同期 GPU ブリッジは
    // 無い)。埋込側の callback を通らないので、gpu_ml 権限の検査もここ
    // (`rasq_wasm_set_gpu_compute` で開けたインスタンスだけ。既定は E01)。
    if let Ok(req) = serde_json::from_slice::<serde_json::Value>(&request) {
        if req["fn"] == "gpu.compute" {
            let response = gpu_compute_response(&req, caller.data().gpu_compute);
            return write_response(&mut caller, memory, response.to_string().as_bytes());
        }
    }

    let response: String = match caller.data().callback {
        None => r#"{"ok":false,"error":{"code":"E00","message":"no host_fn handler registered"}}"#.to_string(),
        Some(callback) => {
            // 要求に NUL が混ざっていたら (JSON としては不正) 空の要求に置き換える。
            let request_c = CString::new(request).unwrap_or_else(|_| c"{}".to_owned());
            let user_data = caller.data().user_data as *mut c_void;
            let raw = callback(request_c.as_ptr(), user_data);
            if raw.is_null() {
                r#"{"ok":false,"error":{"code":"E00","message":"host_fn handler returned null"}}"#.to_string()
            } else {
                // Ownership: the callback allocated this via rasq_wasm_string_dup.
                // nosemgrep: rasq-unsafe-outside-c-abi — 埋込側 (C ABI の callback) が rasq_wasm_string_dup で確保した NUL 終端文字列。ここで読んで解放する
                let s = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
                // nosemgrep: rasq-unsafe-outside-c-abi — 同上 (所有権はこの関数に移っている)
                unsafe { drop(CString::from_raw(raw)) };
                s
            }
        }
    };

    // 埋込側の callback が締め切りを越えて待たせたら、ゲストへ返さずに止める(callback 自体の打ち切りは埋込側の仕事)。
    remaining_call_time(caller.data())?;
    write_response(&mut caller, memory, response.as_bytes())
}

/// いまの呼び出しの残り時間。締め切りを過ぎていたらトラップ([`CallBudgetExceeded::WallClock`])。
/// 締め切りが無い(呼び出しの外、ロード時の start など)ときは予算いっぱい。
fn remaining_call_time(st: &HostState) -> Result<Duration, wasmi::Error> {
    match st.call_deadline {
        None => Ok(Duration::from_millis(st.call_wall_ms.max(1))),
        Some(deadline) => {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                Err(wasmi::Error::host(CallBudgetExceeded::WallClock(st.call_wall_ms)))
            } else {
                Ok(left)
            }
        }
    }
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

// ---- gpu.compute (D-081) --------------------------------------------------
// WGSL compute 専用の狭い GPU 口: 入力 1 バッファ(binding 0, read-only
// storage)→ 出力 1 バッファ(binding 1, read_write storage)の 1 パス。
// カスタム TL の重いソートやメディア編集フィルターの下地。アダプター無し・
// 検証エラーは E01 で返し、プラグインはクラウドカルクレーターへ逃がす。
//
// 権限: `gpu_ml`。この口はランタイム内で短絡するので埋込側の host_fn の
// 権限検査を通らない — だから**ここで**閉じる。埋込側が
// `rasq_wasm_set_gpu_compute(.., true)` で開けたインスタンス(マニフェストに
// `gpu_ml` があり、審査で通ったもの)だけが使える。既定は閉(fail-closed)。
//
// 上限: 開けたインスタンスでも要求の中身はプラグインが自由に決める。wgpu は
// 検証エラーを既定では panic で知らせ(uncaptured-error ハンドラ)、待ちにも
// 上限が無い。どちらもホストごと落ちる/止まる経路なので、
// 1. wgpu に触る前に args を下の定数(とデバイスの limits)で検査する(D01)、
// 2. 生成から encode までをエラースコープで囲み、submit の前に検証エラーを
//    拾う(submit 自体は wgpu 22 では失敗すると panic する)、
// 3. uncaptured-error ハンドラを差し替える(既定の panic を止める)、
// 4. 読み戻しの待ちを [`GPU_TIMEOUT_MS`] で切る、
// 5. それでも wgpu の中で panic したら catch_unwind で E01 に変える。
// タイムアウト・デバイス喪失・panic のあとはこのプロセスでは GPU を使わない
// (投げたジョブは取り消せないので、無限ループのシェーダーを積み増させない)。

/// 入力(binding 0)の上限。デコード後 64 MiB。契約上の作業サイズ
/// (2048×1536 の RGBA = 12 MiB)の 5 倍強で、wgpu 既定の
/// `max_storage_buffer_binding_size`(128 MiB)の半分。
const MAX_GPU_INPUT_BYTES: usize = 64 * 1024 * 1024;

/// base64 の `input` の文字数の上限(デコードより先に切る)。
const MAX_GPU_INPUT_B64_LEN: usize = MAX_GPU_INPUT_BYTES.div_ceil(3) * 4;

/// 出力(binding 1)の上限。64 MiB。**4 の倍数**であること
/// (`copy_buffer_to_buffer` の要件。外すと wgpu の検証エラー)。
const MAX_GPU_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

/// WGSL ソースの上限。256 KiB(フィルターやソートのシェーダーは数 KiB)。
const MAX_GPU_WGSL_BYTES: usize = 256 * 1024;

/// `entry` の名前の上限。
const MAX_GPU_ENTRY_BYTES: usize = 256;

/// `workgroups` の 1 次元あたりの上限。WebGPU / wgpu の既定の
/// `max_compute_workgroups_per_dimension`。デバイスの値がこれより小さければ
/// そちらで切る([`check_gpu_limits`])。
const MAX_GPU_WORKGROUPS_PER_DIM: u32 = 65_535;

/// `workgroups` の x·y·z の上限。2^20 = 1,048,576(12 MP の画像を 8×8 の
/// ワークグループで塗っても 19 万)。シェーダーの中のループは縛れないので、
/// 時間の上限は [`GPU_TIMEOUT_MS`] のほう。
const MAX_GPU_TOTAL_WORKGROUPS: u64 = 1 << 20;

/// 1 回の `gpu.compute` の壁時計上限(ミリ秒)。GPU を待つ間も燃料は減らない
/// (net.fetch と同じ)ので別に切る。直列化のための待ちもここに含む。
const GPU_TIMEOUT_MS: u64 = 10_000;

/// 検査済みの `gpu.compute` の要求。
#[derive(Debug)]
struct GpuJob {
    wgsl: String,
    entry: String,
    input: Vec<u8>,
    output_size: u64,
    workgroups: [u32; 3],
}

/// `gpu.compute` の args を検査する(D01 のメッセージを返す)。wgpu には
/// 触らないので GPU の無い CI でも試せる。デバイスごとの上限は
/// [`check_gpu_limits`]。
fn parse_gpu_args(args: &serde_json::Value) -> Result<GpuJob, String> {
    use base64::Engine as _;
    use serde_json::Value;
    let wgsl = match &args["wgsl"] {
        Value::String(s) if !s.is_empty() => s.as_str(),
        _ => return Err("gpu.compute: wgsl required".to_string()),
    };
    if wgsl.len() > MAX_GPU_WGSL_BYTES {
        return Err(format!("gpu.compute: wgsl too large: {} bytes (max {MAX_GPU_WGSL_BYTES})", wgsl.len()));
    }
    let entry = match &args["entry"] {
        Value::Null => "main",
        Value::String(s) if !s.is_empty() && s.len() <= MAX_GPU_ENTRY_BYTES => s.as_str(),
        _ => return Err(format!("gpu.compute: entry must be a non-empty string of at most {MAX_GPU_ENTRY_BYTES} bytes")),
    };
    let input_b64 = match &args["input"] {
        Value::Null => "",
        Value::String(s) => s.as_str(),
        _ => return Err("gpu.compute: input must be a base64 string".to_string()),
    };
    if input_b64.len() > MAX_GPU_INPUT_B64_LEN {
        return Err(format!("gpu.compute: input too large (max {MAX_GPU_INPUT_BYTES} bytes)"));
    }
    let input = base64::engine::general_purpose::STANDARD
        .decode(input_b64)
        .map_err(|e| format!("gpu.compute: bad input base64: {e}"))?;
    if input.len() > MAX_GPU_INPUT_BYTES {
        return Err(format!("gpu.compute: input too large: {} bytes (max {MAX_GPU_INPUT_BYTES})", input.len()));
    }
    let output_size = match args["outputSize"].as_u64() {
        Some(n) if n > 0 && n <= MAX_GPU_OUTPUT_BYTES && n % 4 == 0 => n,
        _ => {
            return Err(format!(
                "gpu.compute: outputSize must be a multiple of 4 in 4..={MAX_GPU_OUTPUT_BYTES}: {}",
                args["outputSize"]
            ))
        }
    };
    let mut workgroups = [1u32; 3];
    match &args["workgroups"] {
        Value::Null => {}
        Value::Array(a) if !a.is_empty() && a.len() <= 3 => {
            for (i, (slot, v)) in workgroups.iter_mut().zip(a).enumerate() {
                match v.as_u64() {
                    Some(n) if (1..=MAX_GPU_WORKGROUPS_PER_DIM as u64).contains(&n) => *slot = n as u32,
                    _ => {
                        return Err(format!(
                            "gpu.compute: workgroups[{i}] must be an integer in 1..={MAX_GPU_WORKGROUPS_PER_DIM}"
                        ))
                    }
                }
            }
        }
        _ => return Err("gpu.compute: workgroups must be [x, y?, z?]".to_string()),
    }
    let total: u64 = workgroups.iter().map(|&n| n as u64).product();
    if total > MAX_GPU_TOTAL_WORKGROUPS {
        return Err(format!("gpu.compute: too many workgroups: {total} (max {MAX_GPU_TOTAL_WORKGROUPS})"));
    }
    Ok(GpuJob { wgsl: wgsl.to_string(), entry: entry.to_string(), input, output_size, workgroups })
}

/// デバイスの limits で切る(下位のデバイスは既定値より小さいことがある)。
/// 外すと wgpu の検証エラー(wgpu 22 の既定では panic)になる値だけを見る。
fn check_gpu_limits(job: &GpuJob, limits: &wgpu::Limits) -> Result<(), String> {
    let per_dim = limits.max_compute_workgroups_per_dimension;
    if job.workgroups.iter().any(|&n| n > per_dim) {
        return Err(format!("gpu.compute: workgroups exceed this device's limit ({per_dim} per dimension)"));
    }
    let max_binding = (limits.max_storage_buffer_binding_size as u64).min(limits.max_buffer_size);
    // 入力は create_buffer_init が 4 バイト境界に切り上げる(空なら 4 バイトの 0)。
    let input_size = (job.input.len() as u64).max(4).div_ceil(4) * 4;
    if input_size > max_binding || job.output_size > max_binding {
        return Err(format!("gpu.compute: buffer exceeds this device's storage binding limit ({max_binding} bytes)"));
    }
    Ok(())
}

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    limits: wgpu::Limits,
    /// 1 度に 1 ジョブ。wgpu 22 のエラースコープはデバイスに 1 本の積み重ねで
    /// スレッドごとではないので、並べて走らせると他人のスコープを pop する。
    lock: std::sync::Mutex<()>,
}

static GPU: std::sync::OnceLock<Option<GpuCtx>> = std::sync::OnceLock::new();

/// タイムアウト・デバイス喪失・wgpu の panic のあと、このプロセスでは GPU を使わない。
static GPU_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// エラースコープの外で起きた wgpu のエラー(差し替えた uncaptured-error ハンドラが立てる)。
static GPU_UNCAPTURED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn gpu_ctx() -> Option<&'static GpuCtx> {
    // アダプター / デバイスの取得でドライバーが panic しても「GPU 無し」にする。
    GPU.get_or_init(|| std::panic::catch_unwind(gpu_init).ok().flatten()).as_ref()
}

fn gpu_init() -> Option<GpuCtx> {
    use std::sync::atomic::Ordering;
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()?;
    // 既定のハンドラは panic する。拾って E01 にするための旗だけ立てる。
    device.on_uncaptured_error(Box::new(|_e| GPU_UNCAPTURED.store(true, Ordering::SeqCst)));
    device.set_device_lost_callback(|_reason, _message| GPU_DISABLED.store(true, Ordering::SeqCst));
    let limits = device.limits();
    Some(GpuCtx { device, queue, limits, lock: std::sync::Mutex::new(()) })
}

/// `permitted` = このインスタンスに `gpu_ml` が与えられているか(`HostState::gpu_compute`)。
fn gpu_compute_response(req: &serde_json::Value, permitted: bool) -> serde_json::Value {
    use base64::Engine as _;
    let call_id = req["callId"].clone();
    let err = |code: &str, message: String| {
        serde_json::json!({"callId": call_id, "ok": false, "error": {"code": code, "message": message}})
    };
    // 権限の無いプラグインには GPU の初期化もさせない。
    if !permitted {
        return err("E01", "gpu.compute: permission denied (gpu_ml is not granted to this plugin)".to_string());
    }
    let job = match parse_gpu_args(&req["args"]) {
        Ok(job) => job,
        Err(message) => return err("D01", message),
    };
    let Some(ctx) = gpu_ctx() else {
        return err("E01", "no gpu adapter — fall back to the cloud calculator (ml.infer)".to_string());
    };
    if let Err(message) = check_gpu_limits(&job, &ctx.limits) {
        return err("D01", message);
    }
    match run_gpu_job(ctx, &job, Duration::from_millis(GPU_TIMEOUT_MS)) {
        Ok(output) => {
            let output = base64::engine::general_purpose::STANDARD.encode(output);
            serde_json::json!({"callId": call_id, "ok": true, "value": {"output": output, "placement": "device"}})
        }
        Err(message) => err("E01", message),
    }
}

/// 直列化・無効化の確認・panic の捕捉をして [`gpu_dispatch`] を回す。
fn run_gpu_job(ctx: &GpuCtx, job: &GpuJob, timeout: Duration) -> Result<Vec<u8>, String> {
    use std::sync::atomic::Ordering;
    const DISABLED: &str = "gpu.compute: gpu disabled for this process after a timeout / device loss — fall back to the cloud calculator (ml.infer)";
    let deadline = std::time::Instant::now() + timeout;
    // 前のジョブを待つ時間も上限に含める(std の Mutex には待ちの上限が無いので try_lock で回す)。
    let _guard = loop {
        match ctx.lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!("gpu.compute: timed out waiting for the gpu ({} ms)", timeout.as_millis()));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    };
    if GPU_DISABLED.load(Ordering::SeqCst) {
        return Err(DISABLED.to_string());
    }
    GPU_UNCAPTURED.store(false, Ordering::SeqCst);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu_dispatch(ctx, job, deadline))) {
        Ok(Err(GpuFailure::TimedOut)) => {
            GPU_DISABLED.store(true, Ordering::SeqCst);
            Err(format!("gpu.compute: timed out after {} ms; gpu disabled for this process", timeout.as_millis()))
        }
        Ok(Err(GpuFailure::Error(message))) => Err(message),
        Ok(Ok(_)) if GPU_UNCAPTURED.swap(false, Ordering::SeqCst) => Err("gpu.compute: gpu reported an error".to_string()),
        Ok(Ok(output)) => Ok(output),
        Err(_) => {
            GPU_DISABLED.store(true, Ordering::SeqCst);
            Err(DISABLED.to_string())
        }
    }
}

enum GpuFailure {
    TimedOut,
    Error(String),
}

fn gpu_dispatch(ctx: &GpuCtx, job: &GpuJob, deadline: std::time::Instant) -> Result<Vec<u8>, GpuFailure> {
    use wgpu::util::DeviceExt as _;
    let device = &ctx.device;
    // 検証エラー(WGSL の文法・バインド不一致・entry 違い)と確保の失敗はエラースコープで拾う。
    // スコープは encode の終わりまで開けておき、submit の**前に**閉じる。
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rasq gpu.compute"),
        source: wgpu::ShaderSource::Wgsl(job.wgsl.as_str().into()),
    });
    let in_bytes: &[u8] = if job.input.is_empty() { &[0, 0, 0, 0] } else { &job.input };
    let in_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: in_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: job.output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: job.output_size,
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
        entry_point: &job.entry,
        compilation_options: Default::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        let [wx, wy, wz] = job.workgroups;
        pass.dispatch_workgroups(wx, wy, wz);
    }
    encoder.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, job.output_size);
    let commands = encoder.finish();
    let validation = pollster::block_on(device.pop_error_scope());
    let out_of_memory = pollster::block_on(device.pop_error_scope());
    if let Some(e) = validation.or(out_of_memory) {
        return Err(GpuFailure::Error(format!("gpu.compute: validation failed: {e}")));
    }
    ctx.queue.submit([commands]);

    let slice = read_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    // `Maintain::Wait` は上限なしに待つので、Poll を回して締め切りで切る。
    loop {
        device.poll(wgpu::Maintain::Poll);
        match rx.try_recv() {
            Ok(Ok(())) => break,
            Ok(Err(e)) => return Err(GpuFailure::Error(format!("gpu.compute: readback failed: {e}"))),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(GpuFailure::Error("gpu.compute: readback failed: callback dropped".to_string()))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err(GpuFailure::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let output = slice.get_mapped_range().to_vec();
    read_buf.unmap();
    Ok(output)
}

/// FNV-1a (64 bit) を 16 桁の hex に。重複の判定に使うだけで、衝突耐性は要らない
/// (形そのものも一緒に送る)。埋込側の Web ワーカー (JS) も同じ計算をする。
fn fnv1a64(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// パスの区画を型に落とす。値(id / ユーザー名 / ハッシュ)を残さない。
fn path_segment_kind(seg: &str) -> String {
    let s = seg;
    if s.is_empty() {
        return String::new();
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return "{n}".into();
    }
    if s.len() == 36 && s.bytes().filter(|b| *b == b'-').count() == 4 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return "{uuid}".into();
    }
    if s.len() >= 16 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return "{hex}".into();
    }
    if s.starts_with('@') || s.contains('@') {
        return "{acct}".into();
    }
    if s.len() > 24 || s.bytes().any(|b| !(b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')) {
        return "{s}".into();
    }
    // 短い英数字(API の名前: notes, timelines, v1, show.json)はそのまま。
    s.to_ascii_lowercase()
}

/// JSON の骨組み: キーは残し、値は型だけにする。配列は先頭の要素の骨組み。深さ 4 まで。
fn json_skeleton(v: &serde_json::Value, depth: usize) -> serde_json::Value {
    use serde_json::Value;
    if depth >= 4 {
        return Value::String("…".into());
    }
    match v {
        Value::Object(o) => {
            let mut out = serde_json::Map::new();
            for (k, x) in o.iter().take(64) {
                out.insert(k.clone(), json_skeleton(x, depth + 1));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.first().map(|x| vec![json_skeleton(x, depth + 1)]).unwrap_or_default()),
        Value::String(_) => Value::String("s".into()),
        Value::Number(_) => Value::String("n".into()),
        Value::Bool(_) => Value::String("b".into()),
        Value::Null => Value::Null,
    }
}

/// `net.fetch` の引数から**値を持たない形**を取る: host / method / path のテンプレート /
/// query のキー / ヘッダ名 / JSON 本文のキー構造。戻りは (hash, shape)。
///
/// 値(トークン、本文、ユーザー名)はここで落ちるので、端末の外には出ない。
/// 審査 (rasq-judge) は、承認済みの版に初めて現れた形があれば、その版を読み直す。
fn fetch_shape(args: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    use serde_json::Value;
    let url = args["url"].as_str()?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let host = url_host(url);
    let rest = url.split_once("://").map_or("", |x| x.1);
    let after_authority = rest.find('/').map(|i| &rest[i..]).unwrap_or("/");
    let (path, query) = match after_authority.split_once('?') {
        Some((p, q)) => (p, q.split('#').next().unwrap_or("")),
        None => (after_authority.split('#').next().unwrap_or("/"), ""),
    };
    let template: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(path_segment_kind).collect();
    let mut query_keys: Vec<String> = query.split('&').filter(|s| !s.is_empty()).map(|kv| kv.split('=').next().unwrap_or("").to_ascii_lowercase()).collect();
    query_keys.sort();
    query_keys.dedup();
    let mut headers: Vec<String> = args["headers"].as_object().map(|h| h.keys().map(|k| k.to_ascii_lowercase()).collect()).unwrap_or_default();
    headers.sort();
    headers.dedup();
    let body = match &args["body"] {
        Value::Null => Value::String("none".into()),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(j) if j.is_object() || j.is_array() => json_skeleton(&j, 0),
            _ => Value::String("text".into()),
        },
        Value::Object(o) if o.get("b64").is_some_and(Value::is_string) => Value::String("bytes".into()),
        other => json_skeleton(other, 0),
    };
    let shape = serde_json::json!({
        "host": host,
        "method": args["method"].as_str().unwrap_or("GET").to_ascii_uppercase(),
        "path": format!("/{}", template.join("/")),
        "query": query_keys,
        "headers": headers,
        "body": body,
    });
    // serde_json の Map は既定で BTreeMap = キー順が安定なので、to_string が正規形になる。
    let canonical = shape.to_string();
    Some((fnv1a64(&canonical), shape))
}

/// URL のホスト部(小文字、userinfo とポートは落とす)。スキーム付きの URL だけ渡すこと。
///
/// 切り出しは、実際に接続する ureq (WHATWG の `url` クレート) と同じ区切りで行う。特に `\` は
/// http(s) では `/` と同じ authority の終わりで、ここを見落とすと `https://evil.example\@good.example/`
/// が「userinfo 付きの good.example」に見えて許可を素通りする (`url` は evil.example に繋ぐ)。
fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or("", |x| x.1);
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    // IPv6 リテラル `[::1]:8080` はポートの ':' と区別がつかないので、括弧を先に見る。
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    host.to_ascii_lowercase()
}

/// `host` が許可パターンのどれかに当たるか。
///
/// パターンは 3 種類だけ(埋込側の Dart と同じ規則。`plugin_runtime` の
/// `fetchHostPermitted` を変えるならこちらも変えること):
/// - `example.com`   … 完全一致
/// - `*.example.com` … サブドメイン(1 段以上)。apex は別に書く
/// - `*`             … どこでも。開発用の面だけが使う値で、審査では旗になる
fn host_permitted(host: &str, patterns: &[String]) -> bool {
    if host.is_empty() {
        return false;
    }
    patterns.iter().any(|p| {
        let p = p.trim();
        if p == "*" {
            true
        } else if let Some(suffix) = p.strip_prefix("*.") {
            host.len() > suffix.len() + 1 && host.ends_with(suffix) && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
        } else {
            host == p
        }
    })
}

/// Serves one `net.fetch` request with blocking ureq (host-fn schema:
/// args {method, url, headers?, body?} → value {status, headers, bodyText}).
/// ネイティブ `net.fetch` を 1 回。`body_budget` = この呼び出しでまだ読んでよい本文のバイト数。戻りの 2 つ目は読んだ本文のバイト数。
fn native_fetch_with_budget(req: &serde_json::Value, timeout_ms: u64, hosts: &[String], body_budget: u64) -> (serde_json::Value, u64) {
    let mut read = 0u64;
    let v = native_fetch_response(req, timeout_ms, hosts, body_budget, &mut read);
    (v, read)
}

fn native_fetch_response(req: &serde_json::Value, timeout_ms: u64, hosts: &[String], body_budget: u64, read: &mut u64) -> serde_json::Value {
    let call_id = req["callId"].clone();
    let args = &req["args"];
    let method = args["method"].as_str().unwrap_or("GET").to_uppercase();
    let url = args["url"].as_str().unwrap_or("");
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "B03", "message": format!("bad url: {url}")}});
    }
    // 宣言したホストだけ (D-004 の続き)。ここで止めないと、`network` 権限が
    // 「どこへでも」の意味になり、審査で接続先を見る意味が無くなる。
    let host = url_host(url);
    if !host_permitted(&host, hosts) {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "E01", "message": format!("host not permitted: {host}")}});
    }
    // 壁時計の締め: 全体 (timeout) に加えて、接続・read・write それぞれにも
    // 上限を入れる。全体だけだと「1 バイトずつ延々と送ってくる」相手に上限
    // いっぱい付き合うことになり、呼び出しスレッドがその間ずっと死ぬ。
    let overall = Duration::from_millis(timeout_ms.max(1));
    let agent = ureq::builder()
        .timeout_connect(overall.min(Duration::from_millis(FETCH_CONNECT_TIMEOUT_MS)))
        .timeout_read(overall.min(Duration::from_millis(FETCH_IO_TIMEOUT_MS)))
        .timeout_write(overall.min(Duration::from_millis(FETCH_IO_TIMEOUT_MS)))
        .timeout(overall)
        // リダイレクトは追わない: 許可したホストが 302 で別のホストへ飛ばせば、
        // 上の検査を素通りする。3xx はそのまま返し、飛ぶかどうかはプラグインが
        // 次の `net.fetch` で決める(その URL もまた検査される)。
        .redirects(0)
        .build();
    let mut request = agent.request(&method, url);
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
    // 本文はゲストではなくホストの RAM に載るので、ゲストのメモリ上限とは
    // 別に絞る。`take` で読み取り自体を打ち切り、+1 バイト読めたら超過。
    let mut body = Vec::new();
    // 1 回の上限と、呼び出しの残りの予算の小さいほう(+1 バイト読めたら超過)。
    let limit = MAX_FETCH_BODY_BYTES.min(body_budget);
    let mut reader = std::io::Read::take(http_response.into_reader(), limit + 1);
    let res = std::io::Read::read_to_end(&mut reader, &mut body);
    *read = body.len() as u64;
    if let Err(e) = res {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "B03", "message": format!("read body failed: {e}")}});
    }
    if body.len() as u64 > limit && limit < MAX_FETCH_BODY_BYTES {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "E02", "message": format!("network budget exhausted ({MAX_FETCH_BYTES_PER_CALL} response bytes per call)")}});
    }
    if body.len() as u64 > MAX_FETCH_BODY_BYTES {
        return serde_json::json!({"callId": call_id, "ok": false, "error": {"code": "B03", "message": format!("response body exceeds {MAX_FETCH_BODY_BYTES} bytes")}});
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
        .unwrap_or_else(|_| c"{\"ok\":false,\"error\":{\"code\":\"D01\",\"message\":\"nul in output\"}}".to_owned())
        .into_raw()
}

/// 枯渇系(E02/E03)は `Exx 規制`(CONTRACT §9)に置く — ホストが線を引いて
/// 止めた、という意味で、プラグイン間受け渡しの失敗 (C02) とは別物。
/// 埋込側はこのコードだけ見て「暴走したプラグイン」を落として再審査へ
/// 回せる。
fn serde_error_json(e: &RasqWasmError) -> String {
    let (code, message) = match e {
        RasqWasmError::Wasmi(err) => ("C02", err.to_string()),
        RasqWasmError::MissingExport(name) => ("C02", format!("missing export: {name}")),
        RasqWasmError::Utf8 => ("C02", "wasm returned non-utf8 bytes".to_string()),
        RasqWasmError::CallBudget(m) => ("E02", m.clone()),
        RasqWasmError::FuelExhausted => (
            "E02",
            format!(
                "plugin exceeded its execution budget ({FUEL_PER_CALL} fuel per call); \
                 if this is a legitimate media workload, downscale the source image \
                 before media.edit or raise the budget with rasq_wasm_set_fuel_per_call"
            ),
        ),
        RasqWasmError::MemoryLimit => (
            "E03",
            format!("plugin exceeded its memory limit ({MAX_MEMORY_BYTES} bytes)"),
        ),
    };
    format!("{{\"ok\":false,\"error\":{{\"code\":\"{code}\",\"message\":{message:?}}}}}")
}

// ---- zstd(.rspkg の展開、D-034)------------------------------------------
// ホスト(Dart)には zstd の復号器が無いので、ここに 1 関数だけ持つ。出力は `max_out` バイトで打ち切る
// (小さな入力から巨大な出力を作る展開爆弾の対策)。

/// zstd を展開する。失敗(壊れた入力 / `max_out` 超え)は Err。
pub fn zstd_decompress(input: &[u8], max_out: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut dec = ruzstd::decoding::StreamingDecoder::new(input).map_err(|e| format!("zstd header: {e}"))?;
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = dec.read(&mut buf).map_err(|e| format!("zstd: {e}"))?;
        if n == 0 {
            return Ok(out);
        }
        if out.len() + n > max_out {
            return Err(format!("zstd: output exceeds {max_out} bytes"));
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Decompresses a zstd frame. Returns a buffer owned by the caller (free it
/// with `rasq_bytes_free(ptr, *out_len)`), or null on failure: corrupt input,
/// or an output larger than `max_out` bytes. `*out_len` is set on success.
#[no_mangle]
pub extern "C" fn rasq_zstd_decompress(bytes_ptr: *const u8, bytes_len: usize, max_out: usize, out_len: *mut usize) -> *mut u8 {
    if bytes_ptr.is_null() || out_len.is_null() {
        return std::ptr::null_mut();
    }
    let input = unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) };
    match zstd_decompress(input, max_out) {
        Ok(out) => {
            let boxed = out.into_boxed_slice();
            unsafe { *out_len = boxed.len() };
            Box::into_raw(boxed) as *mut u8
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// zstd で圧縮する(ruzstd の `Fastest` = zstd のレベル 1 相当)。契約 v0.23 の落書きの `svgz`(rasq-client D-472):
/// 編集画面が返した SVG を小さくしてサーバー変数に置く。入力は呼ぶ側が小さく切る(SVG は 64 KiB まで)。
pub fn zstd_compress(input: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(input, ruzstd::encoding::CompressionLevel::Fastest)
}

/// Compresses `bytes` into one zstd frame (ruzstd `Fastest`, about zstd level 1).
/// Returns a buffer owned by the caller (free it with
/// `rasq_bytes_free(ptr, *out_len)`), or null when a pointer is null.
#[no_mangle]
pub extern "C" fn rasq_zstd_compress(bytes_ptr: *const u8, bytes_len: usize, out_len: *mut usize) -> *mut u8 {
    if out_len.is_null() || (bytes_ptr.is_null() && bytes_len != 0) {
        return std::ptr::null_mut();
    }
    let input = if bytes_len == 0 { &[][..] } else { unsafe { slice::from_raw_parts(bytes_ptr, bytes_len) } };
    let boxed = zstd_compress(input).into_boxed_slice();
    unsafe { *out_len = boxed.len() };
    Box::into_raw(boxed) as *mut u8
}

/// Frees a buffer returned by `rasq_zstd_decompress` / `rasq_zstd_compress`.
#[no_mangle]
pub extern "C" fn rasq_bytes_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len))) };
    }
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

/// Replaces the set of hosts the runtime-native `net.fetch` may reach:
/// a comma-separated list (`misskey.io,*.example.com`), lower-cased on the
/// way in. `*` means any host. NULL or empty means **no host at all** —
/// the runtime starts closed, so an embedder that never calls this cannot
/// accidentally hand a plugin the whole internet. Call again whenever the
/// session's instance host changes.
#[no_mangle]
pub extern "C" fn rasq_wasm_set_fetch_hosts(instance: *mut RasqWasmInstance, hosts: *const c_char) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *instance };
    let csv = if hosts.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(hosts) }.to_string_lossy().into_owned()
    };
    instance.store.data_mut().fetch_hosts = csv
        .split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
}

/// Grants or revokes `gpu.compute` (D-081) for this instance. Pass `true`
/// only when the plugin's manifest declares the `gpu_ml` permission and the
/// store review approved it. `gpu.compute` is served inside the runtime, so
/// the embedder's own host-function permission checks never see it — this
/// flag is the gate. A fresh instance starts **closed**: without this call
/// every `gpu.compute` answers `E01` (permission denied), so an embedder
/// that forgets it cannot hand every plugin the GPU by omission.
#[no_mangle]
pub extern "C" fn rasq_wasm_set_gpu_compute(instance: *mut RasqWasmInstance, enabled: bool) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *instance };
    instance.store.data_mut().gpu_compute = enabled;
}

/// Takes the `net.fetch` request shapes this instance has seen for the first
/// time since the previous call: a JSON array of `{hash, shape}` (or a single
/// `{overflow: true}` once the per-instance cap is exceeded). Shapes carry no
/// values — host, method, a path template, query keys, header names and the
/// key skeleton of a JSON body. Free with `rasq_wasm_free_string`.
#[no_mangle]
pub extern "C" fn rasq_wasm_take_fetch_shapes(instance: *mut RasqWasmInstance) -> *mut c_char {
    if instance.is_null() {
        return std::ptr::null_mut();
    }
    let instance = unsafe { &mut *instance };
    let taken = std::mem::take(&mut instance.store.data_mut().fetch_shapes_new);
    CString::new(serde_json::Value::Array(taken).to_string())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Overrides the per-invocation fuel budget (default [`FUEL_PER_CALL`]).
///
/// Fuel is what stops a runaway plugin: every `call`/`snapshot`/`patch` is
/// refuelled to this amount, and a guest that burns through it traps with
/// `E02` instead of wedging the calling thread. Lower it for untrusted /
/// sideloaded plugins, raise it for a reviewed plugin that legitimately
/// crunches (`tl.sort` over a huge timeline). `0` is accepted but means
/// "every call fails immediately" — use it only to hard-stop a plugin.
#[no_mangle]
pub extern "C" fn rasq_wasm_set_fuel_per_call(instance: *mut RasqWasmInstance, fuel: u64) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *instance };
    instance.fuel_per_call = fuel;
}

/// Recommended fuel budget for a plugin that processes bitmaps — one that
/// takes the `composer.media` slot, or declares `replace_media`/`tag_media`.
/// This is also the default a fresh instance starts with, so an embedder that
/// never calls the setter is already correct for the heavy case.
///
/// Note the axis: **not** the manifest `kind`. The heaviest sample plugin
/// (`sample.media-process`, 2.1 G fuel for one `media.edit`) declares
/// `kind: "action"`, while `mediaViewer` is the lightest thing measured
/// (118 K) — `kind` says where a plugin plugs in, not what it costs.
#[no_mangle]
pub extern "C" fn rasq_wasm_fuel_budget_media() -> u64 {
    FUEL_BUDGET_MEDIA
}

/// Recommended fuel budget for every other plugin (sns / trend / view /
/// post-processing / plain actions): 24x the heaviest non-bitmap workload
/// measured, and tight enough that a runaway is stopped in well under a
/// second. Apply it with `rasq_wasm_set_fuel_per_call` once the manifest says
/// the plugin does not touch media.
#[no_mangle]
pub extern "C" fn rasq_wasm_fuel_budget_standard() -> u64 {
    FUEL_BUDGET_STANDARD
}

/// Overrides the wall-clock budget of one runtime-native `net.fetch`
/// (default [`DEFAULT_FETCH_TIMEOUT_MS`] ms). Only meaningful with
/// `rasq_wasm_set_native_fetch(.., true)`; when the embedder serves
/// `net.fetch` itself, bounding that call is the embedder's job — fuel does
/// not tick while a host function blocks.
#[no_mangle]
pub extern "C" fn rasq_wasm_set_fetch_timeout_ms(instance: *mut RasqWasmInstance, timeout_ms: u64) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *instance };
    instance.store.data_mut().fetch_timeout_ms = timeout_ms;
}

/// Overrides the wall-clock budget of one guest invocation (`call` /
/// `snapshot` / `patch`, default [`DEFAULT_CALL_WALL_MS`] ms). Every host call
/// (runtime-native `net.fetch` or the embedder callback) checks the remaining
/// time first, and a native fetch never waits longer than what is left — so
/// repeated slow fetches cannot hold the calling thread past this budget (E02).
#[no_mangle]
pub extern "C" fn rasq_wasm_set_call_timeout_ms(instance: *mut RasqWasmInstance, timeout_ms: u64) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *instance };
    instance.store.data_mut().call_wall_ms = timeout_ms;
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

    /// テストの相手 = misskey-wasm-sample の wasm。まだ無ければ一度だけ作る(きれいな checkout で `cargo test` しても落ちない)。
    fn fixture() -> &'static [u8] {
        static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        BYTES.get_or_init(|| {
            if std::fs::metadata(WASM_PATH).is_err() {
                let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
                // nosemgrep: rasq-command-spawn — テストだけ。cargo は cargo test が渡す $CARGO(無ければ "cargo")、引数と作業ディレクトリは固定で、外の文字列は入らない
                let status = std::process::Command::new(cargo)
                    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
                    .args(["build", "--release", "--target", "wasm32-unknown-unknown", "-p", "misskey-wasm-sample"])
                    .status()
                    .expect("run cargo to build misskey-wasm-sample");
                assert!(
                    status.success(),
                    "building misskey-wasm-sample failed (is the wasm32-unknown-unknown target installed? `rustup target add wasm32-unknown-unknown`)"
                );
            }
            std::fs::read(WASM_PATH).expect("misskey_wasm_sample.wasm")
        })
    }

    fn load() -> RasqWasmInstance {
        RasqWasmInstance::load_plain(fixture()).expect("instantiate")
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
        let bytes = fixture().to_vec();
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
        let bytes = fixture().to_vec();
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

        // 通信の形: 埋込側の経路 (callback) でも控えられる。値は入らない。
        let shapes = instance.take_fetch_shapes();
        assert_eq!(shapes.len(), 1, "{shapes:?}");
        assert_eq!(shapes[0]["shape"]["host"], "misskey.example");
        assert_eq!(shapes[0]["shape"]["method"], "POST");
        assert_eq!(shapes[0]["hash"].as_str().unwrap().len(), 16);
        let s = shapes[0].to_string();
        assert!(!s.contains("limit\":10") && !s.contains("\"i\":"), "値が漏れている: {s}");
        // 2 回目は既知なので何も出ない。
        instance.call("timeline", r#"{"host":"misskey.example","limit":10}"#, "call-t2").expect("call");
        assert!(instance.take_fetch_shapes().is_empty());
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

        let bytes = fixture().to_vec();
        // No callback at all: net.fetch must be answered natively.
        let mut instance = RasqWasmInstance::load_plain(&bytes).expect("instantiate");
        instance.set_native_fetch(true);
        instance.set_fetch_hosts(&["127.0.0.1"]);
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
    fn zstd_compress_round_trips_and_shrinks_svg() {
        // 落書きの SVG(契約 v0.23 の svgz、rasq-client D-472): 線の座標が並ぶので、よく縮む。
        let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 400">"#);
        for i in 0..120u32 {
            svg.push_str(&format!(
                r##"<path d="M{} {}L{} {} {} {} {} {}" fill="none" stroke="#1D2939" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>"##,
                i % 300,
                (i * 7) % 400,
                (i * 3) % 300,
                (i * 11) % 400,
                (i * 5) % 300,
                (i * 13) % 400,
                (i * 17) % 300,
                (i * 19) % 400
            ));
        }
        svg.push_str("</svg>");
        let packed = zstd_compress(svg.as_bytes());
        assert!(packed.len() * 3 < svg.len(), "{} -> {}", svg.len(), packed.len());
        assert_eq!(zstd_decompress(&packed, svg.len()).unwrap(), svg.as_bytes());
        // 参照の実装(zstd)でも読める
        assert_eq!(zstd::decode_all(&packed[..]).unwrap(), svg.as_bytes());
        // 空の入力も 1 フレーム
        assert!(zstd_decompress(&zstd_compress(b""), 16).unwrap().is_empty());

        // C ABI: 長さが返り、解放できる。null は null
        let mut n: usize = 0;
        let p = rasq_zstd_compress(svg.as_ptr(), svg.len(), &mut n);
        assert!(!p.is_null());
        assert_eq!(zstd_decompress(unsafe { slice::from_raw_parts(p, n) }, svg.len()).unwrap(), svg.as_bytes());
        rasq_bytes_free(p, n);
        assert!(rasq_zstd_compress(std::ptr::null(), 3, &mut n).is_null());
        assert!(rasq_zstd_compress(svg.as_ptr(), svg.len(), std::ptr::null_mut()).is_null());
    }

    #[test]
    fn zstd_round_trip_limit_and_garbage() {
        let raw: Vec<u8> = (0..200_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
        let packed = zstd::encode_all(&raw[..], 19).unwrap();
        assert_eq!(zstd_decompress(&packed, raw.len()).unwrap(), raw);
        // 上限を 1 バイトでも超えたら失敗(展開爆弾の対策)
        assert!(zstd_decompress(&packed, raw.len() - 1).unwrap_err().contains("exceeds"));
        // zstd でない入力 / 途中で切れた入力
        assert!(zstd_decompress(b"not zstd at all", 1 << 20).is_err());
        assert!(zstd_decompress(&packed[..packed.len() / 2], 1 << 30).is_err());
        // 1 バイト化けた入力(フレームのチェックサム)
        let mut bad = packed.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 0x55;
        assert!(zstd_decompress(&bad, 1 << 30).is_err() || zstd_decompress(&bad, 1 << 30).unwrap() != raw);

        // C ABI: 長さが返り、解放できる。失敗は null
        let mut n: usize = 0;
        let p = rasq_zstd_decompress(packed.as_ptr(), packed.len(), raw.len(), &mut n);
        assert!(!p.is_null());
        assert_eq!(unsafe { slice::from_raw_parts(p, n) }, &raw[..]);
        rasq_bytes_free(p, n);
        assert!(rasq_zstd_decompress(packed.as_ptr(), packed.len(), 10, &mut n).is_null());
        assert!(rasq_zstd_decompress(b"xx".as_ptr(), 2, 100, &mut n).is_null());
    }

    #[test]
    fn native_fetch_is_closed_until_hosts_are_declared() {
        // 埋込側がホストを渡し忘れた = 全開、ではなく全閉 (fail-closed)。
        // 接続先が無いので TCP は開かない(listener を立てないことで確かめる)。
        let bytes = fixture().to_vec();
        let mut instance = RasqWasmInstance::load_plain(&bytes).expect("instantiate");
        instance.set_native_fetch(true);
        let result = instance
            .call("timeline", r#"{"host":"native.example","baseUrl":"http://127.0.0.1:9","limit":5}"#, "call-closed")
            .expect("call");
        assert!(result.contains("E01"), "{result}");
        assert!(result.contains("host not permitted"), "{result}");
    }

    #[test]
    fn native_fetch_rejects_a_host_outside_the_declaration() {
        let bytes = fixture().to_vec();
        let mut instance = RasqWasmInstance::load_plain(&bytes).expect("instantiate");
        instance.set_native_fetch(true);
        instance.set_fetch_hosts(&["misskey.io", "*.example.net"]);
        let result = instance
            .call("timeline", r#"{"host":"native.example","baseUrl":"http://127.0.0.1:9","limit":5}"#, "call-other")
            .expect("call");
        assert!(result.contains("E01"), "{result}");
    }

    #[test]
    fn fetch_shape_drops_values_and_keeps_structure() {
        let args = serde_json::json!({
            "method": "post",
            "url": "https://Misskey.IO/api/users/9abcdef012345678/notes?limit=10&sinceId=abc#frag",
            "headers": {"Content-Type": "application/json", "Authorization": "Bearer SECRET-TOKEN"},
            "body": serde_json::json!({"i": "SECRET", "limit": 10, "tags": ["a", "b"], "opts": {"withFiles": true}}).to_string(),
        });
        let (hash, shape) = fetch_shape(&args).unwrap();
        assert_eq!(hash.len(), 16);
        assert_eq!(shape["host"], "misskey.io");
        assert_eq!(shape["method"], "POST");
        assert_eq!(shape["path"], "/api/users/{hex}/notes");
        assert_eq!(shape["query"], serde_json::json!(["limit", "sinceid"]));
        assert_eq!(shape["headers"], serde_json::json!(["authorization", "content-type"]));
        assert_eq!(shape["body"], serde_json::json!({"i": "s", "limit": "n", "opts": {"withFiles": "b"}, "tags": ["s"]}));
        let s = shape.to_string();
        assert!(!s.contains("SECRET") && !s.contains("9abcdef0"), "{s}");
        // 同じ形なら同じハッシュ、値が違っても同じ。
        let args2 = serde_json::json!({"method": "POST", "url": "https://misskey.io/api/users/00000000deadbeef/notes?sinceId=zzz&limit=5",
            "headers": {"authorization": "Bearer other", "content-type": "application/json"},
            "body": serde_json::json!({"i": "x", "limit": 1, "tags": ["z"], "opts": {"withFiles": false}}).to_string()});
        assert_eq!(fetch_shape(&args2).unwrap().0, hash);
        // 空の配列と中身のある配列は別の形(要素の骨組みを残すため。同じ口で 2 種類までにしかならない)。
        let args4 = serde_json::json!({"method": "POST", "url": "https://misskey.io/api/users/00000000deadbeef/notes?sinceId=zzz&limit=5",
            "headers": {"authorization": "Bearer other", "content-type": "application/json"},
            "body": serde_json::json!({"i": "x", "limit": 1, "tags": [], "opts": {"withFiles": false}}).to_string()});
        assert_ne!(fetch_shape(&args4).unwrap().0, hash);
        // ホストが違えば別の形。
        let args3 = serde_json::json!({"method": "POST", "url": "https://collect.example/api/users/00000000deadbeef/notes"});
        assert_ne!(fetch_shape(&args3).unwrap().0, hash);
        // 本文の種類。
        assert_eq!(fetch_shape(&serde_json::json!({"url": "https://a.test/x", "body": {"b64": "AAAA"}})).unwrap().1["body"], "bytes");
        assert_eq!(fetch_shape(&serde_json::json!({"url": "https://a.test/x", "body": "plain text"})).unwrap().1["body"], "text");
        assert_eq!(fetch_shape(&serde_json::json!({"url": "https://a.test/x"})).unwrap().1["body"], "none");
        assert_eq!(fetch_shape(&serde_json::json!({"url": "https://a.test/@alice/statuses/123456/reblog"})).unwrap().1["path"], "/{acct}/statuses/{n}/reblog");
        assert!(fetch_shape(&serde_json::json!({"url": "ftp://a.test/x"})).is_none());
    }

    #[test]
    fn host_patterns() {
        let pats = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(host_permitted("misskey.io", &pats(&["misskey.io"])));
        assert!(host_permitted("MISSKEY.IO".to_ascii_lowercase().as_str(), &pats(&["misskey.io"])));
        assert!(!host_permitted("misskey.io.evil.example", &pats(&["misskey.io"])));
        assert!(!host_permitted("evilmisskey.io", &pats(&["misskey.io"])));
        // ワイルドカードはサブドメインだけ。apex は別に書く。
        assert!(host_permitted("a.example.com", &pats(&["*.example.com"])));
        assert!(host_permitted("a.b.example.com", &pats(&["*.example.com"])));
        assert!(!host_permitted("example.com", &pats(&["*.example.com"])));
        assert!(!host_permitted("notexample.com", &pats(&["*.example.com"])));
        assert!(host_permitted("anything.test", &pats(&["*"])));
        assert!(!host_permitted("anything.test", &pats(&[])));
        assert!(!host_permitted("", &pats(&["*"])));
    }

    #[test]
    fn url_host_strips_userinfo_port_and_path() {
        assert_eq!(url_host("https://Misskey.IO/api/notes"), "misskey.io");
        assert_eq!(url_host("http://127.0.0.1:8080/x?y#z"), "127.0.0.1");
        assert_eq!(url_host("https://user:pw@example.com:443/"), "example.com");
        assert_eq!(url_host("http://[::1]:8080/"), "::1");
        assert_eq!(url_host("https://evil.example/?u=@good.example"), "evil.example");
        assert_eq!(url_host("https://"), "");
        // `\` は http(s) では `/` と同じ (WHATWG URL)。`url` クレートは evil.example に繋ぐので、こちらもそう読む。
        assert_eq!(url_host("https://evil.example\\@good.example/api"), "evil.example");
        assert_eq!(url_host("https://good.example\\.evil.example/"), "good.example");
    }

    #[test]
    fn guest_length_is_bounds_checked_before_allocation() {
        // ゲストが返す packed の長さは自由に決められる。範囲外なら確保せずに C02 で返る。
        const BOGUS_LEN_WAT: &str = r#"
            (module
              (memory (export "memory") 1)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "dealloc") (param i32 i32))
              (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i64)
                (i64.const 0xFFFFFFFF)))
        "#;
        let mut instance = RasqWasmInstance::load_plain(BOGUS_LEN_WAT.as_bytes()).expect("instantiate");
        let err = instance.call("x", "{}", "call-bogus").expect_err("out-of-bounds length must fail");
        let json = serde_error_json(&err);
        assert!(json.contains("out of bounds"), "{json}");
        assert!(json.contains(r#""code":"C02""#), "{json}");
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


    // ---- gpu.compute ------------------------------------------------------
    //
    // 権限の門と args の検査は wgpu に触る前に終わるので、GPU の無い CI でも
    // 回る。実際に GPU で走らせるものは最後の #[ignore] のテストだけ。

    const GPU_WGSL: &str = "@group(0) @binding(0) var<storage, read> src: array<u32>;\n\
        @group(0) @binding(1) var<storage, read_write> dst: array<u32>;\n\
        @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) { dst[id.x] = src[id.x] * 2u; }";

    fn gpu_req(args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"schema": "rasq.hostfn/1", "callId": "g1", "fn": "gpu.compute", "args": args})
    }

    /// `call` のたびに host_fn へ固定の要求(data セグメント)を投げ、応答をそのまま返すゲスト。
    fn host_fn_guest_wat(request: &serde_json::Value) -> String {
        let json = request.to_string();
        let escaped: String = json.bytes().map(|b| format!("\\{b:02x}")).collect();
        format!(
            r#"
            (module
              (import "rasq" "host_fn" (func $host (param i32 i32) (result i64)))
              (memory (export "memory") 2)
              (data (i32.const 0) "{escaped}")
              (global $bump (mut i32) (i32.const 65536))
              (func (export "alloc") (param $n i32) (result i32) (local $p i32)
                (local.set $p (global.get $bump))
                (global.set $bump (i32.add (global.get $bump) (local.get $n)))
                (local.get $p))
              (func (export "dealloc") (param i32 i32))
              (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i64)
                (call $host (i32.const 0) (i32.const {len}))))
            "#,
            len = json.len()
        )
    }

    #[test]
    fn gpu_compute_is_closed_until_gpu_ml_is_granted() {
        // 埋込側が何も設定しない = GPU は使えない (fail-closed)。GPU の初期化にも進まない。
        let ok_args = serde_json::json!({"wgsl": GPU_WGSL, "input": "AQAAAAIAAAA=", "outputSize": 8, "workgroups": [2]});
        let wat = host_fn_guest_wat(&gpu_req(ok_args));
        let mut instance = RasqWasmInstance::load_plain(wat.as_bytes()).expect("instantiate");
        let result = instance.call("gpu", "{}", "call-gpu").expect("call");
        let v: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(v["ok"], false, "{result}");
        assert_eq!(v["error"]["code"], "E01", "{result}");
        assert!(result.contains("gpu_ml"), "{result}");

        // 開けても、検査に落ちる要求は wgpu に届く前に D01 で返る(GPU の有無に依らない)。
        let bad_args = serde_json::json!({"wgsl": GPU_WGSL, "outputSize": 6});
        let wat = host_fn_guest_wat(&gpu_req(bad_args));
        let mut instance = RasqWasmInstance::load_plain(wat.as_bytes()).expect("instantiate");
        instance.set_gpu_compute(true);
        let result = instance.call("gpu", "{}", "call-gpu-2").expect("call");
        let v: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(v["error"]["code"], "D01", "{result}");
        assert!(result.contains("multiple of 4"), "{result}");

        // C ABI の口でも閉じられる。
        let raw: *mut RasqWasmInstance = Box::into_raw(Box::new(instance));
        rasq_wasm_set_gpu_compute(raw, false);
        let mut instance = unsafe { Box::from_raw(raw) };
        let result = instance.call("gpu", "{}", "call-gpu-3").expect("call");
        assert!(result.contains("E01") && result.contains("gpu_ml"), "{result}");
    }

    #[test]
    fn gpu_compute_response_checks_permission_first() {
        let v = gpu_compute_response(&gpu_req(serde_json::json!({})), false);
        assert_eq!(v["error"]["code"], "E01");
        assert_eq!(v["callId"], "g1");
        // 権限があっても args が壊れていれば D01(アダプターを探しに行かない)。
        let v = gpu_compute_response(&gpu_req(serde_json::json!({"wgsl": ""})), true);
        assert_eq!(v["error"]["code"], "D01");
    }

    #[test]
    fn gpu_args_are_bounded_before_touching_wgpu() {
        use serde_json::json;
        let ok = |args: serde_json::Value| parse_gpu_args(&args).expect("valid");
        let bad = |args: serde_json::Value| parse_gpu_args(&args).expect_err("must be rejected");

        let job = ok(json!({"wgsl": GPU_WGSL, "input": "AQAAAAIAAAA=", "outputSize": 8, "workgroups": [2, 3]}));
        assert_eq!(job.entry, "main");
        assert_eq!(job.input, [1, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(job.workgroups, [2, 3, 1]);
        let job = ok(json!({"wgsl": GPU_WGSL, "entry": "run", "outputSize": MAX_GPU_OUTPUT_BYTES}));
        assert_eq!((job.entry.as_str(), job.workgroups, job.input.len()), ("run", [1, 1, 1], 0));
        ok(json!({"wgsl": GPU_WGSL, "outputSize": 4, "workgroups": [MAX_GPU_WORKGROUPS_PER_DIM, 16]}));

        // wgsl
        bad(json!({"outputSize": 4}));
        bad(json!({"wgsl": "", "outputSize": 4}));
        bad(json!({"wgsl": 1, "outputSize": 4}));
        assert!(bad(json!({"wgsl": "x".repeat(MAX_GPU_WGSL_BYTES + 1), "outputSize": 4})).contains("too large"));
        // entry
        bad(json!({"wgsl": GPU_WGSL, "entry": "", "outputSize": 4}));
        bad(json!({"wgsl": GPU_WGSL, "entry": 7, "outputSize": 4}));
        bad(json!({"wgsl": GPU_WGSL, "entry": "e".repeat(MAX_GPU_ENTRY_BYTES + 1), "outputSize": 4}));
        // outputSize: 0 / 4 の倍数でない (wgpu の検証エラー = 既定では panic) / 上限超え / 数でない
        for size in [json!(0), json!(3), json!(6), json!(MAX_GPU_OUTPUT_BYTES + 4), json!(-4), json!(4.5), json!("8"), json!(null)] {
            assert!(bad(json!({"wgsl": GPU_WGSL, "outputSize": size})).contains("outputSize"), "{size}");
        }
        // workgroups: 0 / 1 次元の上限超え (65536 は wgpu の検証エラー) / 負 / 小数 / 形違い / 積の上限超え
        for wg in [json!([0]), json!([MAX_GPU_WORKGROUPS_PER_DIM + 1]), json!([-1]), json!([1.5]), json!(["2"]), json!([]), json!([1, 1, 1, 1]), json!(8)] {
            assert!(bad(json!({"wgsl": GPU_WGSL, "outputSize": 4, "workgroups": wg})).contains("workgroups"), "{wg}");
        }
        assert!(bad(json!({"wgsl": GPU_WGSL, "outputSize": 4, "workgroups": [MAX_GPU_WORKGROUPS_PER_DIM, MAX_GPU_WORKGROUPS_PER_DIM]}))
            .contains("too many workgroups"));
        // input: base64 でない / 数 / デコード前の長さで切る (確保しない)
        bad(json!({"wgsl": GPU_WGSL, "outputSize": 4, "input": "***"}));
        bad(json!({"wgsl": GPU_WGSL, "outputSize": 4, "input": 12}));
        let huge = "A".repeat(MAX_GPU_INPUT_B64_LEN + 4);
        assert!(bad(json!({"wgsl": GPU_WGSL, "outputSize": 4, "input": huge})).contains("too large"));
        // 上限ちょうどは通る
        let edge = "A".repeat(MAX_GPU_INPUT_BYTES / 3 * 4);
        assert_eq!(ok(json!({"wgsl": GPU_WGSL, "outputSize": 4, "input": edge})).input.len(), MAX_GPU_INPUT_BYTES / 3 * 3);
    }

    #[test]
    fn gpu_args_are_also_bounded_by_the_device_limits() {
        let job = parse_gpu_args(&serde_json::json!({"wgsl": GPU_WGSL, "outputSize": 1024, "workgroups": [300]})).expect("valid");
        check_gpu_limits(&job, &wgpu::Limits::default()).expect("fits the default limits");
        // 下位のデバイス (1 次元 256 まで、binding 512 バイトまで)。
        let small = wgpu::Limits { max_compute_workgroups_per_dimension: 256, ..wgpu::Limits::default() };
        assert!(check_gpu_limits(&job, &small).unwrap_err().contains("workgroups"));
        let tiny = wgpu::Limits { max_storage_buffer_binding_size: 512, ..wgpu::Limits::default() };
        let job = parse_gpu_args(&serde_json::json!({"wgsl": GPU_WGSL, "outputSize": 1024})).expect("valid");
        assert!(check_gpu_limits(&job, &tiny).unwrap_err().contains("binding"));
        let tiny_buffer = wgpu::Limits { max_buffer_size: 512, ..wgpu::Limits::default() };
        assert!(check_gpu_limits(&job, &tiny_buffer).is_err());
    }

    /// 実 GPU で: 正しいシェーダーが動き、wgpu の検証エラーは panic ではなく E01 で返ること。
    /// アダプターが要るので既定では走らせない:
    ///     cargo test -p rasq_wasm_runtime_core -- --ignored gpu_compute_on_a_real_adapter
    #[test]
    #[ignore = "needs a gpu adapter"]
    fn gpu_compute_on_a_real_adapter() {
        if gpu_ctx().is_none() {
            eprintln!("skipped: no gpu adapter");
            return;
        }
        let run = |args: serde_json::Value| gpu_compute_response(&gpu_req(args), true);
        let v = run(serde_json::json!({"wgsl": GPU_WGSL, "input": "AQAAAAIAAAA=", "outputSize": 8, "workgroups": [2]}));
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["value"]["output"], "AgAAAAQAAAA=", "{v}");
        // WGSL の文法エラー / entry 違い = wgpu の検証エラー。ホストは落ちない。
        let v = run(serde_json::json!({"wgsl": "fn main( {", "outputSize": 4}));
        assert_eq!(v["error"]["code"], "E01", "{v}");
        let v = run(serde_json::json!({"wgsl": GPU_WGSL, "entry": "nope", "outputSize": 8}));
        assert_eq!(v["error"]["code"], "E01", "{v}");
        // 検査をすり抜けた値でも(ここでは GpuJob を直接組む)、submit の前のエラースコープで止まる。
        let ctx = gpu_ctx().expect("adapter");
        let over = ctx.limits.max_compute_workgroups_per_dimension + 1;
        let job = GpuJob { wgsl: GPU_WGSL.to_string(), entry: "main".into(), input: vec![0; 8], output_size: 8, workgroups: [over, 1, 1] };
        assert!(run_gpu_job(ctx, &job, Duration::from_secs(10)).is_err());
        let job = GpuJob { output_size: 6, workgroups: [1, 1, 1], ..job };
        assert!(run_gpu_job(ctx, &job, Duration::from_secs(10)).is_err());
        // その後も同じプロセスで使える(検証エラーでは無効化しない)。
        let v = run(serde_json::json!({"wgsl": GPU_WGSL, "input": "AQAAAAIAAAA=", "outputSize": 8, "workgroups": [2]}));
        assert_eq!(v["ok"], true, "{v}");
    }

    // ---- リソース上限 -----------------------------------------------------
    //
    // 実プラグイン (misskey-wasm-sample) は当然どちらの上限にも当たらないので、
    // 枯渇そのものは最小の WAT で作る。wasmi は `Module::new` に WAT を
    // そのまま食わせられる(`wat` feature が既定で入っている)ので、
    // フィクスチャを別途ビルドする必要はない。ABI は PROTOCOL §4.1 の
    // 最小形 — `memory` / `alloc` / `dealloc` / `call` だけ。

    /// `call` の中で `net.fetch` を `n` 回くり返すゲスト(要求の JSON は 4096 番地、応答は 8192 番地に置かせる)。
    fn fetch_loop_wat(port: u16, n: u32) -> String {
        let req = format!(r#"{{"fn":"net.fetch","callId":"c","args":{{"method":"GET","url":"http://127.0.0.1:{port}/slow"}}}}"#);
        format!(
            r#"(module
              (import "rasq" "host_fn" (func $host (param i32 i32) (result i64)))
              (memory (export "memory") 1)
              (data (i32.const 4096) "{data}")
              (func (export "alloc") (param i32) (result i32) (i32.const 8192))
              (func (export "dealloc") (param i32 i32))
              (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i64)
                (local $i i32)
                (loop $again
                  (drop (call $host (i32.const 4096) (i32.const {len})))
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br_if $again (i32.lt_u (local.get $i) (i32.const {n}))))
                (i64.const 0)))"#,
            data = req.replace('"', "\\\""),
            len = req.len(),
        )
    }

    /// 1 回ごとに `delay` 待ってから 200 を返す HTTP サーバー(テストの間だけ)。
    fn slow_server(delay: Duration) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    std::thread::sleep(delay);
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                });
            }
        });
        port
    }

    #[test]
    fn repeated_slow_fetches_stop_at_the_call_wall_clock_budget() {
        // 1 回 300 ms の通信を 50 回(以前は燃料がほとんど減らず 15 秒占有できた)。呼び出しの予算 1 秒で E02。
        let port = slow_server(Duration::from_millis(300));
        let mut instance = RasqWasmInstance::load_plain(fetch_loop_wat(port, 50).as_bytes()).expect("instantiate");
        instance.set_native_fetch(true);
        instance.set_fetch_hosts(&["127.0.0.1"]);
        rasq_wasm_set_call_timeout_ms(&mut instance, 1_000);
        let started = std::time::Instant::now();
        let err = instance.call("loop", "{}", "call-wall").expect_err("the budget must stop the guest");
        let elapsed = started.elapsed();
        assert!(matches!(err, RasqWasmError::CallBudget(_)), "{err:?}");
        assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
        assert!(serde_error_json(&err).contains(r#""code":"E02""#));
        // 1 回の通信も残り時間より長くは待たない(5 秒かかる相手でも 1 秒で戻る)。
        let port = slow_server(Duration::from_secs(5));
        let mut instance = RasqWasmInstance::load_plain(fetch_loop_wat(port, 1).as_bytes()).expect("instantiate");
        instance.set_native_fetch(true);
        instance.set_fetch_hosts(&["127.0.0.1"]);
        rasq_wasm_set_call_timeout_ms(&mut instance, 1_000);
        let started = std::time::Instant::now();
        let _ = instance.call("loop", "{}", "call-wall-1");
        assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
    }

    #[test]
    fn host_calls_per_call_are_bounded_and_ordinary_fetches_still_work() {
        let port = slow_server(Duration::from_millis(0));
        // ふつうの数の通信は通る(予算は呼び出しごとに積み直す)。
        let mut instance = RasqWasmInstance::load_plain(fetch_loop_wat(port, 3).as_bytes()).expect("instantiate");
        instance.set_native_fetch(true);
        instance.set_fetch_hosts(&["127.0.0.1"]);
        for i in 0..3 {
            instance.call("loop", "{}", &format!("call-ok-{i}")).expect("ordinary fetches pass");
        }
        // 1 回の呼び出しで MAX_NATIVE_FETCHES_PER_CALL を超えたら E02。
        let mut instance =
            RasqWasmInstance::load_plain(fetch_loop_wat(port, MAX_NATIVE_FETCHES_PER_CALL + 5).as_bytes()).expect("instantiate");
        instance.set_native_fetch(true);
        instance.set_fetch_hosts(&["127.0.0.1"]);
        let err = instance.call("loop", "{}", "call-many").expect_err("too many fetches");
        assert!(matches!(&err, RasqWasmError::CallBudget(m) if m.contains("fetches")), "{err:?}");
        // ネイティブの通信が閉じていれば(ホストを宣言していない)、従来どおり E01 で返し、予算には数えない。
        let mut instance = RasqWasmInstance::load_plain(fetch_loop_wat(port, 3).as_bytes()).expect("instantiate");
        instance.set_native_fetch(true);
        instance.call("loop", "{}", "call-closed").expect("E01 is returned to the guest, not a trap");
    }

    /// `call` の中で永久に回るだけのゲスト。サンドボックスは破らないが、
    /// 燃料が無ければ呼び出しスレッドは二度と返ってこない。
    const RUNAWAY_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32))
          (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i64)
            (loop $spin (br $spin))
            (i64.const 0)))
    "#;

    /// `call` のたびに上限を大きく越える `memory.grow` を投げるゲスト。
    const MEMORY_HOG_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32))
          (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i64)
            (drop (memory.grow (i32.const 8192)))
            (i64.const 0)))
    "#;

    /// 上限のすぐ内側(+16 MiB)まで伸ばすゲスト。正当なプラグインを
    /// 巻き込んでいないことの対照。
    const MEMORY_OK_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32))
          (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i64)
            (drop (memory.grow (i32.const 256)))
            (i64.const 0)))
    "#;

    #[test]
    fn runaway_guest_runs_out_of_fuel_in_bounded_time() {
        let mut instance = RasqWasmInstance::load_plain(RUNAWAY_WAT.as_bytes()).expect("instantiate");
        // 既定の 2 億は release 前提の値で、debug ビルドのインタプリタだと
        // 秒がかさむ。上限が効くことの確認に大きさは要らないので絞る。
        instance.fuel_per_call = 1_000_000;

        let started = std::time::Instant::now();
        let err = instance
            .call("spin", "{}", "call-fuel")
            .expect_err("a runaway guest must not return normally");
        let elapsed = started.elapsed();

        assert!(matches!(err, RasqWasmError::FuelExhausted), "{err:?}");
        assert!(elapsed < Duration::from_secs(30), "took {elapsed:?}");
        // 埋込側(Dart)が見るのは JSON のほう。E02 で「暴走」と分かること。
        let json = serde_error_json(&err);
        assert!(json.contains(r#""code":"E02""#), "{json}");
    }

    #[test]
    fn fuel_is_refilled_per_call_not_once_per_load() {
        // 同じインスタンスを何度叩いても、毎回同じ予算で回る。燃料を
        // ロード時に一度だけ積む実装だと 2 回目以降が理由もなく落ちる。
        let mut instance = load();
        for i in 0..5 {
            let json = instance
                .call("openThread", r#"{"postId":"abc"}"#, &format!("call-refuel-{i}"))
                .expect("call");
            assert!(json.contains(r#""ok":true"#), "{json}");
        }
        let json = instance.snapshot().expect("snapshot");
        assert!(json.contains("rasq.boundary/1"), "{json}");
    }

    #[test]
    fn memory_growth_past_the_cap_fails_cleanly() {
        let mut instance =
            RasqWasmInstance::load_plain(MEMORY_HOG_WAT.as_bytes()).expect("instantiate");
        let err = instance
            .call("grow", "{}", "call-mem")
            .expect_err("growing past the cap must fail");
        assert!(matches!(err, RasqWasmError::MemoryLimit), "{err:?}");
        let json = serde_error_json(&err);
        assert!(json.contains(r#""code":"E03""#), "{json}");
        // 「clean」= ホストは生きていて、同じインスタンスにまた話しかけられる。
        let err2 = instance.call("grow", "{}", "call-mem-2").expect_err("still limited");
        assert!(matches!(err2, RasqWasmError::MemoryLimit), "{err2:?}");
    }

    /// 1 本ずつなら上限ちょうど(256 MiB = 4096 ページ)だが、合計では倍になるモジュール(2026-10-09 の [5])。
    const TWO_MEMORIES_WAT: &str = r#"
        (module
          (memory (export "memory") 4096)
          (memory $second 4096)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32)))
    "#;

    /// テーブルを 2 本宣言するモジュール(reference-types が有効なので検証は通る)。
    const TWO_TABLES_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (table 1000 funcref)
          (table 1000 funcref)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32)))
    "#;

    #[test]
    fn modules_with_more_than_one_memory_or_table_are_rejected_before_allocation() {
        // 2 本目のメモリは multi-memory を切った検証で落ちる(確保の前)。
        let err = RasqWasmInstance::load_plain(TWO_MEMORIES_WAT.as_bytes()).err().expect("two memories must be rejected");
        assert!(format!("{err:?}").to_lowercase().contains("memor"), "{err:?}");
        // FFI の入口(両方のネイティブ controller が通る rasq_wasm_instantiate*)でも null が返る。
        let bytes = TWO_MEMORIES_WAT.as_bytes(); // wasmi は WAT のテキストもそのまま受ける
        let ptr = rasq_wasm_instantiate(bytes.as_ptr(), bytes.len());
        assert!(ptr.is_null());
        let ptr = rasq_wasm_instantiate_with_host(bytes.as_ptr(), bytes.len(), test_host_fn, std::ptr::null_mut());
        assert!(ptr.is_null());
        // テーブルの数も 1 本まで(インスタンス化の数の検査 = 確保の前)。
        assert!(RasqWasmInstance::load_plain(TWO_TABLES_WAT.as_bytes()).is_err());
        // ふつうの 1 本ずつのモジュールは通る(対照)。
        RasqWasmInstance::load_plain(MEMORY_OK_WAT.as_bytes()).expect("single memory/table still loads");
    }

    #[test]
    fn aggregate_limiter_caps_the_sum_across_memories_and_tables() {
        use wasmi::ResourceLimiter as _;
        const MIB: usize = 1024 * 1024;
        // 数の検査がもし緩んでも、合計で 256 MiB を越える 2 本目の初期確保は拒む。
        let mut l = AggregateLimits::new();
        assert_eq!((l.memories(), l.tables(), l.instances()), (1, 1, 1));
        assert!(l.memory_growing(0, 200 * MIB, None).expect("first memory fits"));
        assert!(l.memory_growing(0, 100 * MIB, None).is_err(), "aggregate must not exceed MAX_MEMORY_BYTES");
        // 1 本目の成長も合計で数える。
        assert!(l.memory_growing(200 * MIB, 256 * MIB, None).expect("grow up to the cap"));
        assert!(l.memory_growing(256 * MIB, 256 * MIB + 65536, None).is_err());
        // 許可のあとの確保の失敗は差し戻す(合計が膨らみ続けない)。
        let mut l = AggregateLimits::new();
        assert!(l.memory_growing(0, 200 * MIB, None).expect("fits"));
        l.memory_grow_failed(&wasmi_core::LimiterError::OutOfSystemMemory);
        assert!(l.memory_growing(0, 256 * MIB, None).expect("budget returned after the failure"));
        // テーブル要素も合計。
        let mut l = AggregateLimits::new();
        assert!(l.table_growing(0, MAX_TABLE_ELEMENTS - 10, None).expect("fits"));
        assert!(l.table_growing(0, 11, None).is_err());
        assert!(l.table_growing(0, 10, None).expect("exactly the cap"));
    }

    #[test]
    fn memory_growth_under_the_cap_still_works() {
        let mut instance =
            RasqWasmInstance::load_plain(MEMORY_OK_WAT.as_bytes()).expect("instantiate");
        instance.call("grow", "{}", "call-mem-ok").expect("grow within the cap");
        assert!(instance.memory.size(&instance.store) > 1);
    }

    #[test]
    fn non_media_plugin_fits_the_standard_budget() {
        // ビットマップを触らないプラグインは FUEL_BUDGET_STANDARD(埋込側が
        // manifest を見て絞る想定の値)に収まること。既定の 6 G ではなく
        // **絞ったほう**で測るのが肝心 — 緩い既定でしか通らないなら、
        // 「軽い種別は絞ってよい」という推奨が嘘になる。
        let bytes = fixture().to_vec();
        let mut instance =
            RasqWasmInstance::load(&bytes, Some(test_host_fn), 0).expect("instantiate");
        instance.fuel_per_call = FUEL_BUDGET_STANDARD;
        instance
            .call("timeline", r#"{"host":"misskey.example","limit":10}"#, "call-budget")
            .expect("timeline must fit the standard budget");
        let used = FUEL_BUDGET_STANDARD - instance.store.get_fuel().expect("fuel metering is on");
        assert!(
            used * 10 < FUEL_BUDGET_STANDARD,
            "timeline burned {used} of {FUEL_BUDGET_STANDARD} fuel — too tight"
        );
    }

    /// 上限の校正そのものを固定する回帰テスト。実測で一番重い**正当な**
    /// 呼び出し(media_process の sticker、約 6.4 億)が既定予算に収まること。
    /// これが落ちたら rasq-client の `slot_plugins_wasm_test.dart` も落ちる。
    ///
    /// 既定では走らせない: フィクスチャが別リポジトリ(plugin-sample/dist)で、
    /// debug ビルドのインタプリタだと 6.4 億燃やすのに分単位かかるため。
    ///     cargo test --release -p rasq_wasm_runtime_core -- --ignored
    #[test]
    #[ignore = "needs plugin-sample/dist; run with --release"]
    fn media_plugin_heaviest_call_fits_the_default_budget() {
        const DIST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../plugin-sample/dist");
        // slot_plugins_wasm_test.dart が使うのと同じ 2×1 PNG。
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADklEQVR4nGP4z8AAQv8BD/kD/YURmXYAAAAASUVORK5CYII=";
        let path = format!("{DIST}/media_process_plugin.wasm");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipped: {path} not built");
            return;
        };
        let mut instance =
            RasqWasmInstance::load(&bytes, Some(test_host_fn), 0).expect("instantiate");
        let mut worst = 0u64;
        for (i, (f, a)) in [
            ("media.edit", format!(r#"{{"token":"t1","index":0,"name":"a.png","mimeType":"image/png","bytesB64":"{PNG}"}}"#)),
            ("media.action", r#"{"token":"t1","genre":"process","action":"faces"}"#.to_string()),
            ("media.action", r#"{"token":"t1","genre":"process","action":"sticker"}"#.to_string()),
            ("sticker.add.0", r#"{"nodeId":"x","kind":"tap"}"#.to_string()),
            ("media.commit", r#"{"token":"t1"}"#.to_string()),
        ]
        .into_iter()
        .enumerate()
        {
            instance.call(f, &a, &format!("cal-{i}")).unwrap_or_else(|e| panic!("{f}: {e:?}"));
            let used = FUEL_PER_CALL - instance.store.get_fuel().expect("fuel");
            worst = worst.max(used);
        }
        assert!(
            worst * 2 < FUEL_PER_CALL,
            "heaviest legitimate media call burned {worst} of {FUEL_PER_CALL} — \
             less than 2x headroom left, the budget needs re-calibrating"
        );
    }
}
