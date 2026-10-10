//! `cargo xtask dashboard`: a progress page for the roadmap and the parity
//! matrix, served on this machine. `cargo xtask progress` prints the same
//! data as JSON.
//!
//! The page reads the planning documents from `origin/main`, fetched every two
//! minutes, so it shows what's merged whatever branch the checkout is on.
//! `--worktree` reads the files on disk instead, to preview edits to the
//! documents. The server uses only the standard library, listens on
//! 127.0.0.1, and answers only requests addressed to localhost, so a web page
//! elsewhere can't read the private plan through DNS rebinding.
//!
//! Localhost (`~/code/_localhost`) starts it from `.claude/launch.json` on
//! port 4106, which it also passes as `PORT`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::progress::{Docs, Progress};

/// Localhost's port for Beatlayer (`curl -s localhost/api/ports`).
const DEFAULT_PORT: u16 = 4106;
/// How often the server fetches `origin/main`.
const FETCH_EVERY: Duration = Duration::from_secs(120);
/// The ref the page reads unless `--worktree` is given.
const MAIN: &str = "origin/main";

const PAGE: &str = include_str!("dashboard.html");
const ICON: &str = include_str!("dashboard-icon.svg");

/// Where the documents come from.
#[derive(Clone)]
enum Source {
    Worktree,
    Ref(String),
}

struct Fetch {
    at: Option<SystemTime>,
    error: Option<String>,
    running: bool,
}

struct Server {
    root: PathBuf,
    source: Source,
    port: u16,
    fetch: Mutex<Fetch>,
}

/// `cargo xtask progress`: prints the progress model as JSON.
pub fn print(root: &Path, options: &[String]) -> Result<(), String> {
    let source = match options {
        [] => default_source(root),
        [flag] if flag == "--worktree" => Source::Worktree,
        _ => return Err("usage: cargo xtask progress [--worktree]".into()),
    };
    let server = Server::new(root, source, 0);
    let data = server.progress()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// `cargo xtask dashboard`: serves the page until stopped.
pub fn serve(root: &Path, options: &[String]) -> Result<(), String> {
    let mut port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let mut source = default_source(root);
    let mut options = options.iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            "--worktree" => source = Source::Worktree,
            "--port" => {
                port = options
                    .next()
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port takes a port number")?;
            }
            other => {
                return Err(format!(
                    "unknown option `{other}`\n\nusage: cargo xtask dashboard [--port N] [--worktree]"
                ));
            }
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("couldn't listen on 127.0.0.1:{port}: {e}"))?;
    let server = Arc::new(Server::new(root, source.clone(), port));
    match &source {
        Source::Ref(name) => {
            eprintln!(
                "Beatlayer progress on http://localhost:{port}, reading {name}, fetched every {} seconds",
                FETCH_EVERY.as_secs()
            );
            let fetcher = Arc::clone(&server);
            thread::spawn(move || {
                loop {
                    fetcher.fetch_now();
                    thread::sleep(FETCH_EVERY);
                }
            });
        }
        Source::Worktree => {
            eprintln!(
                "Beatlayer progress on http://localhost:{port}, reading the documents on disk"
            );
        }
    }

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let server = Arc::clone(&server);
        thread::spawn(move || {
            if let Err(e) = server.answer(stream) {
                eprintln!("request failed: {e}");
            }
        });
    }
    Ok(())
}

/// `origin/main` when the checkout has it, otherwise `HEAD`.
fn default_source(root: &Path) -> Source {
    let has_main = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", MAIN])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success());
    Source::Ref(if has_main { MAIN } else { "HEAD" }.to_string())
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Server {
    fn new(root: &Path, source: Source, port: u16) -> Self {
        Self {
            root: root.to_path_buf(),
            source,
            port,
            fetch: Mutex::new(Fetch {
                at: None,
                error: None,
                running: false,
            }),
        }
    }

    /// Fetches `main` from `origin`, unless a fetch is already running.
    fn fetch_now(&self) {
        let Source::Ref(name) = &self.source else {
            return;
        };
        if !name.starts_with("origin/") {
            return;
        }
        {
            let mut fetch = self.fetch.lock().unwrap_or_else(|e| e.into_inner());
            if fetch.running {
                return;
            }
            fetch.running = true;
        }
        let result = git(&self.root, &["fetch", "--quiet", "origin", "main"]);
        let mut fetch = self.fetch.lock().unwrap_or_else(|e| e.into_inner());
        fetch.running = false;
        match result {
            Ok(_) => {
                fetch.at = Some(SystemTime::now());
                fetch.error = None;
            }
            Err(e) => fetch.error = Some(e),
        }
    }

    /// The progress model with where it came from.
    fn progress(&self) -> Result<Value, String> {
        let (docs, source) = match &self.source {
            Source::Worktree => {
                let docs = Docs::read(|path| {
                    std::fs::read_to_string(self.root.join(path))
                        .map_err(|e| format!("couldn't read {path}: {e}"))
                })?;
                let dirty = !git(&self.root, &["status", "--porcelain", "--", "docs"])
                    .unwrap_or_default()
                    .trim()
                    .is_empty();
                let mut source = self.commit("HEAD")?;
                source["ref"] = json!("the files on disk");
                source["uncommitted"] = json!(dirty);
                (docs, source)
            }
            Source::Ref(name) => {
                let docs =
                    Docs::read(|path| git(&self.root, &["show", &format!("{name}:{path}")]))?;
                let mut source = self.commit(name)?;
                let fetch = self.fetch.lock().unwrap_or_else(|e| e.into_inner());
                source["fetched"] = json!(fetch.at.map(unix_seconds));
                source["fetch_error"] = json!(fetch.error);
                (docs, source)
            }
        };
        let mut data = Progress::parse(&docs).to_json();
        data["source"] = source;
        data["generated"] = json!(unix_seconds(SystemTime::now()));
        Ok(data)
    }

    fn commit(&self, name: &str) -> Result<Value, String> {
        let line = git(&self.root, &["log", "-1", "--format=%h%x1f%ct%x1f%s", name])?;
        let mut fields = line.trim_end().splitn(3, '\u{1f}');
        let hash = fields.next().unwrap_or_default();
        let time: u64 = fields.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        let subject = fields.next().unwrap_or_default();
        Ok(json!({ "ref": name, "commit": hash, "committed": time, "subject": subject }))
    }

    fn answer(&self, mut stream: TcpStream) -> Result<(), String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .map_err(|e| e.to_string())?;
        let mut host = None;
        let mut header_bytes = 0;
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            header_bytes += read;
            if read == 0 || line == "\r\n" || line == "\n" || header_bytes > 16 * 1024 {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("host")
            {
                host = Some(value.trim().to_ascii_lowercase());
            }
        }

        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));

        let allowed_hosts = [
            format!("localhost:{}", self.port),
            format!("127.0.0.1:{}", self.port),
            format!("[::1]:{}", self.port),
        ];
        let response = if !host.as_ref().is_some_and(|h| allowed_hosts.contains(h)) {
            reply(
                421,
                "text/plain; charset=utf-8",
                "Use http://localhost on this machine.".into(),
            )
        } else if method != "GET" && method != "HEAD" {
            reply(
                405,
                "text/plain; charset=utf-8",
                "Only GET and HEAD.".into(),
            )
        } else {
            match path {
                "/" | "/index.html" => reply(200, "text/html; charset=utf-8", PAGE.into()),
                "/icon.svg" | "/favicon.ico" => reply(200, "image/svg+xml", ICON.into()),
                "/api/progress" => {
                    if query.split('&').any(|q| q == "fetch=1") {
                        self.fetch_now();
                    }
                    match self.progress() {
                        Ok(data) => reply(200, "application/json", data.to_string()),
                        Err(e) => reply(500, "application/json", json!({ "error": e }).to_string()),
                    }
                }
                _ => reply(404, "text/plain; charset=utf-8", "Not found.".into()),
            }
        };
        let (head, body) = response;
        stream
            .write_all(head.as_bytes())
            .map_err(|e| e.to_string())?;
        if method != "HEAD" {
            stream
                .write_all(body.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        if started.elapsed() > Duration::from_secs(2) {
            eprintln!("{method} {path} took {:?}", started.elapsed());
        }
        Ok(())
    }
}

fn reply(status: u16, content_type: &str, body: String) -> (String, String) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    (head, body)
}
