//! 内容日历(outputs/_schedule.json)与选题库(outputs/_ideas.json):
//! 逐字段复刻上游 app.py 的 ScheduleItem/IdeaItem CRUD(原子写、枚举兜底)。

use serde_json::{json, Map, Value};
use std::fs;
use std::sync::atomic::Ordering;
use std::path::PathBuf;

const SCHEDULE_STATUSES: &[&str] = &["idea", "draft", "scheduled", "published"];
const SCHEDULE_KINDS: &[&str] = &["content", "event"];
const SCHEDULE_SOURCES: &[&str] = &["manual", "publish-page", "chat", "scheduler"];
const IDEA_STATUSES: &[&str] = &["pending", "doing", "done"];

fn outputs_dir() -> PathBuf {
    crate::workspace::ensure_ready().unwrap_or_else(|_| crate::workspace::project_root()).join("outputs")
}

fn schedule_file() -> PathBuf {
    outputs_dir().join("_schedule.json")
}

fn ideas_file() -> PathBuf {
    outputs_dir().join("_ideas.json")
}

fn read_array(path: &PathBuf) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

/// 原子写(tmp + rename,上游同款防丢)。
fn write_array(path: &PathBuf, items: &[Value]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(items).unwrap_or_default()).map_err(|error| error.to_string())?;
    fs::rename(&temp, path).map_err(|error| error.to_string())
}

fn id12() -> String {
    // 12 位十六进制(上游 uuid4().hex[:12] 同形):时间 + 计数器足够本地唯一
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:012x}", (now as u64) ^ (counter << 32))
}

fn enum_or<'a>(value: Option<&'a str>, allowed: &[&str], fallback: &'a str) -> &'a str {
    match value {
        Some(text) if allowed.contains(&text) => text,
        _ => fallback,
    }
}

fn text_of(object: &Map<String, Value>, key: &str) -> String {
    object.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

// ── schedule ────────────────────────────────────────────────────────────────

pub fn schedule_list() -> Vec<Value> {
    read_array(&schedule_file())
}

pub fn schedule_create(body: &Value) -> Result<Value, (u16, String)> {
    let Some(object) = body.as_object() else {
        return Err((400, "请求体必须是对象".to_string()));
    };
    let kind = enum_or(object.get("kind").and_then(Value::as_str), SCHEDULE_KINDS, "content");
    let mut item = Map::new();
    item.insert("id".into(), json!(id12()));
    let title = text_of(object, "title").trim().to_string();
    let resolved_title = if title.is_empty() {
        if kind == "event" { "未命名活动".to_string() } else { "未命名".to_string() }
    } else {
        title
    };
    item.insert("title".into(), json!(resolved_title));
    item.insert("date".into(), json!(text_of(object, "date")));
    item.insert("platform".into(), json!(text_of(object, "platform")));
    item.insert("time".into(), json!(text_of(object, "time")));
    item.insert(
        "status".into(),
        json!(enum_or(object.get("status").and_then(Value::as_str), SCHEDULE_STATUSES, "idea")),
    );
    item.insert("note".into(), json!(text_of(object, "note")));
    item.insert("kind".into(), json!(kind));
    item.insert("url".into(), json!(text_of(object, "url")));
    item.insert(
        "source".into(),
        json!(enum_or(object.get("source").and_then(Value::as_str), SCHEDULE_SOURCES, "manual")),
    );
    item.insert("event_type".into(), json!(text_of(object, "event_type")));
    item.insert("end_date".into(), json!(text_of(object, "end_date")));

    let mut items = schedule_list();
    items.push(Value::Object(item.clone()));
    write_array(&schedule_file(), &items).map_err(|error| (500, error))?;
    Ok(Value::Object(item))
}

pub fn schedule_update(sid: &str, body: &Value) -> Result<Value, (u16, String)> {
    let Some(object) = body.as_object() else {
        return Err((400, "请求体必须是对象".to_string()));
    };
    let mut items = schedule_list();
    for item in items.iter_mut() {
        let Some(map) = item.as_object_mut() else { continue };
        if map.get("id").and_then(Value::as_str) != Some(sid) {
            continue;
        }
        let kind = enum_or(object.get("kind").and_then(Value::as_str), SCHEDULE_KINDS, "content");
        let title = text_of(object, "title").trim().to_string();
        let title = if title.is_empty() { text_of(map, "title") } else { title };
        map.insert("title".into(), json!(title));
        map.insert("date".into(), json!(text_of(object, "date")));
        map.insert("platform".into(), json!(text_of(object, "platform")));
        map.insert("time".into(), json!(text_of(object, "time")));
        map.insert(
            "status".into(),
            json!(enum_or(object.get("status").and_then(Value::as_str), SCHEDULE_STATUSES, "idea")),
        );
        map.insert("note".into(), json!(text_of(object, "note")));
        map.insert("kind".into(), json!(kind));
        map.insert("url".into(), json!(text_of(object, "url")));
        map.insert("event_type".into(), json!(text_of(object, "event_type")));
        map.insert("end_date".into(), json!(text_of(object, "end_date")));
        let updated = Value::Object(map.clone());
        write_array(&schedule_file(), &items).map_err(|error| (500, error))?;
        return Ok(updated);
    }
    Err((404, "排期不存在".to_string()))
}

pub fn schedule_delete(sid: &str) -> Result<Value, (u16, String)> {
    let items = schedule_list();
    let kept: Vec<Value> = items
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) != Some(sid))
        .cloned()
        .collect();
    if kept.len() == items.len() {
        return Err((404, "排期不存在".to_string()));
    }
    write_array(&schedule_file(), &kept).map_err(|error| (500, error))?;
    Ok(json!({ "ok": true, "deleted": sid }))
}

/// GET /api/schedule/context:规划摘要,上游薄封装 calendar_ops.py(Python 脚本原样)。
pub fn schedule_context(days: u32) -> Value {
    let script = crate::workspace::project_root().join("skills").join("shared").join("scripts").join("calendar_ops.py");
    let data = schedule_file();
    if !script.is_file() {
        return json!({});
    }
    let days = days.clamp(1, 90);
    match std::process::Command::new("python")
        .arg(&script)
        .arg("--data")
        .arg(&data)
        .arg("context")
        .arg("--days")
        .arg(days.to_string())
        .current_dir(crate::workspace::project_root())
        .output()
    {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => {
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| json!({}))
        }
        _ => json!({}),
    }
}

// ── ideas ───────────────────────────────────────────────────────────────────

pub fn ideas_list() -> Vec<Value> {
    read_array(&ideas_file())
}

pub fn ideas_create(body: &Value) -> Result<Value, (u16, String)> {
    let Some(object) = body.as_object() else {
        return Err((400, "请求体必须是对象".to_string()));
    };
    let title = text_of(object, "title").trim().to_string();
    let item = json!({
        "id": id12(),
        "title": if title.is_empty() { "未命名选题".to_string() } else { title },
        "note": text_of(object, "note"),
        "source": text_of(object, "source"),
        "status": enum_or(object.get("status").and_then(Value::as_str), IDEA_STATUSES, "pending"),
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0),
    });
    let mut items = ideas_list();
    items.insert(0, item.clone());
    write_array(&ideas_file(), &items).map_err(|error| (500, error))?;
    Ok(item)
}

pub fn ideas_update(iid: &str, body: &Value) -> Result<Value, (u16, String)> {
    let Some(object) = body.as_object() else {
        return Err((400, "请求体必须是对象".to_string()));
    };
    let mut items = ideas_list();
    for item in items.iter_mut() {
        let Some(map) = item.as_object_mut() else { continue };
        if map.get("id").and_then(Value::as_str) != Some(iid) {
            continue;
        }
        let title = text_of(object, "title").trim().to_string();
        if !title.is_empty() {
            map.insert("title".into(), json!(title));
        } else if !map.contains_key("title") {
            map.insert("title".into(), json!("未命名选题"));
        }
        map.insert("note".into(), json!(text_of(object, "note")));
        map.insert("source".into(), json!(text_of(object, "source")));
        let status = object.get("status").and_then(Value::as_str).unwrap_or_default();
        let status = if IDEA_STATUSES.contains(&status) { status } else { map.get("status").and_then(Value::as_str).unwrap_or("pending") };
        map.insert("status".into(), json!(status));
        let updated = Value::Object(map.clone());
        write_array(&ideas_file(), &items).map_err(|error| (500, error))?;
        return Ok(updated);
    }
    Err((404, "选题不存在".to_string()))
}

pub fn ideas_delete(iid: &str) -> Result<Value, (u16, String)> {
    let items = ideas_list();
    let kept: Vec<Value> = items
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) != Some(iid))
        .cloned()
        .collect();
    if kept.len() == items.len() {
        return Err((404, "选题不存在".to_string()));
    }
    write_array(&ideas_file(), &kept).map_err(|error| (500, error))?;
    Ok(json!({ "ok": true, "deleted": iid }))
}
