//! Easel 工作区:~/.otools/easel 项目根(镜像上游 PROJECT_ROOT 语义)。
//!
//! 目录契约与上游一致:
//! - `skills/builtin/<技能>/SKILL.md + references/`、`skills/shared/`
//! - `agents/workspace/`(SOUL.md / AGENTS.md)
//! - `profiles/<画像>/*.md`(`_template` 为上游种子)
//! - `outputs/`(产物)、`outputs/_harness/config.json`(执行底座设置)
//!
//! 种子来自插件包 `dist/easel/`(构建期由 vite publicDir 拷入),首启同步:
//! skills/agents 覆盖(上游资产),profiles 只补 `_template`,绝不触碰用户画像。

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PLUGIN_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 宿主信封下发的插件安装目录(scrcpy 同款约定),用于定位 dist/easel 种子。
pub fn set_plugin_dir(dir: &str) -> bool {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return false;
    }
    let path = PathBuf::from(trimmed);
    if !path.is_dir() {
        return false;
    }
    PLUGIN_DIR.set(path).is_ok()
}

pub fn plugin_dir() -> Option<&'static PathBuf> {
    PLUGIN_DIR.get()
}

/// 项目根:OTOOLS_EASEL_ROOT 覆盖 → ~/.otools/easel。
pub fn project_root() -> PathBuf {
    if let Ok(value) = std::env::var("OTOOLS_EASEL_ROOT") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".otools").join("easel")
}

fn seed_root() -> Option<PathBuf> {
    let dir = plugin_dir()?.join("dist").join("easel");
    dir.is_dir().then_some(dir)
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<u64> {
    let mut count = 0u64;
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            count += copy_tree(&entry.path(), &target_path)?;
        } else {
            fs::copy(entry.path(), &target_path)?;
            count += 1;
        }
    }
    Ok(count)
}

/// 首启(或种子版本变化)同步:skills/agents 覆盖,profiles 仅补 _template。
pub fn bootstrap() -> Result<Value, String> {
    let Some(seed) = seed_root() else {
        return Err("插件种子资源缺失(dist/easel);请重新安装插件".to_string());
    };
    let root = project_root();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let mut synced = 0u64;

    for dir in ["skills", "agents"] {
        let source = seed.join(dir);
        if source.is_dir() {
            synced += copy_tree(&source, &root.join(dir)).map_err(|error| error.to_string())?;
        }
    }
    let manifest_source = seed.join("skills-manifest.json");
    if manifest_source.is_file() {
        let target_dir = root.join("skills");
        fs::create_dir_all(&target_dir).map_err(|error| error.to_string())?;
        fs::copy(&manifest_source, target_dir.join("skills-manifest.json"))
            .map_err(|error| error.to_string())?;
        synced += 1;
    }
    let template_source = seed.join("profiles").join("_template");
    if template_source.is_dir() {
        synced += copy_tree(&template_source, &root.join("profiles").join("_template"))
            .map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(root.join("outputs")).map_err(|error| error.to_string())?;
    Ok(json!({ "root": root.to_string_lossy(), "synced": synced }))
}

/// 确保工作区就绪(幂等;skills-manifest.json 存在视为已初始化)。
pub fn ensure_ready() -> Result<PathBuf, String> {
    let root = project_root();
    let marker = root.join("skills").join("skills-manifest.json");
    if marker.is_file() {
        return Ok(root);
    }
    bootstrap()?;
    Ok(root)
}
