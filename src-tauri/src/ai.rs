//! Local Ollama client. The AI only explains and suggests. It has no access to
//! any delete or move function, and its output is sanitised by safety rules.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(600))
        .build()
}

#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub size: u64,
    /// True when Ollama reports the "thinking" capability for this model.
    pub thinking: bool,
}

static CAPS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn name_guess(model: &str) -> bool {
    let l = model.to_lowercase();
    ["qwen3", "deepseek-r1", "gpt-oss", "magistral", "qwq", "reasoning", "-thinking"].iter().any(|k| l.contains(k))
}

/// Asks Ollama (/api/show) whether a model can "think". Cached per model.
pub fn supports_thinking(host: &str, model: &str) -> bool {
    let cache = CAPS.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{}|{}", host, model);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return *v;
    }
    let r = agent()
        .post(&format!("{}/api/show", host))
        .timeout(Duration::from_secs(5))
        .send_json(json!({"model": model}));
    match r {
        Ok(resp) => {
            let j: Value = resp.into_json().unwrap_or(Value::Null);
            let b = match j["capabilities"].as_array() {
                Some(a) => a.iter().any(|c| c.as_str() == Some("thinking")),
                None => name_guess(model),
            };
            cache.lock().unwrap().insert(key, b);
            b
        }
        Err(_) => name_guess(model),
    }
}

/// Value for the request's "think" field, or None when the model has no thinking support
/// (sending the field to such a model makes Ollama answer with HTTP 400).
fn think_value(host: &str, model: &str, want: bool) -> Option<Value> {
    if !supports_thinking(host, model) {
        return None;
    }
    if model.to_lowercase().contains("gpt-oss") {
        // gpt-oss ignores booleans and only understands effort levels
        return Some(json!(if want { "medium" } else { "low" }));
    }
    Some(json!(want))
}

pub fn tags(host: &str) -> Result<Vec<ModelInfo>, String> {
    let v: Value = agent()
        .get(&format!("{}/api/tags", host))
        .timeout(Duration::from_secs(4))
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Some(a) = v["models"].as_array() {
        for m in a {
            let name = m["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() || name.contains("embed") {
                continue;
            }
            let thinking = supports_thinking(host, &name);
            out.push(ModelInfo { name, size: m["size"].as_u64().unwrap_or(0), thinking });
        }
    }
    Ok(out)
}

pub fn status(host: &str) -> Value {
    match tags(host) {
        Ok(m) => json!({"running": true, "models": m}),
        Err(e) => json!({"running": false, "models": [], "error": e}),
    }
}

pub fn lang_name(l: &str) -> &'static str {
    if l == "en" {
        "English"
    } else {
        "Bengali (বাংলা). Keep all technical terms, file names, paths and units in English"
    }
}

pub fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(a) = out.find("<think>") {
        match out[a..].find("</think>") {
            Some(b) => {
                let end = a + b + "</think>".len();
                out.replace_range(a..end, "");
            }
            None => {
                out.truncate(a);
                break;
            }
        }
    }
    out.trim().to_string()
}

fn extract_json(s: &str) -> Option<Value> {
    let s = strip_think(s);
    let a = s.find('{')?;
    let b = s.rfind('}')?;
    if b <= a {
        return None;
    }
    serde_json::from_str(&s[a..=b]).ok()
}

/// One non-streaming call. Used for explanations and filters.
pub fn chat_once(host: &str, model: &str, system: &str, user: &str, json_mode: bool, think: bool) -> Result<String, String> {
    let mut tv = think_value(host, model, think);
    for attempt in 0..2 {
        let mut body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "stream": false,
            "keep_alive": "2m",
            "options": {"temperature": 0.15, "num_ctx": 6144}
        });
        if json_mode {
            body["format"] = json!("json");
        }
        if let Some(t) = &tv {
            body["think"] = t.clone();
        }
        match agent().post(&format!("{}/api/chat", host)).send_json(body) {
            Ok(resp) => {
                let v: Value = resp.into_json().map_err(|e| e.to_string())?;
                return Ok(v["message"]["content"].as_str().unwrap_or("").to_string());
            }
            Err(ureq::Error::Status(code, resp)) => {
                let txt = resp.into_string().unwrap_or_default();
                if attempt == 0 && tv.is_some() && txt.to_lowercase().contains("think") {
                    tv = None;
                    continue;
                }
                return Err(format!("Ollama HTTP {}: {}", code, txt));
            }
            Err(e) => return Err(format!("Cannot reach Ollama at {}: {}", host, e)),
        }
    }
    Err("Ollama request failed".into())
}

pub fn explain(host: &str, model: &str, lang: &str, facts: &str, think: bool) -> Result<Value, String> {
    let system = format!(
        "You are a careful Windows disk-cleanup assistant for a software developer's PC. \
You only EXPLAIN. You never delete anything. Reply with ONLY one JSON object: \
{{\"what\":\"what this file/folder most likely is\",\"risk\":\"safe|review|keep\",\"reason\":\"why\",\"advice\":\"what the user should do\"}}. \
risk meanings: safe = regenerable cache or temp data; review = user data or something that needs re-download; keep = system, application, source code, database or unknown. \
If unsure choose review or keep. Never choose safe for anything under Windows, Program Files, documents, source code or databases. \
Write the text fields in {}.",
        lang_name(lang)
    );
    let raw = chat_once(host, model, &system, facts, true, think)?;
    let mut v = extract_json(&raw).unwrap_or_else(|| {
        json!({"what": strip_think(&raw), "risk": "review", "reason": "The model did not return structured output.", "advice": "Check the folder manually."})
    });
    let risk = v["risk"].as_str().unwrap_or("review").to_lowercase();
    v["risk"] = json!(if ["safe", "review", "keep"].contains(&risk.as_str()) { risk } else { "review".into() });
    Ok(v)
}

pub fn nl_to_filter(host: &str, model: &str, query: &str, think: bool) -> Result<Value, String> {
    let system = "Convert the user's request about finding files on a Windows PC into a search filter. \
Reply with ONLY one JSON object: {\"name_contains\":[strings],\"extensions\":[\".mp4\",...],\"min_mb\":number,\"older_than_days\":number,\"summary\":\"one short line in English restating the search\"}. \
Use [] for unknown lists and 0 for unknown numbers. Understand Bengali and English. 1 GB = 1024 MB, 1 month = 30 days, 1 year = 365 days.";
    let raw = chat_once(host, model, system, query, true, think)?;
    extract_json(&raw).ok_or_else(|| "The model did not return a usable filter. Try rephrasing.".to_string())
}

pub fn chat_system(lang: &str, ctx: &str) -> String {
    format!(
        "You are the assistant inside Clean My System, a Windows disk-space tool, running locally through Ollama for a software developer.\n\
Rules:\n\
- You can only advise. You cannot delete, move or run anything; the user does that with the app's buttons.\n\
- Prefer the app's Safe Clean for caches and temp files, and Quarantine (undo possible) for personal files.\n\
- Never advise touching C:\\Windows, Program Files, the registry, drivers or system files by hand.\n\
- Order of advice: (1) Safe Clean items marked safe, (2) review items like old Downloads/installers/duplicates via Quarantine, (3) only then mention moving things.\n\
- BIG DOES NOT MEAN UNNEEDED. Large files are listed for information only. Never suggest deleting AI models (.safetensors, .gguf, Ollama, ComfyUI, HuggingFace), game files, VM/WSL disks or project files unless the user says they are unused. At most mention they could be moved to another drive.\n\
- Never present a list of the largest files as 'things to clean'.\n\
- If you do not know what something is, say so and suggest checking it in the Explorer tab first.\n\
- Be concise: short bullets, use the real sizes from the data below, never invent numbers.\n\
- Answer in {}.\n\n\
Current PC data:\n{}",
        lang_name(lang),
        ctx
    )
}

pub fn stream_chat(
    app: AppHandle,
    host: String,
    model: String,
    system: String,
    msgs: Vec<Value>,
    id: String,
    cancel: Arc<AtomicBool>,
    think: bool,
) {
    std::thread::spawn(move || {
        let mut messages = vec![json!({"role": "system", "content": system})];
        messages.extend(msgs);
        let mut tv = think_value(&host, &model, think);
        for attempt in 0..2 {
            let mut body = json!({
                "model": model,
                "messages": messages,
                "stream": true,
                "keep_alive": "2m",
                "options": {"temperature": 0.3, "num_ctx": 8192}
            });
            if let Some(t) = &tv {
                body["think"] = t.clone();
            }
            match agent().post(&format!("{}/api/chat", host)).send_json(body) {
                Ok(resp) => {
                    let reader = BufReader::new(resp.into_reader());
                    for line in reader.lines() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let line = match line {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        if line.trim().is_empty() {
                            continue;
                        }
                        let v: Value = match serde_json::from_str(&line) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                            let _ = app.emit("ai-error", json!({"id": id, "error": err}));
                            return;
                        }
                        let tok = v["message"]["content"].as_str().unwrap_or("");
                        let thk = v["message"]["thinking"].as_str().unwrap_or("");
                        let done = v["done"].as_bool().unwrap_or(false);
                        if !tok.is_empty() || !thk.is_empty() || done {
                            let _ = app.emit("ai-token", json!({"id": id, "token": tok, "thinking": thk, "done": done}));
                        }
                        if done {
                            return;
                        }
                    }
                    let _ = app.emit("ai-token", json!({"id": id, "token": "", "done": true}));
                    return;
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let txt = resp.into_string().unwrap_or_default();
                    if attempt == 0 && tv.is_some() && txt.to_lowercase().contains("think") {
                        tv = None;
                        continue;
                    }
                    let _ = app.emit("ai-error", json!({"id": id, "error": format!("Ollama HTTP {}: {}", code, txt)}));
                    return;
                }
                Err(e) => {
                    let _ = app.emit("ai-error", json!({"id": id, "error": format!("Cannot reach Ollama at {}: {}", host, e)}));
                    return;
                }
            }
        }
    });
}
