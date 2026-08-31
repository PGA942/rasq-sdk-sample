//! リレーショナルTL(関連TL)エンジンの wasm 移植 (M5f / D-084)。
//!
//! **特別枠**: ストア配布の TL プラグインではなく、RASQ 同梱の公式
//! モジュール。アカウントバンドルに含まれる全アカウントの TL 系を見る
//! (v1 は現在アカウント)。入力はホストがタグ規約 (PROTOCOL §2.2) 経由で
//! 抽出した最小ワイヤ(本文/引用本文/リンク/引用先)— SNS 横断でも同じ形。
//!
//! rasq-client の `relational_engine.dart`(会話イベント方式 D-027)の
//! ルールベース部の忠実な移植。SigLIP / Louvain(ML)はホスト側資産のため
//! v1 は非対応 — ML があるときはホストが Dart エンジンを使う。
//! カルクレーター統合後は wasm 側から `ml.infer` を呼ぶ形に置換する。
//!
//! `call("relational.build", {posts, guests, settings?, communities?, nowMs, lookbackMinutes?})`
//!   → `{events: [{postKeys: [key…(時系列順)], lastActivityMs}]}`
//! 入力 post: `{key, authorKey, repostedByKey?, createdAtMs, body, quoteBody?, linkUrl?, quoteKey?}`

use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::mem;
use std::sync::OnceLock;

// ---- ABI plumbing(misskey-wasm-sample と同じ最小形)---------------------

fn pack(mut buf: Vec<u8>) -> u64 {
    buf.shrink_to_fit();
    let len = buf.len() as u64;
    let ptr = buf.as_ptr() as u64;
    mem::forget(buf);
    (ptr << 32) | len
}

#[no_mangle]
pub extern "C" fn alloc(len: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr() as u32;
    mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: u32, len: u32) {
    unsafe {
        drop(Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize));
    }
}

fn read_str(ptr: u32, len: u32) -> String {
    unsafe {
        let slice = std::slice::from_raw_parts(ptr as *const u8, len as usize);
        String::from_utf8_lossy(slice).into_owned()
    }
}

const SCOPE: &str = "rasq.official.relational:*:0.1.0:engine";

/// ホスト UI は持たない(特別枠エンジン)。snapshot は目印だけ返す。
#[no_mangle]
pub extern "C" fn snapshot() -> u64 {
    let msg = json!({
        "schema": "rasq.boundary/1",
        "scope": SCOPE,
        "type": "snapshot",
        "rev": 1,
        "root": { "t": "Text", "k": "engine", "p": { "text": "relational engine (wasm)" } }
    });
    pack(msg.to_string().into_bytes())
}

#[no_mangle]
pub extern "C" fn patch() -> u64 {
    let msg = json!({ "schema": "rasq.boundary/1", "scope": SCOPE, "type": "patch", "rev": 1, "baseRev": 1, "ops": [] });
    pack(msg.to_string().into_bytes())
}

#[no_mangle]
pub extern "C" fn call(fn_ptr: u32, fn_len: u32, args_ptr: u32, args_len: u32, call_id_ptr: u32, call_id_len: u32) -> u64 {
    let fn_name = read_str(fn_ptr, fn_len);
    let args: Value = serde_json::from_str(&read_str(args_ptr, args_len)).unwrap_or(Value::Null);
    let call_id = read_str(call_id_ptr, call_id_len);
    let outcome: Result<Value, Value> = match fn_name.as_str() {
        "relational.build" => Ok(build(&args)),
        other => Err(json!({ "code": "C01", "message": format!("unsupported fn: {other}") })),
    };
    let mut msg = json!({ "schema": "rasq.boundary/1", "scope": SCOPE, "type": "result", "callId": call_id, "ok": outcome.is_ok() });
    match outcome {
        Ok(value) => msg["value"] = value,
        Err(error) => msg["error"] = error,
    }
    pack(msg.to_string().into_bytes())
}

// ---- 入力 -----------------------------------------------------------------

#[derive(Clone)]
struct P {
    key: String,
    author_key: String,
    reposted_by_key: Option<String>,
    created_ms: i64,
    body: String,
    text: String, // body + 引用本文(特徴抽出用)
    link_url: Option<String>,
    quote_key: Option<String>,
}

fn parse_posts(v: &Value) -> Vec<P> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|j| {
            let body = j["body"].as_str().unwrap_or("").to_string();
            let quote_body = j["quoteBody"].as_str().unwrap_or("");
            Some(P {
                key: j["key"].as_str()?.to_string(),
                author_key: j["authorKey"].as_str().unwrap_or("?").to_string(),
                reposted_by_key: j["repostedByKey"].as_str().map(|s| s.to_string()),
                created_ms: j["createdAtMs"].as_i64().unwrap_or(0),
                text: format!("{body} {quote_body}"),
                body,
                link_url: j["linkUrl"].as_str().map(|s| s.to_string()),
                quote_key: j["quoteKey"].as_str().map(|s| s.to_string()),
            })
        })
        .collect()
}

// ---- 設定(relational_engine.dart の RelationalSettings/LinkWeights)------

struct Weights {
    content: f64,
    entity: f64,
    time: f64,
    affinity: f64,
    turn: f64,
    anchor: f64,
    hard_anchor_bonus: f64,
    reactive_bonus: f64,
    community_prior: f64,
}

struct Settings {
    link_bias: i64,
    candidate_window_minutes: i64,
    min_ttl_minutes: i64,
    max_ttl_minutes: i64,
    tau_seconds: i64,
    guest_bias: f64,
    w: Weights,
}

fn parse_settings(v: &Value) -> Settings {
    let d = |x: &Value, f: f64| x.as_f64().unwrap_or(f);
    let i = |x: &Value, f: i64| x.as_i64().unwrap_or(f);
    let w = &v["weights"];
    Settings {
        link_bias: i(&v["linkBias"], 75),
        candidate_window_minutes: i(&v["candidateWindowMinutes"], 30),
        min_ttl_minutes: i(&v["minTtlMinutes"], 5),
        max_ttl_minutes: i(&v["maxTtlMinutes"], 30),
        tau_seconds: i(&v["tauSeconds"], 600),
        guest_bias: d(&v["guestBias"], 0.07),
        w: Weights {
            content: d(&w["content"], 0.45),
            entity: d(&w["entity"], 0.20),
            time: d(&w["time"], 0.15),
            affinity: d(&w["affinity"], 0.10),
            turn: d(&w["turn"], 0.05),
            anchor: d(&w["anchor"], 0.10),
            hard_anchor_bonus: d(&w["hardAnchorBonus"], 0.2),
            reactive_bonus: d(&w["reactiveBonus"], 0.15),
            community_prior: d(&w["communityPrior"], 0.05),
        },
    }
}

impl Settings {
    fn bias_offset(&self) -> f64 {
        ((self.link_bias.clamp(50, 100) - 75) as f64) / 100.0
    }
}

// ---- 特徴抽出(extractFeaturesText / anchors / reactive / n-gram)---------

fn re(pattern: &'static str) -> &'static Regex {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap();
    map.entry(pattern).or_insert_with(|| Box::leak(Box::new(Regex::new(pattern).unwrap())))
}

fn extract_features(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for c in re(r"#([^\s#＃]+)").captures_iter(text) {
        out.insert(c[1].to_lowercase());
    }
    for c in re(r"@([A-Za-z0-9_.-]+)").captures_iter(text) {
        out.insert(c[1].to_lowercase());
    }
    for c in re(r"([ァ-ヴー]{2,}|[A-Za-z0-9][A-Za-z0-9\-_.]{1,})").captures_iter(text) {
        out.insert(c[1].to_lowercase());
    }
    for c in re(r"([一-龠々]{2,})").captures_iter(text) {
        out.insert(c[1].to_string());
    }
    out
}

fn strong_anchors(text: &str) -> HashSet<String> {
    re(r"#([^\s#＃]+)").captures_iter(text).map(|c| format!("#{}", c[1].to_lowercase())).collect()
}

fn hard_anchors(p: &P) -> HashSet<String> {
    let mut out: HashSet<String> = re(r"https?://[^\s]+")
        .find_iter(&p.text)
        .map(|m| re(r"[).,]+$").replace(m.as_str(), "").to_lowercase())
        .collect();
    if let Some(u) = &p.link_url {
        out.insert(u.to_lowercase());
    }
    if let Some(q) = &p.quote_key {
        out.insert(format!("quote:{q}"));
    }
    out
}

fn is_reactive(body: &str) -> bool {
    let b = body.trim();
    if b.is_empty() || b.encode_utf16().count() > 30 {
        return false;
    }
    re(r"(それな|それはそう|わか(る|り)|分か(る|り)|たしかに|確かに|ほん(と|ま)それ|ほんそれ|まじ|マジ|ﾏｼﾞ|え[っぇえー]+|うそ|ウソ|嘘でしょ|草|くさ|ｗ+|w{2,}|笑った|わろた|ワロタ|やば|ヤバ|つら|辛|無理|むり|尊い|最高|好き|すき|おめでと|お(つかれ|疲れ)|それ|これ|あれ|だよね|よね[ぇー]?)").is_match(b)
}

fn ngram_profile(text: &str) -> BTreeMap<String, f64> {
    let lowered = text.to_lowercase();
    let t = re(r"https?://[^\s]+").replace_all(&lowered, " ");
    let t = re(r"[\s#＃@]+").replace_all(&t, "");
    let chars: Vec<char> = t.chars().collect();
    let mut counts = BTreeMap::new();
    for n in [2usize, 3] {
        if chars.len() < n {
            continue;
        }
        for i in 0..=(chars.len() - n) {
            let g: String = chars[i..i + n].iter().collect();
            *counts.entry(g).or_insert(0.0) += 1.0;
        }
    }
    counts
}

fn cosine(a: &BTreeMap<String, f64>, b: &BTreeMap<String, f64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let na: f64 = a.values().map(|v| v * v).sum();
    let nb: f64 = b.values().map(|v| v * v).sum();
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let dot: f64 = small.iter().filter_map(|(k, v)| large.get(k).map(|w| v * w)).sum();
    dot / (na.sqrt() * nb.sqrt())
}

fn entity_overlap(a: &HashSet<String>, b: &HashSet<String>) -> usize {
    a.iter()
        .filter(|x| {
            if x.chars().count() < 2 {
                return false;
            }
            b.contains(*x) || b.iter().any(|y| y.chars().count() >= 2 && (x.contains(y.as_str()) || y.contains(x.as_str())))
        })
        .count()
}

// ---- 親密度 / イベント -----------------------------------------------------

#[derive(Default)]
struct Affinity {
    count: HashMap<String, i64>,
}

impl Affinity {
    fn key(a: &str, b: &str) -> String {
        if a <= b {
            format!("{a}|{b}")
        } else {
            format!("{b}|{a}")
        }
    }
    fn record(&mut self, a: &str, b: &str, weight: i64) {
        if a == b {
            return;
        }
        *self.count.entry(Self::key(a, b)).or_insert(0) += weight;
    }
    fn of(&self, a: &str, b: &str) -> f64 {
        let c = *self.count.get(&Self::key(a, b)).unwrap_or(&0) as f64;
        c / (c + 3.0)
    }
}

struct Event {
    posts: Vec<usize>, // all_posts への index(時系列順)
    post_keys: HashSet<String>,
    entities: HashSet<String>,
    anchors: HashSet<String>,
    hard: HashSet<String>,
    participants: HashMap<String, i64>,
    intervals: Vec<i64>,
    last_activity_ms: i64,
    root_author_key: String,
}

#[derive(PartialEq)]
enum State {
    Tentative,
    Active,
    Dormant,
    Closed,
}

impl Event {
    fn root(idx: usize, p: &P) -> Event {
        let mut participants = HashMap::new();
        participants.insert(p.author_key.clone(), p.created_ms);
        if let Some(r) = &p.reposted_by_key {
            participants.insert(r.clone(), p.created_ms);
        }
        Event {
            posts: vec![idx],
            post_keys: HashSet::from([p.key.clone()]),
            entities: extract_features(&p.text),
            anchors: strong_anchors(&p.text),
            hard: hard_anchors(p),
            participants,
            intervals: vec![],
            last_activity_ms: p.created_ms,
            root_author_key: p.author_key.clone(),
        }
    }

    fn add(&mut self, idx: usize, p: &P, all: &[P]) {
        self.intervals.push(((p.created_ms - self.last_activity_ms) / 1000).abs());
        // 遡及挿入(ゲスト D-046): 時系列位置に入れる。
        let pos = self.posts.iter().position(|&q| all[q].created_ms > p.created_ms);
        match pos {
            Some(i) => self.posts.insert(i, idx),
            None => self.posts.push(idx),
        }
        self.post_keys.insert(p.key.clone());
        self.entities.extend(extract_features(&p.text));
        self.anchors.extend(strong_anchors(&p.text));
        self.hard.extend(hard_anchors(p));
        let touch = |m: &mut HashMap<String, i64>, k: &str| {
            let e = m.entry(k.to_string()).or_insert(i64::MIN);
            if p.created_ms > *e {
                *e = p.created_ms;
            }
        };
        touch(&mut self.participants, &p.author_key);
        if let Some(r) = &p.reposted_by_key {
            touch(&mut self.participants, r);
        }
        if p.created_ms > self.last_activity_ms {
            self.last_activity_ms = p.created_ms;
        }
    }

    fn ttl_seconds(&self, s: &Settings) -> i64 {
        if self.intervals.is_empty() {
            return s.min_ttl_minutes * 60;
        }
        let mut sorted = self.intervals.clone();
        sorted.sort();
        let q90 = sorted[(((sorted.len() - 1) as f64) * 0.9).round() as usize];
        (2 * q90).clamp(s.min_ttl_minutes * 60, s.max_ttl_minutes * 60)
    }

    fn state_at(&self, now_ms: i64, s: &Settings, all: &[P]) -> State {
        if self.posts.len() < 2 {
            let root_ms = all[self.posts[0]].created_ms;
            return if now_ms - root_ms > s.candidate_window_minutes * 60_000 { State::Closed } else { State::Tentative };
        }
        let idle_sec = (now_ms - self.last_activity_ms) / 1000;
        let t = self.ttl_seconds(s);
        if idle_sec <= t {
            State::Active
        } else if idle_sec <= t * 2 {
            State::Dormant
        } else {
            State::Closed
        }
    }

    /// 固定アンカー(先頭 2〜3)+ 最近プロトタイプ(直近 8)の index 集合。
    fn compare_targets(&self) -> HashSet<usize> {
        let mut out: HashSet<usize> = self.posts.iter().take(3).copied().collect();
        let n = self.posts.len();
        out.extend(self.posts.iter().skip(n.saturating_sub(8)).copied());
        out
    }
}

// ---- エンジン ---------------------------------------------------------------

struct Engine<'a> {
    s: &'a Settings,
    all: &'a [P],
    profiles: HashMap<usize, BTreeMap<String, f64>>,
    root_score_samples: HashMap<Option<i64>, Vec<f64>>,
    communities: &'a HashMap<String, i64>,
}

impl<'a> Engine<'a> {
    fn profile(&mut self, idx: usize) -> &BTreeMap<String, f64> {
        self.profiles.entry(idx).or_insert_with(|| ngram_profile(&self.all[idx].body))
    }

    fn threshold_for(&self, community: Option<i64>) -> f64 {
        let mut base = 0.5;
        if let Some(samples) = self.root_score_samples.get(&community) {
            if samples.len() >= 8 {
                let mut sorted = samples.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median = sorted[sorted.len() / 2];
                let mut devs: Vec<f64> = samples.iter().map(|s| (s - median).abs()).collect();
                devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let mad = devs[devs.len() / 2];
                base = (median + 2.5 * mad).clamp(0.35, 0.8);
            }
        }
        (base + self.s.bias_offset()).clamp(0.3, 0.95)
    }

    fn link_score(&mut self, idx: usize, e: &Event, affinity: &Affinity, anchor_match: bool, hard_match: bool, time_ref_ms: Option<i64>) -> f64 {
        let p = &self.all[idx];
        let w = &self.s.w;
        let mut content: f64 = 0.0;
        let p_profile = self.profile(idx).clone();
        for q in e.compare_targets() {
            let c = cosine(&p_profile, self.profile(q));
            if c > content {
                content = c;
            }
        }
        let dt_sec = ((p.created_ms - time_ref_ms.unwrap_or(e.last_activity_ms)) / 1000).abs();
        let time_decay = (-(dt_sec as f64) / self.s.tau_seconds as f64).exp();
        let overlap = entity_overlap(&extract_features(&p.text), &e.entities);
        let entity_score: f64 = if overlap >= 2 { 1.0 } else if overlap == 1 { 0.6 } else { 0.0 };
        let mut aff: f64 = 0.0;
        for u in e.participants.keys() {
            let a = affinity.of(&p.author_key, u);
            if a > aff {
                aff = a;
            }
        }
        let mut turn = 0.0;
        if e.participants.contains_key(&p.author_key) {
            let last_author = &self.all[*e.posts.last().unwrap()].author_key;
            turn = if last_author != &p.author_key { 1.0 } else { 0.4 };
        }
        let anchor = if hard_match { 1.0 } else if anchor_match { 0.6 } else { 0.0 };
        let hard_bonus = if hard_match { w.hard_anchor_bonus } else { 0.0 };
        let mut prior = 0.0;
        if let (Some(ca), Some(ce)) = (self.communities.get(&p.author_key), self.communities.get(&e.root_author_key)) {
            prior = if ca == ce { w.community_prior } else { -w.community_prior };
        }
        if is_reactive(&p.body) {
            return (0.15 * content + 0.40 * time_decay + 0.20 * aff + 0.15 * turn + 0.10 * entity_score.max(anchor) + w.reactive_bonus + hard_bonus + prior).clamp(0.0, 1.0);
        }
        (w.content * content + w.entity * entity_score + w.time * time_decay + w.affinity * aff + w.turn * turn + w.anchor * anchor + hard_bonus + prior).clamp(0.0, 1.0)
    }
}

fn build(args: &Value) -> Value {
    let settings = parse_settings(&args["settings"]);
    let followees = parse_posts(&args["posts"]);
    let guests = parse_posts(&args["guests"]);
    let now_ms = args["nowMs"].as_i64().unwrap_or(0);
    let lookback_min = args["lookbackMinutes"].as_i64().unwrap_or(0);
    let communities: HashMap<String, i64> = args["communities"]
        .as_object()
        .map(|m| m.iter().filter_map(|(k, v)| v.as_i64().map(|i| (k.clone(), i))).collect())
        .unwrap_or_default();

    // all = followees(時系列順)+ guests(後段接続)。
    let mut followee_sorted = followees.clone();
    followee_sorted.sort_by_key(|p| p.created_ms);
    let mut all: Vec<P> = followee_sorted.clone();
    let guest_start = all.len();
    let mut guest_sorted = guests.clone();
    guest_sorted.sort_by_key(|p| p.created_ms);
    all.extend(guest_sorted);

    let mut engine = Engine { s: &settings, all: &all, profiles: HashMap::new(), root_score_samples: HashMap::new(), communities: &communities };
    let mut events: Vec<Event> = vec![];
    let mut affinity = Affinity::default();
    let window_ms = settings.candidate_window_minutes * 60_000;

    let mut prev: Option<usize> = None;
    for idx in 0..guest_start {
        let p = &all[idx];
        if let Some(pv) = prev {
            if p.created_ms - all[pv].created_ms <= 5 * 60_000 {
                affinity.record(&all[pv].author_key, &p.author_key, 1);
            }
        }
        if let Some(r) = &p.reposted_by_key {
            affinity.record(r, &p.author_key, 2);
        }
        let anchors_of_p = strong_anchors(&p.text);
        let hard_of_p = hard_anchors(p);
        let mut best: Option<usize> = None;
        let mut best_score = 0.0;
        let mut saw_candidate = false;
        for (ei, e) in events.iter().enumerate() {
            if e.post_keys.contains(&p.key) {
                continue;
            }
            let hard_match = hard_of_p.iter().any(|h| e.hard.contains(h));
            let anchor_match = hard_match || anchors_of_p.iter().any(|a| e.anchors.contains(a));
            let within = p.created_ms - e.last_activity_ms <= window_ms;
            if !within && !hard_match {
                continue;
            }
            if e.state_at(p.created_ms, &settings, &all) == State::Closed && !hard_match {
                continue;
            }
            saw_candidate = true;
            let s = engine.link_score(idx, e, &affinity, anchor_match, hard_match, None);
            if s > best_score {
                best_score = s;
                best = Some(ei);
            }
        }
        let community = communities.get(&p.author_key).copied();
        if let Some(ei) = best {
            if best_score >= engine.threshold_for(community) {
                events[ei].add(idx, p, &all);
                prev = Some(idx);
                continue;
            }
        }
        if saw_candidate {
            engine.root_score_samples.entry(community).or_default().push(best_score);
        }
        events.push(Event::root(idx, p));
        prev = Some(idx);
    }

    // ゲスト接続 (D-046): リンクのみ・しきい値 +guestBias・遡及可。
    for idx in guest_start..all.len() {
        let p = &all[idx];
        if events.iter().any(|e| e.post_keys.contains(&p.key)) {
            continue;
        }
        let mut best: Option<usize> = None;
        let mut best_score = 0.0;
        let mut best_ref: Option<i64> = None;
        for (ei, e) in events.iter().enumerate() {
            let hard_match = hard_anchors(p).iter().any(|h| e.hard.contains(h));
            let mut ref_ms = e.last_activity_ms;
            let mut ref_dt = ((p.created_ms - ref_ms) / 1000).abs();
            for &q in &e.posts {
                let dt = ((p.created_ms - all[q].created_ms) / 1000).abs();
                if dt < ref_dt {
                    ref_dt = dt;
                    ref_ms = all[q].created_ms;
                }
            }
            if ref_dt * 1000 > window_ms && !hard_match {
                continue;
            }
            let anchor_match = hard_match || strong_anchors(&p.text).iter().any(|a| e.anchors.contains(a));
            let s = engine.link_score(idx, e, &affinity, anchor_match, hard_match, Some(ref_ms));
            if s > best_score {
                best_score = s;
                best = Some(ei);
                best_ref = Some(ref_ms);
            }
        }
        let threshold = (engine.threshold_for(communities.get(&p.author_key).copied()) + settings.guest_bias).clamp(0.3, 0.98);
        if let (Some(ei), Some(_)) = (best, best_ref) {
            if best_score >= threshold {
                events[ei].add(idx, p, &all);
            }
        }
    }

    // 表示対象 = ACTIVE(+ lookback 以内の沈み会話)、活動が新しい順。
    let mut out: Vec<&Event> = events
        .iter()
        .filter(|e| {
            e.posts.len() >= 2
                && (e.state_at(now_ms, &settings, &all) == State::Active
                    || (lookback_min > 0 && now_ms - e.last_activity_ms <= lookback_min * 60_000))
        })
        .collect();
    out.sort_by_key(|e| -e.last_activity_ms);

    let events_json: Vec<Value> = out
        .iter()
        .map(|e| {
            json!({
                "postKeys": e.posts.iter().map(|&i| all[i].key.clone()).collect::<Vec<String>>(),
                "lastActivityMs": e.last_activity_ms,
            })
        })
        .collect();
    json!({ "events": events_json })
}
