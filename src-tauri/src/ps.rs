//! Small process helpers: run PowerShell or any program hidden, with a timeout,
//! capture or stream the output.

use crate::util::CREATE_NO_WINDOW;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Windows tools answer in UTF-16 (sfc) or the OEM code page. Dropping NULs fixes the first
/// and lossy UTF-8 is good enough for the second.
pub fn decode(b: &[u8]) -> String {
    let v: Vec<u8> = if b.contains(&0) { b.iter().copied().filter(|x| *x != 0).collect() } else { b.to_vec() };
    String::from_utf8_lossy(&v).to_string()
}

fn drain<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = r.read_to_end(&mut v);
        v
    })
}

pub fn run_capture(program: &str, args: &[&str], env: &[(&str, &str)], secs: u64) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {}", program, e))?;
    let out = drain(child.stdout.take().ok_or("no stdout")?);
    let err = drain(child.stderr.take().ok_or("no stderr")?);
    let start = Instant::now();
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(secs) {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(Output {
        code,
        stdout: decode(&out.join().unwrap_or_default()),
        stderr: decode(&err.join().unwrap_or_default()),
        timed_out,
    })
}

/// Runs a PowerShell script (UTF-8 output). Values for the script travel in environment
/// variables, never inside the script text.
pub fn powershell(script: &str, env: &[(&str, &str)], secs: u64) -> Result<String, String> {
    let full = format!(
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ErrorActionPreference='Stop'; {}",
        script
    );
    let o = run_capture(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &full],
        env,
        secs,
    )?;
    if o.timed_out {
        return Err("PowerShell timed out".into());
    }
    if o.code != Some(0) {
        let e = o.stderr.trim();
        return Err(if e.is_empty() { format!("PowerShell exited with {:?}", o.code) } else { e.lines().next().unwrap_or(e).to_string() });
    }
    Ok(o.stdout)
}

/// PowerShell that prints a JSON array. Returns the parsed array (empty on empty output).
pub fn powershell_json(script: &str, env: &[(&str, &str)], secs: u64) -> Result<serde_json::Value, String> {
    let out = powershell(script, env, secs)?;
    let t = out.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return Ok(serde_json::Value::Array(vec![]));
    }
    let v: serde_json::Value = serde_json::from_str(t).map_err(|e| format!("Unexpected PowerShell output: {}", e))?;
    Ok(match v {
        serde_json::Value::Array(_) => v,
        serde_json::Value::Null => serde_json::Value::Array(vec![]),
        other => serde_json::Value::Array(vec![other]),
    })
}

// ---------------------------------------------------------------- streaming

/// Starts a program and calls `on_line` for every output line (stdout and stderr).
/// Blocks until it exits or `cancel` is set. Returns the exit code.
pub fn run_stream(
    program: &str,
    args: &[String],
    cancel: &std::sync::atomic::AtomicBool,
    on_line: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<Option<i32>, String> {
    let mut child = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {}", program, e))?;
    let so = child.stdout.take().ok_or("no stdout")?;
    let se = child.stderr.take().ok_or("no stderr")?;
    let pump = |mut r: Box<dyn Read + Send>, cb: Arc<dyn Fn(String) + Send + Sync>| {
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut cur: Vec<u8> = Vec::new();
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &buf[..n] {
                            if b == b'\n' || b == b'\r' {
                                if !cur.is_empty() {
                                    let line = decode(&cur);
                                    if !line.trim().is_empty() {
                                        cb(line.trim_end().to_string());
                                    }
                                    cur.clear();
                                }
                            } else {
                                cur.push(b);
                            }
                        }
                    }
                }
            }
            if !cur.is_empty() {
                let line = decode(&cur);
                if !line.trim().is_empty() {
                    cb(line.trim_end().to_string());
                }
            }
        })
    };
    let h1 = pump(Box::new(so), on_line.clone());
    let h2 = pump(Box::new(se), on_line);
    let code = loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let pid = child.id().to_string();
            let _ = Command::new("taskkill").args(["/PID", &pid, "/T", "/F"]).creation_flags(CREATE_NO_WINDOW).output();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => std::thread::sleep(Duration::from_millis(60)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let _ = h1.join();
    let _ = h2.join();
    Ok(code)
}
