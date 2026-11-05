//! Output storage and media transport shared by the desktop and remote CMD paths.

use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::router::ApiError;

pub const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;
const CHUNK_BYTES: usize = 1024 * 1024;
const MAX_BATCH_FILES: usize = 200;
static UPLOAD_LOCK: Mutex<()> = Mutex::new(());

pub type FileResult = Result<Value, ApiError>;

fn error(status: u16, detail: impl Into<String>) -> ApiError {
    ApiError { status, detail: detail.into() }
}

fn io_error(cause: std::io::Error) -> ApiError {
    error(500, cause.to_string())
}

fn outputs_root() -> Result<PathBuf, ApiError> {
    let root = crate::workspace::ensure_ready().map_err(|cause| error(500, cause))?.join("outputs");
    fs::create_dir_all(&root).map_err(io_error)?;
    root.canonicalize().map_err(io_error)
}

pub fn relative_path(rel: &str) -> Result<PathBuf, ApiError> {
    if rel.is_empty()
        || rel.starts_with('/')
        || rel.contains('\\')
        || rel.contains(':')
        || rel.chars().any(|character| character.is_control())
    {
        return Err(error(403, "非法路径"));
    }
    let path = PathBuf::from(rel);
    if path.components().any(|component| !matches!(component, Component::Normal(_))) {
        return Err(error(403, "非法路径"));
    }
    if rel.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..") {
        return Err(error(403, "非法路径"));
    }
    Ok(path)
}

pub fn output_path(rel: &str) -> Result<PathBuf, ApiError> {
    let root = outputs_root()?;
    let requested = root.join(relative_path(rel)?);
    let path = requested.canonicalize().map_err(|cause| {
        if cause.kind() == std::io::ErrorKind::NotFound { error(404, "文件不存在") } else { io_error(cause) }
    })?;
    if path == root || !path.starts_with(&root) {
        return Err(error(403, "非法路径"));
    }
    Ok(path)
}

fn safe_directory(root: &Path, relative: &Path) -> Result<PathBuf, ApiError> {
    let mut dir = root.to_path_buf();
    for segment in relative.components() {
        let Component::Normal(name) = segment else { return Err(error(403, "非法路径")) };
        dir.push(name);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err(error(403, "非法目录")),
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&dir).map_err(io_error)?,
            Err(cause) => return Err(io_error(cause)),
        }
    }
    Ok(dir)
}

pub fn file_kind(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" => "image",
        "mp4" | "mov" | "webm" | "m4v" => "video",
        "mp3" | "wav" | "m4a" | "aac" | "ogg" | "flac" => "audio",
        "txt" | "md" | "json" | "csv" | "log" | "py" | "js" | "ts" | "html" | "htm" | "css" | "xml" | "yaml" | "yml" | "srt" | "vtt" => "text",
        _ => "binary",
    }
}

pub fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase().as_str() {
        "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp", "svg" => "image/svg+xml", "bmp" => "image/bmp", "avif" => "image/avif", "ico" => "image/x-icon",
        "mp4" | "m4v" => "video/mp4", "mov" => "video/quicktime", "webm" => "video/webm",
        "mp3" => "audio/mpeg", "wav" => "audio/wav", "ogg" => "audio/ogg", "m4a" => "audio/mp4", "aac" => "audio/aac", "flac" => "audio/flac",
        "html" | "htm" => "text/html", "css" => "text/css", "js" => "text/javascript", "json" => "application/json", "pdf" => "application/pdf", "zip" => "application/zip",
        "txt" | "md" | "srt" | "vtt" | "csv" | "log" | "xml" | "yaml" | "yml" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn modified(path: &Path) -> u64 {
    fs::metadata(path).ok().and_then(|meta| meta.modified().ok()).and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok()).map(|date| date.as_secs()).unwrap_or(0)
}

fn build_node(root: &Path, path: &Path) -> Result<Value, ApiError> {
    let actual = path.canonicalize().map_err(io_error)?;
    if !actual.starts_with(root) { return Err(error(403, "非法路径")) }
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let rel = path.strip_prefix(root).map_err(|_| error(403, "非法路径"))?.to_string_lossy().replace('\\', "/");
    if path.is_dir() {
        let mut entries = fs::read_dir(path).map_err(io_error)?.collect::<Result<Vec<_>, _>>().map_err(io_error)?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut children = Vec::new();
        for entry in entries {
            if entry.file_name().to_string_lossy().starts_with('.') || entry.file_type().map_err(io_error)?.is_symlink() { continue }
            children.push(build_node(root, &entry.path())?);
        }
        let mtime = children.iter().filter_map(|node| node["mtime"].as_u64()).max().unwrap_or_else(|| modified(path));
        let count: u64 = children.iter().map(|node| if node["type"] == "dir" { node["fileCount"].as_u64().unwrap_or(0) } else { 1 }).sum();
        Ok(json!({"name":name,"type":"dir","path":rel,"mtime":mtime,"children":children,"fileCount":count}))
    } else {
        Ok(json!({"name":name,"type":"file","path":rel,"mtime":modified(path),"kind":file_kind(path),"size":fs::metadata(path).map_err(io_error)?.len()}))
    }
}

fn project_meta(root: &Path, project: &Path) -> Value {
    let data = fs::read_to_string(project.join(".easel.json")).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or(Value::Null);
    let mut meta = Map::new();
    for key in ["title", "summary", "platform", "kind", "status", "tags", "deliverables"] {
        if let Some(value) = data.get(key).filter(|value| !value.is_null() && **value != json!("") && **value != json!([])) {
            meta.insert(key.into(), value.clone());
        }
    }
    if meta.is_empty() { return Value::Null }
    let resolve = |name: &str| -> Option<String> {
        let path = project.join(relative_path(name).ok()?);
        let canonical = path.canonicalize().ok()?;
        if !canonical.starts_with(project) || !canonical.is_file() { return None }
        Some(path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/"))
    };
    let deliverables: Vec<&str> = data.get("deliverables").and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let cover = data.get("cover").and_then(Value::as_str).and_then(resolve).or_else(|| deliverables.iter().filter(|name| matches!(file_kind(Path::new(name)), "image" | "video")).find_map(|name| resolve(name)));
    if let Some(cover) = cover { meta.insert("cover".into(), json!(cover)); }
    meta.insert("deliverablePaths".into(), json!(deliverables.iter().filter_map(|name| resolve(name)).collect::<Vec<_>>()));
    Value::Object(meta)
}

pub fn outputs_tree() -> FileResult {
    let root = outputs_root()?;
    let mut entries = fs::read_dir(&root).map_err(io_error)?.collect::<Result<Vec<_>, _>>().map_err(io_error)?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut items = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(['.', '_']) || name == "analytics" || !entry.file_type().map_err(io_error)?.is_dir() { continue }
        let mut node = build_node(&root, &entry.path())?;
        let meta = project_meta(&root, &entry.path());
        if !meta.is_null() { node["meta"] = meta; }
        items.push(node);
    }
    items.sort_by_key(|node| std::cmp::Reverse(node["mtime"].as_u64().unwrap_or(0)));
    Ok(json!(items))
}

pub fn output_content(rel: &str) -> FileResult {
    let path = output_path(rel)?;
    if !path.is_file() { return Err(error(404, "文件不存在")) }
    let kind = file_kind(&path);
    if kind != "text" { return Ok(json!({"path":rel,"content":"","kind":kind,"isBinary":true})) }
    let bytes = fs::read(&path).map_err(io_error)?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(json!({"path":rel,"content":text,"kind":"text","isBinary":false})),
        Err(_) => Ok(json!({"path":rel,"content":"","kind":"binary","isBinary":true})),
    }
}

pub fn output_delete(rel: &str) -> FileResult {
    let relative = relative_path(rel)?;
    let first = relative.components().next().unwrap().as_os_str().to_string_lossy().to_string();
    if first.starts_with(['_', '.']) || matches!(first.to_ascii_lowercase().as_str(), "analytics" | "wechat") {
        return Err(error(403, "系统数据受保护，不可从内容库删除"));
    }
    let path = output_path(rel)?;
    let root = outputs_root()?;
    let resolved_first = path.strip_prefix(&root).map_err(|_| error(403, "非法路径"))?.components().next().unwrap().as_os_str().to_string_lossy().to_string();
    if resolved_first.starts_with(['_', '.']) || matches!(resolved_first.to_ascii_lowercase().as_str(), "analytics" | "wechat") {
        return Err(error(403, "系统数据受保护，不可从内容库删除"));
    }
    let is_dir = path.is_dir();
    if is_dir { fs::remove_dir_all(path).map_err(io_error)? } else { fs::remove_file(path).map_err(io_error)? }
    Ok(json!({"ok":true,"deleted":rel,"kind":if is_dir {"dir"} else {"file"}}))
}

pub fn media(rel: &str, offset: Option<u64>, length: Option<usize>) -> FileResult {
    let path = output_path(rel)?;
    if !path.is_file() { return Err(error(404, "文件不存在")) }
    let size = fs::metadata(&path).map_err(io_error)?.len();
    if let Some(offset) = offset {
        if offset > size { return Err(error(416, "读取位置超出文件范围")) }
        let length = length.unwrap_or(CHUNK_BYTES).clamp(1, CHUNK_BYTES);
        let mut file = fs::File::open(&path).map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        let mut bytes = vec![0; length.min((size - offset) as usize)];
        file.read_exact(&mut bytes).map_err(io_error)?;
        return Ok(json!({"path":rel,"offset":offset,"contentBase64":base64::engine::general_purpose::STANDARD.encode(&bytes),"eof":offset + bytes.len() as u64 == size}));
    }
    Ok(json!({"path":rel,"absolutePath":path.to_string_lossy(),"mimeType":mime_type(&path),"size":size,"mtime":modified(&path),"chunkSize":CHUNK_BYTES}))
}

pub fn attachment_scope(session: &str) -> Result<String, ApiError> {
    let session = session.trim();
    if session.is_empty() || session.chars().count() > 256 { return Err(error(400, "无效的会话标识")) }
    Ok(format!("{:x}", Sha256::digest(session.as_bytes()))[..20].to_string())
}

fn attachment_id(scope: &str, path: &str) -> String {
    format!("{:x}", Sha256::digest(format!("{scope}\0{path}").as_bytes()))[..24].to_string()
}

fn supported_upload(path: &Path) -> bool {
    matches!(path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "mp4" | "mov" | "webm" | "m4v" |
        "pdf" | "txt" | "md" | "markdown" | "csv" | "json" | "srt" | "vtt" | "docx" | "doc" | "xlsx" | "xls" | "pptx" | "ppt" |
        "mp3" | "wav" | "m4a")
}

pub fn upload(body: &Value, local: bool) -> FileResult {
    let _guard = UPLOAD_LOCK.lock().map_err(|_| error(500, "上传状态不可用"))?;
    let session = body.pointer("/fields/sessionId").and_then(Value::as_str).or_else(|| body.get("sessionId").and_then(Value::as_str)).ok_or_else(|| error(400, "附件必须绑定到会话"))?;
    let scope = attachment_scope(session)?;
    let files = body.get("files").and_then(Value::as_array).ok_or_else(|| error(400, "没有文件"))?;
    if files.is_empty() || files.len() > MAX_BATCH_FILES { return Err(error(400, "上传文件数量无效")) }
    let root = outputs_root()?;
    let batch = format!("{}-{}", chrono::Local::now().format("%Y%m%d"), &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let dir = safe_directory(&root, &PathBuf::from("_inbox").join(&scope).join(&batch))?;
    let mut written = Vec::new();
    let result = (|| -> FileResult {
        let mut saved = Vec::new();
        for item in files {
            let raw = item.get("name").and_then(Value::as_str).unwrap_or("file");
            let name = raw.rsplit(['/', '\\']).next().filter(|name| !name.is_empty()).unwrap_or("file");
            if name.starts_with('.') || name.chars().any(|character| character.is_control()) || name.contains(':') { return Err(error(400, "文件名非法")) }
            let filename = Path::new(name);
            if !supported_upload(filename) { return Err(error(400, format!("不支持的文件类型：{name}"))) }
            let content = item.get("contentBase64").and_then(Value::as_str).ok_or_else(|| error(400, "缺少文件内容"))?;
            if !local && content.len() > (MAX_UPLOAD_BYTES + 2) / 3 * 4 { return Err(error(413, format!("{name} 超过 50MB 上限"))) }
            let bytes = base64::engine::general_purpose::STANDARD.decode(content).map_err(|_| error(400, "文件内容不是有效 base64"))?;
            if !local && bytes.len() > MAX_UPLOAD_BYTES { return Err(error(413, format!("{name} 超过 50MB 上限"))) }
            let mut target = dir.join(name);
            let mut duplicate = 2;
            while target.exists() {
                let stem = filename.file_stem().unwrap_or_default().to_string_lossy();
                let suffix = filename.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default();
                target = dir.join(format!("{stem} ({duplicate}){suffix}"));
                duplicate += 1;
            }
            let mut file = OpenOptions::new().create_new(true).write(true).open(&target).map_err(io_error)?;
            written.push(target.clone());
            file.write_all(&bytes).map_err(io_error)?;
            let rel = target.strip_prefix(&root).map_err(|_| error(403, "非法路径"))?.to_string_lossy().replace('\\', "/");
            saved.push(json!({"id":attachment_id(&scope,&rel),"name":target.file_name().unwrap().to_string_lossy(),"path":rel}));
        }
        Ok(json!({"ok":true,"files":saved}))
    })();
    if result.is_err() { for path in written { let _ = fs::remove_file(path); } }
    result
}

struct UploadFileState {
    path: PathBuf,
    relative: String,
    name: String,
    expected: u64,
    received: u64,
}

struct UploadBatch {
    root: PathBuf,
    scope: String,
    files: Vec<UploadFileState>,
    touched: std::time::Instant,
}

static UPLOADS: std::sync::OnceLock<Mutex<std::collections::HashMap<String, UploadBatch>>> = std::sync::OnceLock::new();

fn uploads() -> &'static Mutex<std::collections::HashMap<String, UploadBatch>> {
    UPLOADS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn cleanup_batch(batch: UploadBatch) {
    for file in batch.files { let _ = fs::remove_file(file.path); }
}

pub fn upload_start(body: &Value) -> FileResult {
    let _guard = UPLOAD_LOCK.lock().map_err(|_| error(500, "上传状态不可用"))?;
    let session = body["sessionId"].as_str().ok_or_else(|| error(400, "附件必须绑定到会话"))?;
    let scope = attachment_scope(session)?;
    let local = body["local"].as_bool().unwrap_or(false);
    let specifications = body["files"].as_array().ok_or_else(|| error(400, "没有文件"))?;
    if specifications.is_empty() || specifications.len() > MAX_BATCH_FILES { return Err(error(400, "上传文件数量无效")); }
    let mut registry = uploads().lock().map_err(|_| error(500, "上传状态不可用"))?;
    let expired = registry.iter().filter(|(_, batch)| batch.touched.elapsed() > std::time::Duration::from_secs(1800)).map(|(id, _)| id.clone()).collect::<Vec<_>>();
    for id in expired { if let Some(batch) = registry.remove(&id) { cleanup_batch(batch); } }
    if registry.len() >= 32 { return Err(error(429, "进行中的上传过多")); }
    let root = outputs_root()?;
    let batch_name = format!("{}-{}", chrono::Local::now().format("%Y%m%d"), &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let dir = safe_directory(&root, &PathBuf::from("_inbox").join(&scope).join(&batch_name))?;
    let mut batch = UploadBatch { root: crate::workspace::project_root(), scope, files: Vec::new(), touched: std::time::Instant::now() };
    let result = (|| -> Result<(), ApiError> {
        for specification in specifications {
            let name = specification["name"].as_str().unwrap_or("file").rsplit(['/', '\\']).next().unwrap_or("file");
            if name.is_empty() || name.starts_with('.') || name.contains(':') || name.chars().any(char::is_control) { return Err(error(400, "文件名非法")); }
            if !supported_upload(Path::new(name)) { return Err(error(400, format!("不支持的文件类型：{name}"))); }
            let expected = specification["size"].as_u64().ok_or_else(|| error(400, "文件大小无效"))?;
            if !local && expected > MAX_UPLOAD_BYTES as u64 { return Err(error(413, format!("{name} 超过 50MB 上限"))); }
            let source = Path::new(name);
            let mut target = dir.join(name);
            let mut duplicate = 2;
            while target.exists() {
                let stem = source.file_stem().unwrap_or_default().to_string_lossy();
                let extension = source.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default();
                target = dir.join(format!("{stem} ({duplicate}){extension}"));
                duplicate += 1;
            }
            OpenOptions::new().create_new(true).write(true).open(&target).map_err(io_error)?;
            let relative = target.strip_prefix(&root).map_err(|_| error(403, "非法路径"))?.to_string_lossy().replace('\\', "/");
            batch.files.push(UploadFileState { name: target.file_name().unwrap().to_string_lossy().into_owned(), path: target, relative, expected, received: 0 });
        }
        Ok(())
    })();
    if let Err(cause) = result { cleanup_batch(batch); return Err(cause); }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let response = json!({"uploadId":id,"files":batch.files.iter().map(|file| json!({"name":file.name,"path":file.relative})).collect::<Vec<_>>()});
    registry.insert(id, batch);
    Ok(response)
}

pub fn upload_chunk(body: &Value) -> FileResult {
    let id = body["uploadId"].as_str().ok_or_else(|| error(400, "uploadId 无效"))?;
    let index = body["index"].as_u64().ok_or_else(|| error(400, "文件序号无效"))? as usize;
    let offset = body["offset"].as_u64().ok_or_else(|| error(400, "读取位置无效"))?;
    let encoded = body["contentBase64"].as_str().ok_or_else(|| error(400, "缺少文件内容"))?;
    if encoded.len() > (CHUNK_BYTES + 2) / 3 * 4 { return Err(error(413, "上传块超过 1MB")); }
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| error(400, "文件内容不是有效 base64"))?;
    if bytes.is_empty() || bytes.len() > CHUNK_BYTES { return Err(error(400, "上传块大小无效")); }
    let mut registry = uploads().lock().map_err(|_| error(500, "上传状态不可用"))?;
    let batch = registry.get_mut(id).ok_or_else(|| error(404, "上传任务不存在"))?;
    if batch.root != crate::workspace::project_root() { return Err(error(404, "上传任务不存在")); }
    let file = batch.files.get_mut(index).ok_or_else(|| error(400, "文件序号无效"))?;
    if file.received != offset { return Err(error(409, "上传块位置不匹配")); }
    if bytes.len() as u64 > file.expected.saturating_sub(offset) { return Err(error(400, "上传内容超过声明大小")); }
    let metadata = fs::symlink_metadata(&file.path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || metadata.len() != offset || !file.path.canonicalize().map_err(io_error)?.starts_with(outputs_root()?) { return Err(error(403, "上传目标无效")); }
    OpenOptions::new().append(true).open(&file.path).map_err(io_error)?.write_all(&bytes).map_err(io_error)?;
    file.received += bytes.len() as u64;
    batch.touched = std::time::Instant::now();
    Ok(json!({"received":file.received}))
}

pub fn upload_finish(body: &Value) -> FileResult {
    let id = body["uploadId"].as_str().ok_or_else(|| error(400, "uploadId 无效"))?;
    let mut registry = uploads().lock().map_err(|_| error(500, "上传状态不可用"))?;
    let batch = registry.get(id).ok_or_else(|| error(404, "上传任务不存在"))?;
    if batch.root != crate::workspace::project_root() { return Err(error(404, "上传任务不存在")); }
    if batch.files.iter().any(|file| file.received != file.expected) { return Err(error(409, "文件上传尚未完成")); }
    let batch = registry.remove(id).unwrap();
    Ok(json!({"ok":true,"files":batch.files.iter().map(|file| json!({"id":attachment_id(&batch.scope,&file.relative),"name":file.name,"path":file.relative})).collect::<Vec<_>>()}))
}

pub fn upload_abort(body: &Value) -> FileResult {
    let id = body["uploadId"].as_str().ok_or_else(|| error(400, "uploadId 无效"))?;
    let mut registry = uploads().lock().map_err(|_| error(500, "上传状态不可用"))?;
    if registry.get(id).map(|batch| batch.root == crate::workspace::project_root()).unwrap_or(false) {
        if let Some(batch) = registry.remove(id) { cleanup_batch(batch); }
    }
    Ok(json!({"ok":true}))
}

pub fn attachment_context(session: Option<&str>, attachments: &Value) -> Result<String, ApiError> {
    let Some(items) = attachments.as_array().filter(|items| !items.is_empty()) else { return Ok(String::new()) };
    let scope = attachment_scope(session.ok_or_else(|| error(400, "附件必须绑定到会话"))?)?;
    let mut rows = Vec::new();
    for item in items {
        let rel = item.get("path").and_then(Value::as_str).ok_or_else(|| error(400, "附件路径无效"))?;
        relative_path(rel).map_err(|_| error(400, "附件路径无效"))?;
        let segments: Vec<_> = rel.split('/').collect();
        if segments.len() != 4 || segments[0] != "_inbox" { return Err(error(400, "附件路径无效")) }
        if segments[1] != scope { return Err(error(403, "附件不属于当前会话")) }
        if item.get("id").and_then(Value::as_str) != Some(attachment_id(&scope, rel).as_str()) { return Err(error(403, "附件标识校验失败")) }
        if !output_path(rel)?.is_file() { return Err(error(404, "附件不存在")) }
        if !rows.contains(&rel) { rows.push(rel); }
    }
    Ok(format!("〔系统附件清单，仅供本轮执行，不要向用户复述文件上传过程或内部路径〕\n只允许使用下列当前会话附件；禁止扫描、枚举或猜测 outputs/_inbox 中的其他文件：\n{}\n需要纳入内容项目时，将清单内文件复制到 outputs/<项目>/assets/ 后再使用；保留 inbox 原件，确保重试仍可复现。", rows.iter().map(|rel| format!("- outputs/{rel}")).collect::<Vec<_>>().join("\n")))
}
