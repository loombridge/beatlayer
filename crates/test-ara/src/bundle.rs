//! The test plug-in as a VST3 bundle, for the host's tests.
//!
//! A file of its own, with nothing but the standard library, so the host's
//! tests can take it in with `#[path]` instead of linking this crate beside
//! `bl-test-vst3`: both export the VST3 entry points, and a test binary that
//! links the two has two `GetPluginFactory`s.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// For the host's tests: the test plug-in as a VST3 bundle, made once per
/// test run from the cdylib Cargo built beside the running test binary: on
/// macOS a `.vst3` bundle with its `Info.plist` and its binary under
/// `Contents/MacOS`, on Linux under `Contents/x86_64-linux`, and on Windows
/// under `Contents/x86_64-win`, as the VST3 specification lays them out.
#[must_use]
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let library = library();
        let dir = std::env::temp_dir().join(format!("beatlayer-test-ara-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        let bundle = dir.join("Test ARA Player.vst3");
        let contents = bundle.join("Contents");
        let (folder, name) = if cfg!(target_os = "macos") {
            ("MacOS".to_owned(), "Test ARA Player".to_owned())
        } else if cfg!(target_os = "windows") {
            ("x86_64-win".to_owned(), "Test ARA Player.vst3".to_owned())
        } else {
            (
                "x86_64-linux".to_owned(),
                format!("{}bl_test_ara.so", std::env::consts::DLL_PREFIX),
            )
        };
        let binary = contents.join(&folder);
        std::fs::create_dir_all(&binary).expect("the bundle's folders");
        std::fs::copy(&library, binary.join(name)).expect("the library");
        if cfg!(target_os = "macos") {
            std::fs::write(
                contents.join("Info.plist"),
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Test ARA Player</string>
<key>CFBundleIdentifier</key><string>bl.test.vst3.ara-player</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>
"#,
            )
            .expect("the bundle's Info.plist");
        }
        bundle
    })
}

/// The newest test plug-in library Cargo built beside the running test
/// binary.
fn library() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    let deps = exe.parent().expect("the test binary's folder");
    let prefix = format!("{}bl_test_ara", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = [deps, deps.parent().unwrap_or(deps)]
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(suffix))
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    found.sort();
    found
        .pop()
        .map(|(_, path)| path)
        .expect("Cargo builds the test plug-in's cdylib beside the tests")
}
