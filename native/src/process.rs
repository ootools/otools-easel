//! Bounded native process runner for checks and user-requested environment installs.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
pub type LogSink = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Debug)]
pub struct ProcessOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn executable(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() && path.is_file() { return Some(path.to_path_buf()) }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join(name);
        if candidate.is_file() { return Some(candidate) }
        #[cfg(windows)]
        for extension in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{name}.{extension}"));
            if candidate.is_file() { return Some(candidate) }
        }
    }
    None
}

fn read_pipe<R: std::io::Read>(pipe: R, logs: Option<LogSink>) -> String {
    let mut reader = BufReader::new(pipe);
    let mut bytes = Vec::new();
    let mut captured = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if let Some(logs) = &logs {
                    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).trim_end().to_string();
                    if !text.is_empty() { logs(text) }
                }
                let remaining = OUTPUT_LIMIT.saturating_sub(captured.len());
                captured.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
            }
        }
    }
    String::from_utf8_lossy(&captured).into_owned()
}

pub fn capture(program: &Path, args: &[String], cwd: &Path, timeout: Duration, logs: Option<LogSink>) -> Result<ProcessOutput, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|cause| format!("无法启动 {}：{cause}", program.display()))?;
    let stdout = child.stdout.take().ok_or("缺少进程 stdout")?;
    let stderr = child.stderr.take().ok_or("缺少进程 stderr")?;
    let out = std::thread::spawn(move || read_pipe(stdout, None));
    let err = std::thread::spawn(move || read_pipe(stderr, logs));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|cause| cause.to_string())? { break Some(status) }
        if start.elapsed() >= timeout {
            #[cfg(unix)]
            {
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let Some(status) = status else { return Err(format!("操作超时（{}ms）", timeout.as_millis())) };
    Ok(ProcessOutput { success: status.success(), stdout, stderr })
}
