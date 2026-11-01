//! The Packages manager (WP-9.1; the stem separation spec, R15 to R21)
//! through the shell, headless: a manifest of test packages, served from
//! files and local servers, stands in for the built-in one.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::packages::Manifest;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-packages-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shell(config: &Path, manifest: &Value) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.use_manifest(Manifest::parse(&manifest.to_string()).unwrap());
    shell
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A package of the manifest, for any platform, kept as `<id>.bin`.
fn package(id: &str, role: &str, url: &str, bytes: &[u8]) -> Value {
    json!({
        "id": id, "name": id, "version": "1", "project": "https://example.com/",
        "purpose": "A test package", "platform": "any", "role": role, "url": url,
        "size": bytes.len(), "sha256": sha256(bytes), "unpack": "file",
        "folder": format!("{id}-1"), "files": [{ "name": format!("{id}.bin") }],
    })
}

/// A package served from a file in `dir`.
fn from_file(dir: &Path, id: &str, role: &str, bytes: &[u8]) -> Value {
    let file = dir.join(format!("{id}.source"));
    std::fs::write(&file, bytes).unwrap();
    package(id, role, &format!("file://{}", file.display()), bytes)
}

/// Serves `bytes` to every request, counting them; the first `hold` bytes,
/// then (if `release` is given) nothing until it's sent to.
fn serve(
    bytes: &'static [u8],
    hold: usize,
    release: Option<Receiver<()>>,
) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/package", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&requests);
    let release = Mutex::new(release);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            count.fetch_add(1, Ordering::Relaxed);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(head.as_bytes());
            if let Some(release) = release.lock().unwrap().as_ref() {
                let _ = stream.write_all(&bytes[..hold]);
                let _ = release.recv();
                let _ = stream.write_all(&bytes[hold..]);
            } else {
                let _ = stream.write_all(bytes);
            }
        }
    });
    (url, requests)
}

fn run(shell: &mut Shell, id: &str, args: Value) -> Value {
    shell.run(id, args).unwrap()
}

fn state_of(listed: &Value, id: &str) -> String {
    listed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("{id} in {listed}"))["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What's in `Packages`, whatever the names.
fn entries(shell: &Shell) -> usize {
    std::fs::read_dir(shell.packages_folder()).map_or(0, Iterator::count)
}

// Spec: stem-separation R15, R17, R18, R20, R21. `stems.install` installs
// the packages the feature needs from the manifest's URLs; `packages.list`
// reflects each install and removal.
#[test]
fn a_manifest_served_from_files_installs_a_features_packages() {
    let scratch = Scratch::new("install");
    let manifest = json!({
        "features": [{ "id": "stems", "name": "Stem separation", "roles": ["runtime", "model"] }],
        "packages": [
            from_file(&scratch.0, "runtime", "runtime", b"the library"),
            from_file(&scratch.0, "model", "model", b"the network"),
        ],
    });
    let mut shell = shell(&scratch.0.join("config"), &manifest);
    let listed = run(&mut shell, "packages.list", Value::Null);
    assert_eq!(state_of(&listed, "runtime"), "not_installed");
    assert_eq!(listed["packages"][0]["features"], json!(["stems"]));
    assert_eq!(listed["packages"][0]["role"], "runtime");

    run(&mut shell, "stems.install", Value::Null);
    shell.wait_stems();
    let listed = run(&mut shell, "packages.list", Value::Null);
    assert_eq!(state_of(&listed, "runtime"), "installed");
    assert_eq!(state_of(&listed, "model"), "installed");
    assert_eq!(
        run(&mut shell, "stems.status", Value::Null)["installed"],
        true
    );
    for folder in ["runtime-1", "model-1"] {
        assert!(
            shell
                .packages_folder()
                .join(folder)
                .join("package.json")
                .is_file()
        );
    }

    run(&mut shell, "packages.remove", json!({ "ids": ["model"] }));
    let listed = run(&mut shell, "packages.list", Value::Null);
    assert_eq!(state_of(&listed, "model"), "not_installed");
    assert!(!shell.packages_folder().join("model-1").exists());
    assert_eq!(
        run(&mut shell, "stems.status", Value::Null)["installed"],
        false
    );

    // A package that isn't in the manifest is refused, by name.
    let refused = shell.run("packages.install", json!({ "ids": ["nothing"] }));
    assert!(refused.is_err_and(|e| e.to_string().contains("nothing")));
}

// Spec: stem-separation R3, R17. A download whose checksum is wrong installs
// nothing, and the status says why.
#[test]
fn a_tampered_checksum_installs_nothing() {
    let scratch = Scratch::new("tampered");
    let mut bad = from_file(&scratch.0, "runtime", "runtime", b"the library");
    bad["sha256"] = json!("0".repeat(64));
    let manifest = json!({
        "features": [{ "id": "stems", "name": "Stem separation", "roles": ["runtime", "model"] }],
        "packages": [bad, from_file(&scratch.0, "model", "model", b"the network")],
    });
    let mut shell = shell(&scratch.0.join("config"), &manifest);
    run(&mut shell, "stems.install", Value::Null);
    shell.wait_stems();
    let status = run(&mut shell, "stems.status", Value::Null);
    assert_eq!(status["install"]["state"], "failed", "{status}");
    assert!(
        status["install"]["error"]
            .as_str()
            .unwrap()
            .contains("SHA-256")
    );
    assert_eq!(status["installed"], false);
    // The failure stopped the download waiting behind it.
    assert_eq!(entries(&shell), 0);
}

// Spec: stem-separation R17. Cancel mid-download leaves no file in
// `Packages`.
#[test]
fn cancel_mid_download_leaves_no_file() {
    static LIBRARY: [u8; 1 << 20] = [7; 1 << 20];
    let scratch = Scratch::new("cancel");
    let (release, held) = channel();
    let (url, _) = serve(&LIBRARY, 1 << 17, Some(held));
    let manifest = json!({
        "features": [{ "id": "stems", "name": "Stem separation", "roles": ["runtime", "model"] }],
        "packages": [
            package("runtime", "runtime", &url, &LIBRARY),
            from_file(&scratch.0, "model", "model", b"the network"),
        ],
    });
    let mut shell = shell(&scratch.0.join("config"), &manifest);
    run(&mut shell, "stems.install", Value::Null);
    // Until the server's first bytes have arrived.
    for _ in 0..1_000_000 {
        shell.poll();
        let listed = run(&mut shell, "packages.list", Value::Null);
        if listed["packages"][0]["progress"].as_f64().unwrap_or(0.0) > 0.0 {
            break;
        }
        std::thread::yield_now();
    }
    let started = run(&mut shell, "packages.list", Value::Null);
    assert_eq!(state_of(&started, "runtime"), "downloading", "{started}");
    assert_eq!(state_of(&started, "model"), "queued");
    run(&mut shell, "stems.cancel", Value::Null);
    release.send(()).unwrap();
    shell.wait_stems();
    let status = run(&mut shell, "stems.status", Value::Null);
    assert_eq!(status["install"]["state"], "cancelled", "{status}");
    assert_eq!(entries(&shell), 0);
}

// Spec: stem-separation R17. A package two features ask for at once
// downloads once.
#[test]
fn two_features_sharing_a_runtime_download_it_once() {
    static LIBRARY: [u8; 4096] = [3; 4096];
    let scratch = Scratch::new("shared");
    let (url, requests) = serve(&LIBRARY, 0, None);
    let manifest = json!({
        "features": [
            { "id": "stems", "name": "Stem separation", "roles": ["runtime", "model"] },
            { "id": "pitch", "name": "Audio to MIDI", "roles": ["runtime", "pitch-model"] },
        ],
        "packages": [
            package("runtime", "runtime", &url, &LIBRARY),
            from_file(&scratch.0, "model", "model", b"the network"),
            from_file(&scratch.0, "pitch", "pitch-model", b"the pitches"),
        ],
    });
    let mut shell = shell(&scratch.0.join("config"), &manifest);
    run(&mut shell, "stems.install", Value::Null);
    run(
        &mut shell,
        "packages.install",
        json!({ "ids": ["runtime", "pitch"] }),
    );
    // A package that's waiting can't be removed.
    let busy = shell.run("packages.remove", json!({ "ids": ["pitch"] }));
    assert!(busy.is_err_and(|e| e.to_string().contains("downloading")));
    let listed = run(&mut shell, "packages.list", Value::Null);
    assert_eq!(listed["packages"][0]["features"], json!(["stems", "pitch"]));
    assert_eq!(state_of(&listed, "pitch"), "queued");
    shell.wait_stems();
    let listed = run(&mut shell, "packages.list", Value::Null);
    for id in ["runtime", "model", "pitch"] {
        assert_eq!(state_of(&listed, id), "installed", "{id}");
    }
    assert_eq!(requests.load(Ordering::Relaxed), 1);
}

// Spec: stem-separation R14, R21. A headless run keeps its packages in the
// system's temporary folder.
#[test]
fn a_headless_run_keeps_its_packages_in_the_temporary_folder() {
    let folder = Shell::headless().packages_folder();
    assert!(
        folder.starts_with(std::env::temp_dir()),
        "{}",
        folder.display()
    );
    assert!(folder.ends_with("Packages"));
}
