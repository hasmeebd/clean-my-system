//! Developer junk finder: regenerable build output, caches and dependency folders inside
//! project folders. Every candidate is verified against a manifest file that sits next to
//! it (package.json, Cargo.toml, *.uproject ...) or against git's ignore rules, so a plain
//! folder that merely happens to be called "build" is never offered.
//!
//! It also reads git (when installed) to tell whether a project has uncommitted work and
//! when it was last committed to, so the UI can offer "stale and clean" projects first.

use crate::util::*;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- output types

#[derive(Serialize, Clone)]
pub struct JunkEntry {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub stack: String,
    pub size: u64,
    pub files: u64,
    pub modified: u64,
    pub project: String,
    pub rebuild: String,
    /// how painful the rebuild is: low | medium | high
    pub cost: String,
    /// safe = regenerable; review = probably regenerable, look first
    pub risk: String,
    pub note: String,
    /// Rust only: size of target\debug (so just that part can be removed)
    pub debug_size: u64,
    pub is_file: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct GitInfo {
    pub repo: bool,
    /// false when git could not be queried for this repo
    pub known: bool,
    pub dirty: u32,
    pub last_commit: u64,
    pub root: String,
}

#[derive(Serialize, Clone)]
pub struct Project {
    pub path: String,
    pub name: String,
    pub last_active: u64,
    pub git: GitInfo,
}

#[derive(Serialize, Default)]
pub struct Report {
    pub entries: Vec<JunkEntry>,
    pub projects: HashMap<String, Project>,
    pub git_available: bool,
    pub small_count: u64,
    pub small_bytes: u64,
}

// ---------------------------------------------------------------- rules

#[derive(Clone, Copy)]
struct Hit {
    kind: &'static str,
    stack: &'static str,
    rebuild: &'static str,
    cost: &'static str,
    risk: &'static str,
    note: &'static str,
}

const fn hit(kind: &'static str, stack: &'static str, rebuild: &'static str, cost: &'static str, risk: &'static str, note: &'static str) -> Hit {
    Hit { kind, stack, rebuild, cost, risk, note }
}

/// Folders that are never searched (tools, system, app data, version control).
const SKIP: [&str; 24] = [
    "appdata", "program files", "program files (x86)", "windows", "$recycle.bin",
    "system volume information", ".cargo", ".rustup", ".vscode", ".vscode-shared", ".cursor",
    ".claude", ".gemini", ".copilot", ".npm", ".gradle", ".unity", ".ollama", ".cache",
    ".local", ".git", "programdata", ".nuget", ".docker",
];

fn has(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| dir.join(n).exists())
}

fn has_ext(dir: &Path, exts: &[&str]) -> bool {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                exts.iter().any(|x| n.ends_with(x))
            })
        })
        .unwrap_or(false)
}

const GRADLE: [&str; 4] = ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"];
const AUTO: &str = "regenerated automatically";

/// Is this directory a regenerable artifact of a known stack? `name` is the lower-case folder name.
fn classify_dir(p: &Path, name: &str) -> Option<Hit> {
    let parent = p.parent()?;
    let node = || has(parent, &["package.json"]);

    if name == "node_modules" && node() {
        return Some(hit("node_modules", "Node", "npm install (or pnpm / yarn install)", "low", "safe", ""));
    }
    if matches!(name, ".next" | ".nuxt" | ".svelte-kit" | ".turbo" | ".parcel-cache" | ".angular" | ".vite" | ".pnpm-store") && node() {
        return Some(hit("Framework build cache", "Node", "rebuilt by the dev server / build", "low", "safe", ""));
    }
    if name == ".output" && has(parent, &["nuxt.config.ts", "nuxt.config.js", "nuxt.config.mjs"]) {
        return Some(hit("Nuxt build output", "Node", "npm run build", "low", "safe", ""));
    }
    if name == "coverage" && node() {
        return Some(hit("Test coverage report", "Node", "npm test -- --coverage", "low", "safe", ""));
    }
    if name == "target" {
        if has(parent, &["Cargo.toml"]) && (p.join("CACHEDIR.TAG").exists() || p.join("debug").is_dir() || p.join("release").is_dir()) {
            return Some(hit("Rust target", "Rust", "cargo build", "high", "safe", "The next build recompiles every dependency (can take minutes)."));
        }
        if has(parent, &["pom.xml"]) {
            return Some(hit("Maven target", "Java", "mvn package", "medium", "safe", ""));
        }
    }
    if matches!(name, ".venv" | "venv" | "env") && p.join("pyvenv.cfg").is_file() {
        return Some(hit(
            "Python venv",
            "Python",
            "python -m venv + pip install -r requirements.txt",
            "medium",
            "review",
            "Exact package versions are only recoverable if the project pins them (requirements.txt / lock file).",
        ));
    }
    if matches!(name, "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".hypothesis" | ".ipynb_checkpoints") {
        return Some(hit("Python cache", "Python", AUTO, "low", "safe", ""));
    }
    if matches!(name, ".tox" | ".nox") && has(parent, &["tox.ini", "noxfile.py", "pyproject.toml", "setup.py"]) {
        return Some(hit("tox / nox environments", "Python", "tox / nox recreate them", "medium", "safe", ""));
    }
    // Unity
    if matches!(name, "library" | "temp" | "obj" | "logs") && parent.join("ProjectSettings").is_dir() && parent.join("Assets").is_dir() {
        return Some(if name == "library" {
            hit("Unity Library", "Unity", "open the project in Unity", "high", "safe", "Unity reimports every asset on the next open (minutes to hours for big projects).")
        } else {
            hit("Unity temp / logs", "Unity", AUTO, "low", "safe", "")
        });
    }
    // Unreal
    if matches!(name, "intermediate" | "deriveddatacache" | "binaries") && has_ext(parent, &[".uproject"]) {
        return Some(hit("Unreal build data", "Unreal", "open / build the project in Unreal", "high", "safe", "Shaders and binaries are rebuilt on the next open (long)."));
    }
    if name == "saved" && has_ext(parent, &[".uproject"]) {
        return Some(hit("Unreal Saved", "Unreal", AUTO, "medium", "review", "May hold SaveGames and local config. Look inside before deleting."));
    }
    // Gradle / Android
    if matches!(name, ".gradle" | ".cxx" | "build") && has(parent, &GRADLE) {
        return Some(hit("Gradle build output", "Gradle / Android", "gradlew build", "medium", "safe", ""));
    }
    // Flutter
    if matches!(name, ".dart_tool" | "build") && has(parent, &["pubspec.yaml"]) {
        return Some(hit("Flutter build output", "Flutter", "flutter pub get / flutter build", "low", "safe", ""));
    }
    // .NET
    if matches!(name, "bin" | "obj") && has_ext(parent, &[".csproj", ".fsproj", ".vbproj", ".sln"]) {
        return Some(hit(".NET build output", ".NET", "dotnet build", "low", "safe", ""));
    }
    // iOS
    if name == "pods" && has(parent, &["Podfile"]) {
        return Some(hit("CocoaPods", "iOS", "pod install", "low", "safe", ""));
    }
    if name == "deriveddata" && has_ext(parent, &[".xcodeproj", ".xcworkspace"]) {
        return Some(hit("Xcode DerivedData", "iOS", AUTO, "low", "safe", ""));
    }
    if name == ".terraform" && has_ext(parent, &[".tf"]) {
        return Some(hit("Terraform providers", "Terraform", "terraform init", "medium", "safe", ""));
    }
    None
}

/// Blender keeps `scene.blend1`, `scene.blend2` ... as automatic backups.
fn is_blender_backup(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.len() > 5 && ext.starts_with("blend") && ext[5..].chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Folder names that must never be offered through the git-ignore route.
fn dangerous(name: &str) -> bool {
    ["secret", "credential", ".env", "key", "cert", "private", "password", "token", "ssh", "backup", "wallet"]
        .iter()
        .any(|k| name.contains(k))
}

/// Ignored-by-git folders with these names are normally plain build output.
fn known_output(name: &str) -> bool {
    matches!(
        name,
        "dist" | "build" | "out" | "coverage" | "tmp" | "temp" | "bin" | "obj" | "release" | "debug" | "generated"
            | "cache" | "logs" | "log" | ".turbo" | ".parcel-cache" | ".next" | ".nuxt" | ".svelte-kit" | ".output"
            | ".gradle" | "intermediate" | "deriveddatacache" | "library" | ".dart_tool" | "storybook-static"
    )
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('\\').to_lowercase()
}
fn under(child: &str, parent: &str) -> bool {
    child == parent || child.starts_with(&format!("{}\\", parent))
}
fn lname(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

// ---------------------------------------------------------------- git

fn git_cmd() -> Command {
    let mut c = Command::new("git");
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

pub fn git_available() -> bool {
    git_cmd()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_git(repo: &Path, args: &[&str], secs: u64) -> Option<Vec<u8>> {
    let mut child = git_cmd()
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    // drain stdout on a thread so a full pipe can never block git
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut v);
        v
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let v = reader.join().ok()?;
                return if st.success() { Some(v) } else { None };
            }
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(secs) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// Is `p` ignored by git (and therefore untracked)?
fn git_ignored(p: &Path) -> bool {
    let (parent, name) = match (p.parent(), p.file_name()) {
        (Some(a), Some(b)) => (a, b),
        _ => return false,
    };
    git_cmd()
        .arg("-C")
        .arg(parent)
        .args(["check-ignore", "-q", "--"])
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Status of one repository: git info plus the collapsed ignored folders (relative paths).
fn repo_status(repo: &Path) -> (GitInfo, Vec<String>) {
    let mut info = GitInfo { repo: true, known: false, dirty: 0, last_commit: 0, root: repo.to_string_lossy().to_string() };
    let mut ignored = Vec::new();
    if let Some(out) = run_git(repo, &["status", "--porcelain=v1", "-z", "--ignored=traditional"], 90) {
        info.known = true;
        let mut it = out.split(|b| *b == 0);
        while let Some(rec) = it.next() {
            if rec.len() < 4 || rec[2] != b' ' {
                continue;
            }
            let x = rec[0];
            let y = rec[1];
            let path = String::from_utf8_lossy(&rec[3..]).to_string();
            if x == b'!' && y == b'!' {
                if let Some(d) = path.strip_suffix('/') {
                    ignored.push(d.to_string());
                }
                continue;
            }
            info.dirty += 1;
            if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
                it.next(); // the "from" path of a rename
            }
        }
    }
    if let Some(out) = run_git(repo, &["log", "-1", "--format=%ct"], 15) {
        info.last_commit = String::from_utf8_lossy(&out).trim().parse().unwrap_or(0);
    }
    (info, ignored)
}

/// Does `dir` contain a nested git repository within a few levels?
fn contains_git(dir: &Path, depth: u32) -> bool {
    if dir.join(".git").exists() {
        return true;
    }
    if depth == 0 {
        return false;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(md) = e.metadata() {
                let a = md.file_attributes();
                if is_dir_attr(a) && !is_reparse(a) && contains_git(&e.path(), depth - 1) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------- validation used by delete

/// Final gate before anything is removed under the "dev junk" flow.
pub fn is_junk_target(p: &Path) -> bool {
    let name = lname(p);
    let md = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if is_reparse(md.file_attributes()) {
        return false;
    }
    if !is_dir_attr(md.file_attributes()) {
        return is_blender_backup(&name);
    }
    if classify_dir(p, &name).is_some() {
        return true;
    }
    if name == "debug" {
        if let Some(par) = p.parent() {
            if lname(par) == "target" && is_junk_target(par) {
                return true;
            }
        }
    }
    !dangerous(&name) && !SKIP.contains(&name.as_str()) && git_ignored(p)
}

// ---------------------------------------------------------------- scan

struct Raw {
    path: PathBuf,
    hit: Option<Hit>,
    project: PathBuf,
    is_file: bool,
    modified: u64,
}

struct Found {
    raws: Mutex<Vec<Raw>>,
    repos: Mutex<Vec<PathBuf>>,
}

fn junk_walk(dir: &Path, ctx: &Ctx, f: &Found, depth: u32) {
    if ctx.cancel.load(Ordering::Relaxed) || depth > 16 {
        return;
    }
    ctx.files.fetch_add(1, Ordering::Relaxed);
    if dir.join(".git").exists() {
        f.repos.lock().unwrap().push(dir.to_path_buf());
    }
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut subs: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let md = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let a = md.file_attributes();
        let lower = e.file_name().to_string_lossy().to_lowercase();
        if !is_dir_attr(a) {
            if is_blender_backup(&lower) {
                f.raws.lock().unwrap().push(Raw { path: e.path(), hit: None, project: dir.to_path_buf(), is_file: true, modified: mtime(&md) });
            }
            continue;
        }
        if is_reparse(a) || SKIP.contains(&lower.as_str()) {
            continue;
        }
        let p = e.path();
        if let Some(h) = classify_dir(&p, &lower) {
            f.raws.lock().unwrap().push(Raw { path: p, hit: Some(h), project: dir.to_path_buf(), is_file: false, modified: mtime(&md) });
        } else {
            subs.push(p);
        }
    }
    subs.par_iter().for_each(|s| junk_walk(s, ctx, f, depth + 1));
}

/// Newest file modification time inside a project, ignoring git data and regenerable folders.
fn project_activity(p: &Path, cancel: Arc<AtomicBool>) -> u64 {
    let newest = Arc::new(AtomicU64::new(0));
    let n2 = newest.clone();
    let mut ctx = Ctx::new(cancel);
    ctx.on_file = Some(Box::new(move |_p, _a, _l, md| {
        n2.fetch_max(mtime(md), Ordering::Relaxed);
    }));
    ctx.skip_dir = Some(Box::new(|p, name| {
        let l = name.to_lowercase();
        l == ".git" || classify_dir(p, &l).is_some()
    }));
    walk(p, &ctx);
    newest.load(Ordering::Relaxed)
}

pub fn find(roots: &[PathBuf], ctx: &Ctx, include_ignored: bool) -> Report {
    let found = Found { raws: Mutex::new(Vec::new()), repos: Mutex::new(Vec::new()) };

    // drop roots that live inside another root
    let mut sorted: Vec<&PathBuf> = roots.iter().collect();
    sorted.sort_by_key(|p| p.as_os_str().len());
    let mut kept: Vec<PathBuf> = Vec::new();
    for r in sorted {
        let rn = norm(r);
        if !kept.iter().any(|k| under(&rn, &norm(k))) {
            kept.push(r.clone());
        }
    }
    for r in &kept {
        junk_walk(r, ctx, &found, 0);
    }

    let raws = found.raws.into_inner().unwrap();
    let mut repo_paths = found.repos.into_inner().unwrap();
    repo_paths.sort();
    repo_paths.dedup();

    let git_ok = git_available();
    let cancel = ctx.cancel.clone();
    let repo_data: Vec<(PathBuf, GitInfo, Vec<String>)> = if git_ok {
        repo_paths
            .par_iter()
            .map(|r| {
                if cancel.load(Ordering::Relaxed) {
                    return (r.clone(), GitInfo { repo: true, known: false, root: r.to_string_lossy().to_string(), ..Default::default() }, vec![]);
                }
                let (i, ign) = repo_status(r);
                (r.clone(), i, ign)
            })
            .collect()
    } else {
        repo_paths
            .iter()
            .map(|r| (r.clone(), GitInfo { repo: true, known: false, root: r.to_string_lossy().to_string(), ..Default::default() }, vec![]))
            .collect()
    };
    let repo_norms: Vec<String> = repo_data.iter().map(|(p, _, _)| norm(p)).collect();
    let repo_for = |p: &Path| -> Option<usize> {
        let n = norm(p);
        let mut best: Option<usize> = None;
        for (i, rn) in repo_norms.iter().enumerate() {
            if under(&n, rn) && best.map_or(true, |b| repo_norms[b].len() < rn.len()) {
                best = Some(i);
            }
        }
        best
    };

    let mut entries: Vec<JunkEntry> = Vec::new();
    let mut known: Vec<String> = Vec::new();
    for r in &raws {
        known.push(norm(&r.path));
        let (kind, stack, rebuild, cost, risk, note) = match &r.hit {
            Some(h) => (h.kind, h.stack, h.rebuild, h.cost, h.risk, h.note),
            None => ("Blender backup", "Blender", "Blender writes a new backup on every save", "low", "safe", "Older automatic version of the .blend file."),
        };
        entries.push(JunkEntry {
            path: r.path.to_string_lossy().to_string(),
            name: r.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            kind: kind.into(),
            stack: stack.into(),
            size: 0,
            files: 0,
            modified: r.modified,
            project: r.project.to_string_lossy().to_string(),
            rebuild: rebuild.into(),
            cost: cost.into(),
            risk: risk.into(),
            note: note.into(),
            debug_size: 0,
            is_file: r.is_file,
        });
    }

    // folders git ignores but the rules above did not recognise
    let mut ignored_idx: HashSet<usize> = HashSet::new();
    if include_ignored {
        for (repo, _info, ign) in &repo_data {
            for rel in ign {
                let p = repo.join(rel.replace('/', "\\"));
                let n = norm(&p);
                let name = lname(&p);
                if SKIP.contains(&name.as_str()) || dangerous(&name) {
                    continue;
                }
                if known.iter().any(|k| under(k, &n) || under(&n, k)) {
                    continue;
                }
                let md = match fs::symlink_metadata(&p) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !is_dir_attr(md.file_attributes()) || is_reparse(md.file_attributes()) {
                    continue;
                }
                if contains_git(&p, 3) {
                    continue; // a nested clone: its history exists nowhere else
                }
                known.push(n);
                let out = known_output(&name);
                ignored_idx.insert(entries.len());
                entries.push(JunkEntry {
                    path: p.to_string_lossy().to_string(),
                    name: rel.clone(),
                    kind: if out { "Build output (git-ignored)".into() } else { "Git-ignored folder".into() },
                    stack: "Git ignored".into(),
                    size: 0,
                    files: 0,
                    modified: mtime(&md),
                    project: repo.to_string_lossy().to_string(),
                    rebuild: "regenerate with the project's own build / setup".into(),
                    cost: "medium".into(),
                    risk: if out { "safe".into() } else { "review".into() },
                    note: if out {
                        "Ignored by git and named like build output.".into()
                    } else {
                        "Ignored by git, so probably generated. Check it holds no local-only data (databases, uploads, models) first.".into()
                    },
                    debug_size: 0,
                    is_file: false,
                });
            }
        }
    }

    // sizes
    let cancel2 = ctx.cancel.clone();
    entries.par_iter_mut().for_each(|j| {
        let p = PathBuf::from(&j.path);
        if j.is_file {
            if let Ok(md) = fs::metadata(&p) {
                j.size = alloc_size(&p, &md);
                j.files = 1;
            }
        } else {
            let a = walk(&p, &Ctx::new(cancel2.clone()));
            j.size = a.alloc;
            j.files = a.files;
            if j.kind == "Rust target" {
                let d = p.join("debug");
                if d.is_dir() {
                    j.debug_size = walk(&d, &Ctx::new(cancel2.clone())).alloc;
                }
            }
        }
    });

    // drop the crumbs
    let mut small_count = 0u64;
    let mut small_bytes = 0u64;
    let mut idx = 0usize;
    entries.retain(|e| {
        let min = if ignored_idx.contains(&idx) { 50 * 1_048_576 } else { 1_048_576 };
        idx += 1;
        if e.size < min {
            small_count += 1;
            small_bytes += e.size;
            false
        } else {
            true
        }
    });
    entries.sort_by(|a, b| b.size.cmp(&a.size));

    // projects: git state and last activity
    let mut proj_paths: Vec<String> = entries.iter().map(|e| e.project.clone()).collect();
    proj_paths.sort();
    proj_paths.dedup();
    let cancel3 = ctx.cancel.clone();
    let projects: HashMap<String, Project> = proj_paths
        .par_iter()
        .map(|pp| {
            let path = PathBuf::from(pp);
            let git = repo_for(&path).map(|i| repo_data[i].1.clone()).unwrap_or_default();
            let activity = project_activity(&path, cancel3.clone());
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| pp.clone());
            (
                pp.clone(),
                Project { path: pp.clone(), name, last_active: activity.max(git.last_commit), git },
            )
        })
        .collect();

    Report { entries, projects, git_available: git_ok, small_count, small_bytes }
}
