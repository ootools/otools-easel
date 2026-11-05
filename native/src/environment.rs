//! Native environment checks and fixed-recipe installation jobs for the original EnvBoard.

use crate::{process::{self, capture, LogSink}, router::ApiError, settings, workspace};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Tool { id: &'static str, name: &'static str, group: &'static str, desc: &'static str, big: bool, needs_dir: bool }
const TOOLS: &[Tool] = &[
    Tool{id:"node",name:"Node.js",group:"rm",desc:"视频产线运行时",big:false,needs_dir:false},
    Tool{id:"ffmpeg",name:"ffmpeg",group:"rm",desc:"音视频处理与滤镜",big:false,needs_dir:false},
    Tool{id:"python",name:"Python",group:"rm",desc:"技能脚本解释器",big:false,needs_dir:false},
    Tool{id:"fw",name:"faster-whisper",group:"rm",desc:"本地音频转录",big:false,needs_dir:false},
    Tool{id:"model",name:"Whisper large-v3 模型",group:"rm",desc:"约 3.1GB · 按需下载",big:true,needs_dir:false},
    Tool{id:"rmdeps",name:"Remotion 工程依赖",group:"rm",desc:"工程目录内 npm ci",big:false,needs_dir:true},
    Tool{id:"shell",name:"Chrome Headless Shell",group:"rm",desc:"Remotion 渲染浏览器",big:false,needs_dir:true},
    Tool{id:"biliup",name:"biliup",group:"pub",desc:"B站投稿 CLI",big:false,needs_dir:false},
    Tool{id:"pw",name:"Playwright Chromium",group:"pub",desc:"平台登录与发布浏览器",big:false,needs_dir:false},
    Tool{id:"cft",name:"Chrome for Testing",group:"pub",desc:"登录态浏览器",big:false,needs_dir:false},
    Tool{id:"pylibs",name:"常用 Python 库",group:"common",desc:"Pillow · edge-tts · pdfplumber · requests",big:false,needs_dir:false},
];

fn error(status: u16, detail: impl Into<String>) -> ApiError { ApiError{status,detail:detail.into()} }
fn tool(id: &str) -> Option<&'static Tool> { TOOLS.iter().find(|tool| tool.id == id) }
fn group_name(group: &str) -> &str { match group {"rm"=>"视频产线套件（Remotion）","pub"=>"发布链",_=>"常用库"} }

pub fn python() -> Option<PathBuf> {
    let root = workspace::project_root();
    let candidates = [root.join(if cfg!(windows){".venv/Scripts/python.exe"}else{".venv/bin/python"}), root.join(".venv/bin/python3")];
    candidates.into_iter().find(|path| path.is_file()).or_else(|| process::executable("python3")).or_else(|| process::executable("python"))
}

fn check(tool: &Tool, python: Option<&Path>) -> Value {
    if tool.needs_dir { return json!({"id":tool.id,"state":"no_dir","version":null,"detail":"需要指定工程目录"}) }
    if tool.id == "model" {
        let dir = settings::home().join("models/whisper-large-v3");
        let ready = dir.is_dir() && std::fs::read_dir(dir).ok().and_then(|mut items| items.next()).is_some();
        return json!({"id":tool.id,"state":if ready {"ok"}else{"missing"},"version":if ready {Some("模型目录已就绪")}else{None},"detail":null});
    }
    if tool.id == "cft" {
        let cache = if cfg!(target_os="macos") {settings::home().join("Library/Caches/ms-playwright")}else if cfg!(windows){PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("ms-playwright")}else{settings::home().join(".cache/ms-playwright")};
        let ready = std::fs::read_dir(cache).ok().map(|items| items.flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("chromium"))).unwrap_or(false);
        return json!({"id":tool.id,"state":if ready {"ok"}else{"missing"},"version":if ready {Some("Chromium 已缓存")}else{None},"detail":null});
    }
    let (program, args) = match tool.id {
        "node" => (process::executable("node"),vec!["--version".into()]),
        "ffmpeg" => (process::executable("ffmpeg"),vec!["-version".into()]),
        "biliup" => (process::executable("biliup"),vec!["--version".into()]),
        "python" => (python.map(Path::to_path_buf),vec!["--version".into()]),
        "fw" => (python.map(Path::to_path_buf),vec!["-I".into(),"-c".into(),"import faster_whisper as f; print(getattr(f,'__version__','ok'))".into()]),
        "pw" => (python.map(Path::to_path_buf),vec!["-I".into(),"-c".into(),"import playwright; print('playwright ok')".into()]),
        "pylibs" => (python.map(Path::to_path_buf),vec!["-I".into(),"-c".into(),"import PIL,edge_tts,pdfplumber,requests; print('4/4 ok')".into()]),
        _ => (None,Vec::new()),
    };
    let Some(program) = program else { return json!({"id":tool.id,"state":"missing","version":null,"detail":null}) };
    match capture(&program,&args,&workspace::project_root(),Duration::from_secs(12),None) {
        Ok(output) if output.success => {
            let text = if output.stdout.trim().is_empty(){&output.stderr}else{&output.stdout};
            let version = text.lines().next().unwrap_or("已装").chars().take(70).collect::<String>();
            json!({"id":tool.id,"state":"ok","version":version,"detail":null})
        }
        Ok(_) => json!({"id":tool.id,"state":"missing","version":null,"detail":null}),
        Err(cause) => json!({"id":tool.id,"state":"missing","version":null,"detail":cause}),
    }
}

struct CheckCache { root: PathBuf, at: Instant, value: Value }
static CHECK_CACHE: OnceLock<Mutex<Option<CheckCache>>> = OnceLock::new();
fn cache() -> &'static Mutex<Option<CheckCache>> { CHECK_CACHE.get_or_init(||Mutex::new(None)) }

pub fn tools(refresh: bool) -> Result<Value,ApiError> {
    let root = workspace::ensure_ready().map_err(|cause|error(500,cause))?;
    if !refresh { if let Ok(cache) = cache().lock() { if let Some(cache) = cache.as_ref() { if cache.root == root && cache.at.elapsed() < Duration::from_secs(15) {return Ok(cache.value.clone())} } } }
    let python = python();
    let mut rows = Vec::new();
    for tool in TOOLS {
        let mut row = check(tool,python.as_deref());
        row["name"] = json!(tool.name); row["group"] = json!(tool.group); row["group_name"] = json!(group_name(tool.group));row["desc"]=json!(tool.desc);row["big"]=json!(tool.big);
        rows.push(row);
    }
    let value = json!({"python":python.map(|path|path.to_string_lossy().into_owned()).unwrap_or_default(),"tools":rows,"cachedAt":chrono::Utc::now().timestamp()});
    if let Ok(mut cache) = cache().lock(){*cache=Some(CheckCache{root,at:Instant::now(),value:value.clone()});}
    Ok(value)
}

struct InstallJob {root:PathBuf, value:Mutex<Value>}
static JOBS:OnceLock<Mutex<HashMap<String,Arc<InstallJob>>>>=OnceLock::new();
fn jobs()->&'static Mutex<HashMap<String,Arc<InstallJob>>>{JOBS.get_or_init(||Mutex::new(HashMap::new()))}

fn append(job:&Arc<InstallJob>,line:String){if let Ok(mut value)=job.value.lock(){let mut lines=value["lines"].as_array().cloned().unwrap_or_default();lines.push(json!(line.chars().take(4096).collect::<String>()));if lines.len()>40{lines.drain(..lines.len()-40);}value["lines"]=json!(lines);}}

fn run(program:&str,args:Vec<String>,root:&Path,logs:LogSink)->Result<(),String>{
    let program=process::executable(program).ok_or_else(||format!("未找到 {program}"))?;
    let output=capture(&program,&args,root,Duration::from_secs(3600),Some(logs))?;
    if output.success{Ok(())}else{Err(format!("安装命令失败：{}",output.stderr.chars().rev().take(300).collect::<String>().chars().rev().collect::<String>()))}
}

fn install_tool(tool:&Tool,root:&Path,logs:LogSink)->Result<String,String>{
    let interpreter=python();
    let checked=check(tool,interpreter.as_deref());
    if checked["state"]=="ok" {return Ok("已装".into())}
    match tool.id {
        "node"|"ffmpeg"|"python"=>{
            #[cfg(windows)]
            {let id=match tool.id{"node"=>"OpenJS.NodeJS.LTS","python"=>"Python.Python.3.13",_=>"Gyan.FFmpeg"};run("winget",vec!["install".into(),"-e".into(),"--id".into(),id.into(),"--accept-source-agreements".into(),"--accept-package-agreements".into()],root,logs)?;}
            #[cfg(not(windows))]
            {return Err(format!("{} 需要通过系统软件管理器安装；未执行提权或系统修改",tool.name));}
        }
        "fw"|"biliup"|"pw"|"cft"|"pylibs"|"model"=>{
            let python=interpreter.ok_or("请先安装 Python")?;
            let packages=match tool.id {"fw"=>vec!["faster-whisper"],"biliup"=>vec!["biliup"],"pw"|"cft"=>vec!["playwright"],"pylibs"=>vec!["Pillow","edge-tts","pdfplumber","requests"],_=>vec!["huggingface_hub"]};
            let mut args=vec!["-m".into(),"pip".into(),"install".into(),"--upgrade".into(),"--timeout".into(),"60".into(),"--retries".into(),"5".into()];args.extend(packages.into_iter().map(str::to_string));
            let output=capture(&python,&args,root,Duration::from_secs(1800),Some(logs.clone()))?;
            if !output.success {args.extend(["-i".into(),"https://pypi.tuna.tsinghua.edu.cn/simple".into()]);let output=capture(&python,&args,root,Duration::from_secs(1800),Some(logs.clone()))?;if !output.success{return Err("pip 安装失败，未通过安装后校验".into())}}
            if matches!(tool.id,"pw"|"cft"){let output=capture(&python,&["-m".into(),"playwright".into(),"install".into(),"chromium".into()],root,Duration::from_secs(1800),Some(logs.clone()))?;if !output.success{return Err("Chromium 安装失败".into())}}
            if tool.id=="model"{let script="import os;os.environ.setdefault('HF_ENDPOINT','https://hf-mirror.com');from huggingface_hub import snapshot_download; print(snapshot_download('Systran/faster-whisper-large-v3',local_dir=os.path.expanduser('~/models/whisper-large-v3')))";let output=capture(&python,&["-I".into(),"-c".into(),script.into()],root,Duration::from_secs(7200),Some(logs))?;if !output.success{return Err("Whisper 模型下载失败".into())}}
        }
        _=>return Err("该工具需要指定工程目录".into()),
    }
    let checked=check(tool,python().as_deref());
    if checked["state"]=="ok"{Ok("安装后校验通过".into())}else{Err("安装命令已结束，但重新检测未通过".into())}
}

pub fn start_install(body:&Value)->Result<Value,ApiError>{
    let id=body["id"].as_str().unwrap_or("").trim();
    let tool=tool(id).ok_or_else(||error(400,"无效的工具 id"))?;
    if tool.needs_dir{return Err(error(400,"该工具需要指定工程目录"))}
    let root=workspace::ensure_ready().map_err(|cause|error(500,cause))?;
    let mut registry=jobs().lock().map_err(|_|error(500,"安装任务状态不可用"))?;
    if registry.values().any(|job|job.value.lock().map(|value|value["state"]=="running").unwrap_or(true)){return Err(error(409,"已有环境安装任务运行中"))}
    if registry.len()>=200{let mut completed=registry.iter().filter_map(|(id,job)|job.value.lock().ok().filter(|value|value["state"]!="running").map(|value|(id.clone(),value["ended"].as_i64().unwrap_or(0)))).collect::<Vec<_>>();completed.sort_by_key(|(_,ended)|*ended);for(id,_)in completed.into_iter().take(100){registry.remove(&id);}}
    let job_id=uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
    let job=Arc::new(InstallJob{root:root.clone(),value:Mutex::new(json!({"jobId":job_id,"id":id,"state":"running","lines":[],"result":null,"started":chrono::Utc::now().timestamp(),"ended":null}))});
    registry.insert(job_id.clone(),job.clone());drop(registry);
    let result=json!({"jobId":job_id,"id":id,"state":"running"});
    std::thread::Builder::new().name(format!("easel-env-{job_id}")).spawn(move||{
        let job_logs=job.clone();let logs:LogSink=Arc::new(move|line|append(&job_logs,line));logs(format!("检查并安装 {}",tool.name));
        let outcome=install_tool(tool,&root,logs);
        if let Ok(mut value)=job.value.lock(){let state=if outcome.is_ok(){"ok"}else{"fail"};value["state"]=json!(state);value["result"]=match outcome{Ok(strategy)=>{
                let current=check(tool,python().as_deref());
                json!({"id":tool.id,"state":"ok","strategy":strategy,"version":current["version"],"detail":null})
            },Err(detail)=>json!({"id":tool.id,"state":"fail","detail":detail})};value["ended"]=json!(chrono::Utc::now().timestamp());}
        if let Ok(mut cache)=cache().lock(){*cache=None;}
    }).map_err(|cause|error(500,cause.to_string()))?;
    Ok(result)
}

pub fn job_status(id:&str)->Result<Value,ApiError>{
    let job=jobs().lock().map_err(|_|error(500,"任务状态不可用"))?.get(id).cloned().ok_or_else(||error(404,"任务不存在（服务可能重启过）"))?;
    if job.root!=workspace::project_root(){return Err(error(404,"任务不存在"))}
    let value=job.value.lock().map_err(|_|error(500,"任务状态不可用"))?.clone();
    Ok(value)
}
