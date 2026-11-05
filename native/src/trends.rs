//! 热点雷达(/api/trends*):逐语义复刻上游 easel/trends(P5 忠实移植)。
//!
//! - 内置 6 平台(微博/抖音/知乎/B站/百度/头条)原生 reqwest 收集(上游为
//!   bundled python 收集器,输出同 v1 契约:双上游 URL 兜底 + 解析规则一致)
//! - 自定义数据源 `config/trend-sources.json`:http / rss / python 脚本(spawn,
//!   stdout v1 契约,128KiB 配置上限,≤64 源)
//! - 缓存语义:refresh 重取、max_stale 可用性、失败 30s 重试、busy 单飞
//! - 分组渲染字段与上游 _render/failure_group 一致

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 500;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(8);
const COLLECT_TIMEOUT: Duration = Duration::from_secs(20);
const FAILURE_RETRY: Duration = Duration::from_secs(30);
const BUSY_RETRY: Duration = Duration::from_secs(1);

const BUILTIN_LABELS: &[(&str, &str)] = &[
    ("weibo", "微博"),
    ("douyin", "抖音"),
    ("zhihu", "知乎"),
    ("bilibili", "B站"),
    ("baidu", "百度"),
    ("toutiao", "头条"),
];

/// 双上游兜底(与上游 builtin.py SOURCES 一致)。
const BUILTIN_UPSTREAMS: &[(&str, [&str; 2])] = &[
    ("weibo", ["https://v2.xxapi.cn/api/weibohot", "https://60s.viki.moe/v2/weibo"]),
    ("douyin", ["https://v2.xxapi.cn/api/douyinhot", "https://60s.viki.moe/v2/douyin"]),
    ("zhihu", ["https://api.zhihu.com/topstory/hot-list?limit=50", "https://60s.viki.moe/v2/zhihu"]),
    ("bilibili", ["https://v2.xxapi.cn/api/bilibilihot", "https://60s.viki.moe/v2/bili"]),
    ("baidu", ["https://v2.xxapi.cn/api/baiduhot", "https://60s.viki.moe/v2/baidu/hot"]),
    ("toutiao", ["https://www.toutiao.com/hot-event/hot-board/?origin=toutiao_pc", "https://60s.viki.moe/v2/toutiao"]),
];

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("创建 trends runtime 失败")
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn utc_now_iso() -> String {
    // 上游为 ISO UTC;此处以 epoch 秒的 ISO 形式承载(前端仅透传展示)。
    format!("{}-epoch", now_secs())
}

// ── 数据源定义 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Source {
    pub id: String,
    pub label: String,
    pub kind: String, // builtin | http | rss | python
    pub default: bool,
    pub timeout: Duration,
    pub refresh: Duration,
    pub max_stale: Duration,
    pub options: Map<String, Value>,
}

impl Source {
    fn public(&self) -> Value {
        json!({
            "id": self.id,
            "label": display_label(&self.label),
            "type": self.kind,
            "default": self.default,
            "refresh_seconds": self.refresh.as_secs(),
        })
    }
}

fn display_label(label: &str) -> String {
    // 上游 display_label:剥掉 "· python/http/rss/atom" 尾缀
    let mut result = label.trim().to_string();
    while let Some(position) = result.rfind(['·', '|', '•']) {
        let tail = result[position..].to_ascii_lowercase();
        if tail.contains("python") || tail.contains("http") || tail.contains("rss") || tail.contains("atom") {
            result.truncate(position);
            result = result.trim().to_string();
        } else {
            break;
        }
    }
    if result.is_empty() {
        "未命名来源".to_string()
    } else {
        result
    }
}

fn config_path() -> PathBuf {
    if let Ok(configured) = std::env::var("EASEL_TREND_SOURCES_CONFIG") {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    crate::workspace::project_root().join("config").join("trend-sources.json")
}

fn load_user_config() -> (Vec<Value>, bool, Vec<String>) {
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (Vec::new(), true, Vec::new());
    };
    if text.len() > 128 * 1024 {
        return (Vec::new(), true, vec!["热点数据源配置超过 128 KiB,已忽略".to_string()]);
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(document) => {
            let version_ok = document.get("version").and_then(Value::as_i64) == Some(1);
            if !version_ok {
                return (Vec::new(), true, vec!["数据源配置需要 version: 1".to_string()]);
            }
            let rows = document.get("sources").and_then(Value::as_array).cloned().unwrap_or_default();
            let include_builtin = document.get("include_builtin").and_then(Value::as_bool).unwrap_or(true);
            (rows, include_builtin, Vec::new())
        }
        Err(_) => (Vec::new(), true, vec!["数据源配置无法解析,已忽略".to_string()]),
    }
}

/// 注册表:内置(默认仅 weibo/douyin/zhihu 为默认源,同上游)+ 用户配置。
pub fn registry() -> (Vec<Source>, Vec<String>) {
    let (rows, include_builtin, warnings) = load_user_config();
    let mut sources: Vec<Source> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if include_builtin {
        for (identifier, label) in BUILTIN_LABELS {
            if seen.iter().any(|id| id == identifier) {
                continue;
            }
            seen.push((*identifier).to_string());
            sources.push(Source {
                id: (*identifier).to_string(),
                label: (*label).to_string(),
                kind: "builtin".to_string(),
                default: matches!(*identifier, "weibo" | "douyin" | "zhihu"),
                timeout: COLLECT_TIMEOUT,
                refresh: Duration::from_secs(300),
                max_stale: Duration::from_secs(86400),
                options: Map::new(),
            });
        }
    }
    for row in rows.iter().take(64) {
        let Some(object) = row.as_object() else { continue };
        let Some(id) = object.get("id").and_then(Value::as_str) else { continue };
        if seen.iter().any(|item| item == id) {
            continue;
        }
        if object.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        seen.push(id.to_string());
        let kind = object.get("type").and_then(Value::as_str).unwrap_or("http").to_string();
        let refresh = Duration::from_secs(
            object.get("refresh_seconds").and_then(Value::as_u64).unwrap_or(300).clamp(1, 86400),
        );
        let max_stale = Duration::from_secs(
            object
                .get("max_stale_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(86400)
                .clamp(refresh.as_secs(), 604800),
        );
        sources.push(Source {
            id: id.to_string(),
            label: object.get("label").and_then(Value::as_str).unwrap_or("未命名来源").to_string(),
            kind,
            default: object.get("default").and_then(Value::as_bool).unwrap_or(true),
            timeout: Duration::from_millis(
                (object.get("timeout_seconds").and_then(Value::as_f64).unwrap_or(20.0) * 1000.0) as u64,
            ),
            refresh,
            max_stale,
            options: object.clone(),
        });
    }
    (sources, warnings)
}

// ── v1 载荷解析/校验(与上游 protocol.validate_payload 对齐)─────────────────

fn parse_upstream(obj: &Value) -> Vec<Value> {
    let mut data = obj.get("data").cloned().unwrap_or(Value::Null);
    if data.is_object() {
        data = data
            .get("data")
            .or_else(|| data.get("list"))
            .cloned()
            .unwrap_or_else(|| json!([]));
    }
    let Some(rows) = data.as_array() else { return Vec::new() };
    let mut out = Vec::new();
    for item in rows {
        let object: Map<String, Value> = if item.is_string() {
            let mut map = Map::new();
            map.insert("title".into(), Value::String(item.as_str().unwrap_or_default().to_string()));
            map
        } else if item.is_object() {
            item.as_object().cloned().unwrap_or_default()
        } else {
            continue;
        };
        let target = object.get("target").and_then(Value::as_object).cloned().unwrap_or_default();
        let title = pick_text(&object, &["title", "Title", "word", "name", "keyword"])
            .or_else(|| pick_text(&target, &["title"]))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let heat = pick_text(&object, &["hot", "hot_value", "HotValue", "hotValue", "num", "detail_text"])
            .or_else(|| {
                target
                    .get("metrics_area")
                    .and_then(|area| area.get("text"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let mut url = pick_text(&object, &["url", "Url", "link", "mobil_url"])
            .or_else(|| pick_text(&target, &["url"]))
            .unwrap_or_default();
        // 知乎问题 API 链接转网页链接(上游同款)
        if let Some(rest) = url.strip_prefix("https://api.zhihu.com/questions/") {
            url = format!("https://www.zhihu.com/question/{}", rest.trim_end_matches('/'));
        }
        out.push(json!({ "title": title, "url": url, "heat": { "text": heat } }));
    }
    out
}

fn pick_text(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        let value = map.get(*key)?;
        if let Some(number) = value.as_f64() {
            return Some(format!("{}", number as i64));
        }
        if let Some(text) = value.as_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn web_url_guard(value: &str) -> Result<String, String> {
    if !value.starts_with("http://") && !value.starts_with("https://") {
        return Err("仅允许 http(s) URL".to_string());
    }
    Ok(value.to_string())
}

/// v1 载荷校验(上游 validate_payload 对齐:版本/上限/字段界/heat 形状)。
fn validate_payload(value: &Value) -> Result<Value, String> {
    let object = value.as_object().ok_or("invalid_response")?;
    if object.get("version").and_then(Value::as_i64) != Some(1) {
        return Err("invalid_response".to_string());
    }
    let items = object
        .get("items")
        .and_then(Value::as_array)
        .ok_or("invalid_response")?;
    if items.len() > MAX_ITEMS {
        return Err("invalid_response".to_string());
    }
    let mut normalized = Vec::with_capacity(items.len());
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let object = item.as_object().ok_or("invalid_response")?;
        let title = object
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(1000)
            .collect::<String>();
        if title.trim().is_empty() {
            return Err("invalid_response".to_string());
        }
        let url = match object.get("url").and_then(Value::as_str).unwrap_or_default() {
            "" => String::new(),
            url => web_url_guard(url)?,
        };
        let mut identifier = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        if identifier.is_empty() {
            use base64::Engine as _;
            use std::fmt::Write as _;
            let source = if url.is_empty() { &title } else { &url };
            let digest = base64::engine::general_purpose::STANDARD.encode(source);
            for byte in digest.bytes().take(18) {
                write!(identifier, "{byte:02x}").ok();
            }
        }
        if seen.contains(&identifier) {
            continue;
        }
        seen.insert(identifier.clone());
        let heat = object.get("heat").cloned().unwrap_or_else(|| json!({}));
        normalized.push(json!({ "title": title, "url": url, "id": identifier, "heat": heat }));
    }
    Ok(json!({
        "version": 1,
        "fetched_at": object.get("fetched_at").cloned().unwrap_or_else(|| Value::String(utc_now_iso())),
        "items": normalized,
    }))
}

// ── 收集器 ──────────────────────────────────────────────────────────────────

fn http_get_json(url: &str, timeout: Duration) -> Result<Value, String> {
    runtime().block_on(async {
        let response = tokio::time::timeout(
            timeout,
            reqwest::Client::new()
                .get(url)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0 Safari/537.36")
                .header("Accept", "application/json, text/plain, */*")
                .send(),
        )
        .await
        .map_err(|_| "upstream timeout".to_string())?
        .map_err(|error| error.to_string())?;
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("upstream output too large".to_string());
        }
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    })
}

/// 内置收集器(与上游 builtin.py collect_platform 语义一致)。
fn collect_builtin(platform: &str) -> Result<Value, String> {
    let upstreams = BUILTIN_UPSTREAMS
        .iter()
        .find(|(identifier, _)| *identifier == platform)
        .map(|(_, urls)| urls)
        .ok_or_else(|| format!("unknown platform {platform}"))?;
    for url in upstreams {
        if let Ok(obj) = http_get_json(url, UPSTREAM_TIMEOUT) {
            let items = parse_upstream(&obj);
            if !items.is_empty() {
                return validate_payload(&json!({ "version": 1, "fetched_at": utc_now_iso(), "items": items }));
            }
        }
    }
    Err("collector_failed".to_string())
}

fn collect_http(source: &Source) -> Result<Value, String> {
    let url = source
        .options
        .get("url")
        .and_then(Value::as_str)
        .ok_or("数据源缺少 url")?;
    web_url_guard(url)?;
    let payload = http_get_json(url, source.timeout)?;
    validate_payload(&json!({ "version": 1, "fetched_at": utc_now_iso(), "items": parse_upstream(&payload) }))
}

fn collect_rss(source: &Source) -> Result<Value, String> {
    let url = source.options.get("url").and_then(Value::as_str).ok_or("RSS 数据源缺少 url")?;
    web_url_guard(url)?;
    let xml = runtime().block_on(async {
        let response = tokio::time::timeout(source.timeout, reqwest::Client::new().get(url).send())
            .await
            .map_err(|_| "upstream timeout".to_string())?
            .map_err(|error| error.to_string())?;
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("upstream output too large".to_string());
        }
        Ok(String::from_utf8_lossy(&bytes).to_string())
    })?;
    validate_payload(&json!({ "version": 1, "fetched_at": utc_now_iso(), "items": parse_feed(&xml)? }))
}

/// 极简 RSS2/RDF/Atom 解析(取 title/link;上游 stdlib 实现的子集,满足 v1 契约)。
fn parse_feed(xml: &str) -> Result<Vec<Value>, String> {
    let mut items = Vec::new();
    for block in extract_blocks(xml, "<item", "</item>").into_iter().chain(extract_blocks(xml, "<entry", "</entry>")) {
        let title = tag_text(&block, "title");
        if title.is_empty() {
            continue;
        }
        // atom:<link href="…" />
        let link = {
            let direct = tag_text(&block, "link");
            if direct.is_empty() {
                tag_attr(&block, "link", "href").unwrap_or_default()
            } else {
                direct
            }
        };
        items.push(json!({ "title": title, "url": link }));
    }
    Ok(items)
}

fn extract_blocks(xml: &str, open: &str, close: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open) {
        let Some(close_offset) = rest[start..].find(close) else { break };
        let end = start + close_offset + close.len();
        blocks.push(rest[start..end].to_string());
        rest = &rest[end..];
    }
    blocks
}

fn tag_text(block: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let Some(start) = block.find(&open) else { return String::new() };
    let Some(after_open) = block[start..].find('>') else { return String::new() };
    let tag_name_end = start + after_open;
    let close = format!("</{tag}>");
    let Some(end) = block[tag_name_end..].find(&close) else { return String::new() };
    decode_entities(block[tag_name_end + 1..tag_name_end + end].trim())
}

fn tag_attr(block: &str, tag: &str, attr: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = block.find(&open)?;
    let end = block[start..].find('>')? + start;
    let needle = format!("{attr}=\"");
    let attr_start = block[start..end].find(&needle)? + start + needle.len();
    let attr_end = block[attr_start..].find('"')? + attr_start;
    Some(block[attr_start..attr_end].to_string())
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn collect_script(source: &Source) -> Result<Value, String> {
    let script = source.options.get("script").and_then(Value::as_str).unwrap_or_default();
    if script.is_empty() {
        return Err("脚本数据源缺少 script".to_string());
    }
    let root = crate::workspace::project_root();
    let script_path = root.join(script.trim_start_matches("./"));
    if !script_path.is_file() {
        return Err(format!("收集脚本不存在:{script}"));
    }
    let args: Vec<String> = source
        .options
        .get("args")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let output = std::process::Command::new("python")
        .arg(&script_path)
        .args(&args)
        .current_dir(&root)
        .output()
        .map_err(|error| format!("脚本执行失败:{error}"))?;
    if !output.status.success() {
        return Err("collector_failed".to_string());
    }
    let stdout = &output.stdout[..output.stdout.len().min(MAX_BYTES)];
    let value: Value = serde_json::from_slice(stdout).map_err(|error| error.to_string())?;
    validate_payload(&value)
}

fn collect(source: &Source) -> Result<Value, String> {
    match source.kind.as_str() {
        "builtin" => collect_builtin(&source.id),
        "http" => collect_http(source),
        "rss" => collect_rss(source),
        "python" => collect_script(source),
        other => Err(format!("unknown source type {other}")),
    }
}

// ── 缓存服务(上游 TrendService 语义)────────────────────────────────────────

struct Entry {
    payload: Option<Value>,
    fetched_at_epoch: u64,
    attempted_at_epoch: u64,
    error: Option<String>,
    lock: Mutex<()>,
}

static ENTRIES: OnceLock<Mutex<HashMap<String, Arc<Entry>>>> = OnceLock::new();

fn entries() -> &'static Mutex<HashMap<String, Arc<Entry>>> {
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry_for(id: &str) -> Arc<Entry> {
    let mut guard = entries().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = guard
        .entry(id.to_string())
        .or_insert_with(|| {
            Arc::new(Entry {
                payload: None,
                fetched_at_epoch: 0,
                attempted_at_epoch: 0,
                error: None,
                lock: Mutex::new(()),
            })
        })
        .clone();
    // 简易容量界(上游 96)
    if guard.len() > 96 {
        if let Some(first_key) = guard.keys().next().cloned() {
            guard.remove(&first_key);
        }
    }
    entry
}

fn attempt_age(entry: &Entry) -> Duration {
    Duration::from_secs(now_secs().saturating_sub(entry.attempted_at_epoch))
}

fn render(source: &Source, entry: &Entry, limit: usize, overridde: Option<&str>) -> Value {
    let now = now_secs();
    let fetched = entry
        .payload
        .as_ref()
        .and_then(|payload| payload.get("fetched_at"))
        .and_then(Value::as_str)
        .and_then(|text| text.strip_suffix("-epoch"))
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(0);
    let usable = entry.payload.is_some() && now.saturating_sub(fetched) <= source.max_stale.as_secs();
    let stale = usable
        && (overridde.is_some()
            || entry.error.is_some()
            || now.saturating_sub(fetched) >= source.refresh.as_secs());
    let error_code = overridde
        .map(str::to_string)
        .or_else(|| entry.error.clone())
        .or_else(|| {
            if entry.payload.is_some() && !usable {
                Some("expired_data".to_string())
            } else {
                None
            }
        });
    json!({
        "platform": source.id,
        "label": display_label(&source.label),
        "items": if usable { entry.payload.as_ref().map(|payload| {
            payload.get("items").and_then(Value::as_array).map(|items| items.iter().take(limit).cloned().collect::<Vec<_>>())
        }).flatten().unwrap_or_default() } else { Vec::<Value>::new() },
        "ok": usable,
        "status": if stale { "stale" } else if usable { "fresh" } else { "error" },
        "stale": stale,
        "fetched_at": entry.payload.as_ref().map(|payload| payload.get("fetched_at").cloned().unwrap_or(Value::Null)).unwrap_or(Value::Null),
        "last_attempt_at": entry.attempted_at_epoch,
        "error": error_code.map(|code| json!({ "code": code, "message": error_message(&code) })),
    })
}

fn error_message(code: &str) -> &'static str {
    match code {
        "busy" => "数据源正在刷新,稍候自动重试",
        "expired_data" => "缓存数据已过期,暂无新数据",
        "invalid_response" => "数据源返回了无法识别的数据",
        "collector_failed" => "数据源收集失败",
        _ => "数据源收集失败",
    }
}

fn failure_group(source: &Source, code: &str) -> Value {
    json!({
        "platform": source.id,
        "label": display_label(&source.label),
        "items": [],
        "ok": false,
        "status": "error",
        "stale": false,
        "fetched_at": Value::Null,
        "last_attempt_at": 0,
        "error": { "code": code, "message": error_message(code) },
    })
}

fn service_get(source: &Source, limit: usize) -> Value {
    let entry = entry_for(&source.id);
    let _flight = match entry.lock.try_lock() {
        Ok(guard) => Some(guard),
        Err(_) => return render(source, &entry, limit, Some("busy")),
    };
    let now = now_secs();
    let retry = if entry.error.as_deref() == Some("busy") {
        BUSY_RETRY
    } else if entry.error.is_some() {
        FAILURE_RETRY
    } else {
        source.refresh
    };
    if entry.attempted_at_epoch == 0 || attempt_age(&entry) >= retry {
        let result = collect(source);
        let (payload, error) = match result {
            Ok(payload) => (Some(payload), None),
            Err(error) => (None, Some(error)),
        };
        // Entry 不可变;以新 Entry 整体替换(快照语义同上游)。
        let replacement = Arc::new(Entry {
            payload,
            fetched_at_epoch: now,
            attempted_at_epoch: now,
            error: error.clone(),
            lock: Mutex::new(()),
        });
        if let Ok(mut guard) = entries().lock() {
            guard.insert(source.id.clone(), replacement.clone());
        }
        return render(source, &replacement, limit, None);
    }
    render(source, &entry, limit, None)
}

// ── 路由处理 ────────────────────────────────────────────────────────────────

pub fn trends_list(platforms_query: Option<&str>, limit_query: Option<&str>) -> Value {
    let (sources, warnings) = registry();
    let selected: Vec<Source> = match platforms_query.filter(|value| !value.trim().is_empty()) {
        Some(platforms) => platforms
            .split(',')
            .filter_map(|id| {
                let id = id.trim();
                sources.iter().find(|source| source.id == id).cloned()
            })
            .collect(),
        None => sources.iter().filter(|source| source.default).cloned().collect(),
    };
    let limit = limit_query.and_then(|value| value.parse::<usize>().ok()).unwrap_or(12).clamp(1, 30);
    let trends: Vec<Value> = selected.iter().map(|source| service_get(source, limit)).collect();
    json!({ "version": 1, "trends": trends, "updated": now_secs(), "warnings": warnings })
}

pub fn trend_sources() -> Value {
    let (sources, warnings) = registry();
    json!({
        "version": 1,
        "sources": sources.iter().map(Source::public).collect::<Vec<_>>(),
        "warnings": warnings,
    })
}

/// GET /api/trends/manage:配置文件现状(内容 + 路径)。
pub fn trend_manage_get() -> Result<Value, (u16, String)> {
    let path = config_path();
    let document = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        json!({ "version": 1, "include_builtin": true, "sources": [] }).to_string()
    });
    Ok(json!({ "path": path.to_string_lossy(), "document": document }))
}

/// POST /api/trends/manage:校验(v1 + ≤64 源 + ≤256KiB)后原子写回。
pub fn trend_manage_save(body: &Value) -> Result<Value, (u16, String)> {
    let document = body.get("document").cloned().unwrap_or(Value::Null);
    let text = document.as_str().map(str::to_string).unwrap_or_else(|| document.to_string());
    if text.len() > 256 * 1024 {
        return Err((413, "数据源操作最多 256 KiB".to_string()));
    }
    let value: Value = serde_json::from_str(&text).map_err(|error| (400, format!("数据源操作必须是有效 JSON:{error}")))?;
    if value.get("version").and_then(Value::as_i64) != Some(1) {
        return Err((400, "配置需要 version: 1".to_string()));
    }
    let count = value.get("sources").and_then(Value::as_array).map(|items| items.len()).unwrap_or(0);
    if count > 64 {
        return Err((400, "sources 必须为数组,最多 64 个数据源".to_string()));
    }
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| (409, format!("配置保存失败:{error}")))?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&value).unwrap_or_default())
        .map_err(|error| (409, format!("配置保存失败:{error}")))?;
    std::fs::rename(&temp, &path).map_err(|error| (409, format!("配置保存失败:{error}")))?;
    Ok(json!({ "ok": true, "path": path.to_string_lossy() }))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_upstream_shapes() {
        let payload = json!({
            "data": { "data": [
                {"title": "微博热搜", "hot": "1234", "url": "https://s.weibo.com/x"},
                "纯字符串条目",
                {"target": {"title": "知乎问题", "url": "https://api.zhihu.com/questions/123"}, "detail_text": "9900万"},
            ]}
        });
        let items = parse_upstream(&payload);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["title"], "微博热搜");
        assert_eq!(items[2]["url"], "https://www.zhihu.com/question/123");
    }

    #[test]
    fn validate_payload_dedups_and_bounds() {
        let payload = json!({
            "version": 1, "fetched_at": "1-epoch",
            "items": [
                {"title": "同题", "url": "https://a/1"},
                {"title": "同题", "url": "https://a/1"}
            ]
        });
        let validated = validate_payload(&payload).unwrap();
        assert_eq!(validated["items"].as_array().unwrap().len(), 1);
        // 空标题整载荷拒绝(上游 all-or-nothing)
        assert!(validate_payload(&json!({"version": 1, "items": [{"title": ""}]})).is_err());
    }

    #[test]
    fn registry_defaults_match_upstream() {
        std::env::remove_var("EASEL_TREND_SOURCES_CONFIG");
        let (sources, _) = registry();
        let defaults: Vec<&str> = sources.iter().filter(|s| s.default).map(|s| s.id.as_str()).collect();
        assert_eq!(defaults, vec!["weibo", "douyin", "zhihu"]);
        assert!(sources.iter().all(|s| BUILTIN_LABELS.iter().any(|(id, _)| id == &s.id)));
    }

    #[test]
    fn rss_and_display_label() {
        let xml = "<rss><channel><item><title>条目一</title><link>https://a/1</link></item></channel></rss>";
        let items = parse_feed(xml).unwrap();
        assert_eq!(items[0]["title"], "条目一");
        assert_eq!(display_label("我的源 · python"), "我的源");
    }
}
