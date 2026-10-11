//! `cargo xtask package`: builds this platform's packages with cargo-packager,
//! then starts each packaged binary to check it runs.
//!
//! | Platform | Packages |
//! |---|---|
//! | macOS | `Beatlayer.app` and a DMG holding it |
//! | Windows | an MSI |
//! | Linux | an AppImage and a `.tar.gz` |
//!
//! The configuration is `[package.metadata.packager]` in crates/app/Cargo.toml.
//! This task adds what only the build knows: the version, the build and output
//! directories, and the signing identity when the signing secrets exist.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::{APP, cargo_bin, cargo_metadata};

/// The cargo-packager release the packages are built with. The nightly
/// workflow installs it and README.md names it; change all three together.
const CARGO_PACKAGER_VERSION: &str = "0.11.8";

/// The packages' directory, inside the target directory.
const OUT_DIR: &str = "packages";

/// The Uniform Type Identifier and extension of a Beatlayer project.
const PROJECT_UTI: &str = "io.loombridge.beatlayer.project";
const PROJECT_EXTENSION: &str = "blset";

/// The only architecture the macOS app runs on.
const MACOS_ARCH: &str = "arm64";

/// A tool that would otherwise be downloaded while packaging, pinned to a
/// fixed release and checked against its SHA-256 before every use.
struct PinnedTool {
    /// The file name cargo-packager or appimagetool looks for.
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

// The AppImage tools. cargo-packager 0.11.8 downloads linuxdeploy, its AppImage
// plug-in, and AppRun from tags that move, and the plug-in's appimagetool
// downloads the AppImage runtime from another, so the AppImage would be built
// with whatever those tags held on the day. These pins replace all four
// downloads; `prepare_appimage_tools` explains how. Each digest is the one
// GitHub records for the release asset. To update a tool, change its URL and
// digest together.

/// linuxdeploy, which assembles the AppImage. Its release bundles
/// linuxdeploy-plugin-appimage, and appimagetool inside that, so this pin
/// covers the plug-in too.
const LINUXDEPLOY: PinnedTool = PinnedTool {
    file: "linuxdeploy-x86_64.AppImage",
    url: "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage",
    sha256: "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d",
};

/// The AppRun launcher at the root of the AppImage: the copy cargo-packager
/// uses, from Tauri's mirror of AppImageKit.
const APPRUN: PinnedTool = PinnedTool {
    file: "AppRun-x86_64",
    url: "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64",
    sha256: "f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f",
};

/// The AppImage runtime, the loader at the start of every AppImage.
/// appimagetool takes it from `LDAI_RUNTIME_FILE` instead of downloading one.
const APPIMAGE_RUNTIME: PinnedTool = PinnedTool {
    file: "runtime-x86_64",
    url: "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64",
    sha256: "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
};

/// What cargo-packager calls the AppImage plug-in it would download.
const APPIMAGE_PLUGIN: &str = "linuxdeploy-plugin-appimage.AppImage";

/// Variables that sign, notarise, or name a signing identity. GitHub Actions
/// passes a missing secret as an empty string, so an empty variable counts as
/// unset and is removed before cargo-packager sees it.
const SIGNING_VARIABLES: &[&str] = &[
    "APPLE_SIGNING_IDENTITY",
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_PATH",
    "APPLE_KEYCHAIN_PROFILE",
    "WINDOWS_SIGN_COMMAND",
];

/// Keys this task sets, which the table must leave out.
const SET_HERE: &[(Option<&str>, &[&str])] = &[
    (
        None,
        &[
            "version",
            "out-dir",
            "outDir",
            "binaries-dir",
            "binariesDir",
        ],
    ),
    (Some("macos"), &["signing-identity", "signingIdentity"]),
    (
        Some("windows"),
        &[
            "sign-command",
            "signCommand",
            "certificate-thumbprint",
            "certificateThumbprint",
        ],
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    fn current() -> Result<Self, String> {
        match std::env::consts::OS {
            "macos" => Ok(Self::MacOs),
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            other => Err(format!("packaging isn't set up for {other}")),
        }
    }

    /// cargo-packager's formats. The Linux tarball is made here instead.
    fn formats(self) -> &'static str {
        match self {
            Self::MacOs => "app,dmg",
            Self::Windows => "wix",
            Self::Linux => "appimage",
        }
    }
}

/// Signing settings, from the environment.
#[derive(Debug, Default)]
struct Signing {
    /// A Developer ID identity for codesign, such as
    /// `Developer ID Application: Name (TEAMID)`.
    macos_identity: Option<String>,
    /// A command that signs the file at `%1` (a maintainer chooses the route).
    windows_command: Option<String>,
}

impl Signing {
    fn from_env() -> Self {
        Self {
            macos_identity: non_empty_var("APPLE_SIGNING_IDENTITY"),
            windows_command: non_empty_var("WINDOWS_SIGN_COMMAND"),
        }
    }

    fn report(&self, platform: Platform) {
        match platform {
            Platform::MacOs => match &self.macos_identity {
                Some(_) if notarisation_configured() => {
                    eprintln!(
                        "Signing with the Developer ID in APPLE_SIGNING_IDENTITY, then notarising"
                    );
                }
                Some(_) => eprintln!(
                    "Signing with the Developer ID in APPLE_SIGNING_IDENTITY. Notarisation skipped: \
                     set APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID, or the APPLE_API_* variables"
                ),
                None => eprintln!(
                    "Signing skipped: APPLE_SIGNING_IDENTITY isn't set (a Developer ID). \
                     The packages are unsigned."
                ),
            },
            Platform::Windows => match &self.windows_command {
                Some(_) => eprintln!("Signing with WINDOWS_SIGN_COMMAND"),
                None => eprintln!(
                    "Signing skipped: WINDOWS_SIGN_COMMAND isn't set (a Windows signing route). \
                     The packages are unsigned."
                ),
            },
            Platform::Linux => eprintln!("Linux packages aren't signed"),
        }
    }
}

fn notarisation_configured() -> bool {
    let all = |names: &[&str]| names.iter().all(|name| non_empty_var(name).is_some());
    all(&["APPLE_KEYCHAIN_PROFILE"])
        || all(&["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"])
        || all(&["APPLE_API_KEY", "APPLE_API_ISSUER"])
}

fn non_empty_var(name: &str) -> Option<String> {
    non_empty(std::env::var(name).ok())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Builds the packages for this platform and smoke-tests them.
pub fn package(root: &Path) -> Result<(), String> {
    let platform = Platform::current()?;
    require_cargo_packager()?;

    let metadata = cargo_metadata(root)?;
    let app = metadata["packages"]
        .as_array()
        .and_then(|packages| packages.iter().find(|p| p["name"] == APP))
        .ok_or_else(|| format!("`cargo metadata` lists no `{APP}` package"))?;
    let version = app["version"]
        .as_str()
        .ok_or("the app package has no version")?;
    let manifest_dir = app["manifest_path"]
        .as_str()
        .and_then(|path| Path::new(path).parent())
        .ok_or("the app package has no manifest path")?;
    let table = &app["metadata"]["packager"];
    if !table.is_object() {
        return Err(format!(
            "{} has no [package.metadata.packager] table",
            manifest_dir.join("Cargo.toml").display()
        ));
    }
    let target_dir = metadata["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or("`cargo metadata` gives no target directory")?;
    let binaries_dir = target_dir.join("release");
    let out_dir = target_dir.join(OUT_DIR);
    let product = string_at(table, &["product-name"])?;
    let binaries = binaries(table)?;
    let main_binary = binaries[0].as_str();
    let expected_version = format!("{main_binary} {version}");

    eprintln!("\n==> build release binaries");
    let mut build = Command::new(cargo_bin());
    build
        .args(["build", "--release", "--locked"])
        .current_dir(root);
    for binary in &binaries {
        build.args(["--bin", binary]);
    }
    if platform == Platform::MacOs {
        // Builds against the oldest macOS the Info.plist allows.
        let minimum = string_at(table, &["macos", "minimum-system-version"])?;
        build.env("MACOSX_DEPLOYMENT_TARGET", minimum);
    }
    run(&mut build, "cargo build")?;

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .map_err(|e| format!("couldn't clear {}: {e}", out_dir.display()))?;
    }
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("couldn't create {}: {e}", out_dir.display()))?;

    let appimage_tools = if platform == Platform::Linux {
        eprintln!("\n==> pinned AppImage tools");
        Some(prepare_appimage_tools(&target_dir)?)
    } else {
        None
    };

    eprintln!("\n==> cargo packager ({})", platform.formats());
    let signing = Signing::from_env();
    signing.report(platform);
    let config = packager_config(table, version, &binaries_dir, &out_dir, &signing)?;
    let mut packager = Command::new(cargo_bin());
    packager
        .args(["packager", "--formats", platform.formats(), "--config"])
        .arg(config.to_string())
        // Relative paths in the table are relative to crates/app/Cargo.toml.
        .current_dir(manifest_dir);
    for name in SIGNING_VARIABLES {
        if non_empty_var(name).is_none() {
            packager.env_remove(name);
        }
    }
    match platform {
        // With CI set, create-dmg skips laying out the DMG window through
        // Finder, which needs Automation permission. CI always skips it, so
        // this makes a local DMG match the nightly one.
        Platform::MacOs => {
            packager.env("CI", "true");
        }
        // linuxdeploy and its plug-ins are AppImages. Extracting them avoids
        // needing FUSE, which CI runners don't have.
        Platform::Linux => {
            packager.env("APPIMAGE_EXTRACT_AND_RUN", "1");
        }
        Platform::Windows => {}
    }
    if let Some(tools) = &appimage_tools {
        packager
            .env("XDG_CACHE_HOME", &tools.cache_home)
            .env("LDAI_RUNTIME_FILE", &tools.runtime);
    }
    run(&mut packager, "cargo packager")?;
    if let Some(tools) = &appimage_tools {
        tools.check_untouched()?;
    }

    let scratch = out_dir.join(".smoke-test");
    let mut packages = Vec::new();
    match platform {
        Platform::MacOs => {
            let dmg = find_one(&out_dir, ".dmg")?;
            eprintln!("\n==> check the app in {}", file_name(&dmg));
            let entitlements = manifest_dir.join(string_at(table, &["macos", "entitlements"])?);
            report_problems(
                "the entitlements",
                check_entitlements(&plist_json(&entitlements)?),
            )?;
            smoke_test_dmg(
                &dmg,
                &scratch,
                &product,
                main_binary,
                &expected_version,
                table,
            )?;
            packages.push(dmg);
        }
        Platform::Windows => {
            let msi = find_one(&out_dir, ".msi")?;
            eprintln!("\n==> check {}", file_name(&msi));
            smoke_test_msi(&msi, &scratch, main_binary, &expected_version)?;
            packages.push(msi);
        }
        Platform::Linux => {
            let appimage = find_one(&out_dir, ".AppImage")?;
            eprintln!("\n==> check {}", file_name(&appimage));
            let mut command = Command::new(&appimage);
            command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
            check_version(&mut command, &expected_version)?;
            packages.push(appimage);

            eprintln!("\n==> tarball");
            let tarball = make_tarball(
                &out_dir,
                &binaries_dir,
                &binaries,
                &resources(table, manifest_dir)?,
                version,
            )?;
            smoke_test_tarball(&tarball, &scratch, main_binary, &expected_version)?;
            packages.push(tarball);
        }
    }
    remove_dir_if_present(&scratch)?;

    eprintln!("\n==> packages");
    for path in &packages {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        eprintln!("{} ({:.1} MB)", path.display(), size as f64 / 1e6);
    }
    Ok(())
}

/// cargo-packager's configuration: the table, plus the version, directories,
/// and signing settings.
fn packager_config(
    table: &Value,
    version: &str,
    binaries_dir: &Path,
    out_dir: &Path,
    signing: &Signing,
) -> Result<Value, String> {
    let mut config = table
        .as_object()
        .cloned()
        .ok_or("[package.metadata.packager] isn't a table")?;

    for (section, keys) in SET_HERE {
        let object = match section {
            Some(section) => config.get(*section).and_then(Value::as_object),
            None => Some(&config),
        };
        if let Some(key) = keys
            .iter()
            .find(|key| object.is_some_and(|o| o.contains_key(**key)))
        {
            let name = section.map_or_else(|| (*key).to_owned(), |s| format!("{s}.{key}"));
            return Err(format!(
                "`{name}` in [package.metadata.packager] is set by `cargo xtask package`; remove it"
            ));
        }
    }

    config.insert("version".into(), version.into());
    config.insert("outDir".into(), path_value(out_dir)?);
    config.insert("binariesDir".into(), path_value(binaries_dir)?);
    if let Some(identity) = &signing.macos_identity {
        section(&mut config, "macos")?.insert("signingIdentity".into(), identity.as_str().into());
    }
    if let Some(command) = &signing.windows_command {
        section(&mut config, "windows")?.insert("signCommand".into(), command.as_str().into());
    }
    Ok(Value::Object(config))
}

fn section<'a>(
    config: &'a mut Map<String, Value>,
    name: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    config
        .entry(name)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("[package.metadata.packager.{name}] isn't a table"))
}

fn path_value(path: &Path) -> Result<Value, String> {
    path.to_str()
        .map(Value::from)
        .ok_or_else(|| format!("{} isn't valid UTF-8", path.display()))
}

/// Looks up a key path in the table, written in kebab case. cargo-packager
/// also accepts camel case, so that's tried second.
fn lookup<'a>(table: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(table, |value, key| {
        value.get(*key).or_else(|| value.get(camel_case(key)))
    })
}

fn camel_case(kebab: &str) -> String {
    let mut parts = kebab.split('-');
    let mut out = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn string_at(table: &Value, path: &[&str]) -> Result<String, String> {
    lookup(table, path)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "[package.metadata.packager] needs `{}` as a string",
                path.join(".")
            )
        })
}

/// The packaged binaries' names, the main one first.
fn binaries(table: &Value) -> Result<Vec<String>, String> {
    let entries = lookup(table, &["binaries"])
        .and_then(Value::as_array)
        .ok_or("[package.metadata.packager] needs a `binaries` list")?;
    let mut names = Vec::new();
    for entry in entries {
        let name = entry["path"]
            .as_str()
            .ok_or("each entry in `binaries` needs a `path`")?;
        if entry["main"] == true {
            names.insert(0, name.to_owned());
        } else {
            names.push(name.to_owned());
        }
    }
    let mains = entries.iter().filter(|e| e["main"] == true).count();
    if mains == 1 {
        Ok(names)
    } else {
        Err(format!(
            "`binaries` needs exactly one entry with `main = true`, not {mains}"
        ))
    }
}

/// The files listed in `resources`, which the tarball carries too.
fn resources(table: &Value, manifest_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let Some(entries) = lookup(table, &["resources"]) else {
        return Ok(Vec::new());
    };
    entries
        .as_array()
        .ok_or("`resources` must be a list")?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(|path| manifest_dir.join(path))
                .ok_or_else(|| "the tarball supports only plain paths in `resources`".to_owned())
        })
        .collect()
}

fn require_cargo_packager() -> Result<(), String> {
    let install = format!("cargo install --locked cargo-packager@{CARGO_PACKAGER_VERSION}");
    let output = Command::new(cargo_bin())
        .args(["packager", "--version"])
        .output();
    let installed = match &output {
        Ok(output) if output.status.success() => {
            installed_version(&String::from_utf8_lossy(&output.stdout)).map(str::to_owned)
        }
        _ => None,
    };
    match installed {
        Some(version) if version == CARGO_PACKAGER_VERSION => Ok(()),
        Some(version) => Err(format!(
            "cargo-packager {version} is installed, but packaging is pinned to \
             {CARGO_PACKAGER_VERSION}. Install it with `{install} --force`."
        )),
        None => Err(format!(
            "cargo-packager isn't installed. Install it with `{install}`."
        )),
    }
}

/// The version in `cargo packager --version` output, `cargo-packager 0.11.8`.
fn installed_version(output: &str) -> Option<&str> {
    output
        .strip_prefix("cargo-packager ")?
        .split_whitespace()
        .next()
}

/// Problems with an app's Info.plist, as JSON from `plutil`.
fn check_info_plist(info: &Value, table: &Value, executable: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut expect = |key: &str, want: Option<&str>| {
        if let Some(want) = want
            && info[key] != want
        {
            problems.push(format!("{key} is {}, not \"{want}\"", info[key]));
        }
    };
    expect(
        "CFBundleIdentifier",
        lookup(table, &["identifier"]).and_then(Value::as_str),
    );
    expect(
        "LSMinimumSystemVersion",
        lookup(table, &["macos", "minimum-system-version"]).and_then(Value::as_str),
    );
    expect("CFBundleExecutable", Some(executable));

    if info["LSArchitecturePriority"] != Value::from(vec![MACOS_ARCH]) {
        problems.push(format!(
            "LSArchitecturePriority is {}, not [\"{MACOS_ARCH}\"]",
            info["LSArchitecturePriority"]
        ));
    }
    if info["NSMicrophoneUsageDescription"]
        .as_str()
        .is_none_or(|text| text.trim().is_empty())
    {
        problems.push("NSMicrophoneUsageDescription is missing".into());
    }

    let lists = |value: &Value, item: &str| {
        value
            .as_array()
            .is_some_and(|items| items.iter().any(|i| i == item))
    };
    let opens_projects = info["CFBundleDocumentTypes"]
        .as_array()
        .is_some_and(|types| {
            types
                .iter()
                .any(|t| lists(&t["LSItemContentTypes"], PROJECT_UTI))
        });
    if !opens_projects {
        problems.push(format!("CFBundleDocumentTypes doesn't list {PROJECT_UTI}"));
    }
    let declares_projects = info["UTExportedTypeDeclarations"]
        .as_array()
        .is_some_and(|types| {
            types.iter().any(|t| {
                t["UTTypeIdentifier"] == PROJECT_UTI
                    && lists(
                        &t["UTTypeTagSpecification"]["public.filename-extension"],
                        PROJECT_EXTENSION,
                    )
            })
        });
    if !declares_projects {
        problems.push(format!(
            "UTExportedTypeDeclarations doesn't declare {PROJECT_UTI} for .{PROJECT_EXTENSION}"
        ));
    }
    problems
}

/// Problems with the hardened-runtime entitlements, as JSON from `plutil`.
fn check_entitlements(entitlements: &Value) -> Vec<String> {
    [
        "com.apple.security.device.audio-input",
        "com.apple.security.cs.disable-library-validation",
    ]
    .iter()
    .filter(|key| entitlements[**key] != true)
    .map(|key| format!("{key} isn't true"))
    .collect()
}

fn report_problems(what: &str, problems: Vec<String>) -> Result<(), String> {
    if problems.is_empty() {
        eprintln!("{what}: ok");
        Ok(())
    } else {
        Err(format!("{what}:\n  {}", problems.join("\n  ")))
    }
}

/// Reads a property list as JSON, through macOS's `plutil`.
fn plist_json(path: &Path) -> Result<Value, String> {
    let output = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .map_err(|e| format!("couldn't run plutil: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "plutil couldn't read {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("couldn't parse plutil's JSON for {}: {e}", path.display()))
}

/// Mounts the DMG read-only, checks the app inside, and runs its binary.
fn smoke_test_dmg(
    dmg: &Path,
    scratch: &Path,
    product: &str,
    executable: &str,
    expected_version: &str,
    table: &Value,
) -> Result<(), String> {
    let mount_point = scratch.join("dmg");
    fs::create_dir_all(&mount_point)
        .map_err(|e| format!("couldn't create {}: {e}", mount_point.display()))?;
    run(
        Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(&mount_point)
            .arg(dmg)
            .stdout(Stdio::null()),
        "hdiutil attach",
    )?;
    let _mounted = Mounted(mount_point.clone());

    let app = mount_point.join(format!("{product}.app"));
    let info = plist_json(&app.join("Contents/Info.plist"))?;
    report_problems("Info.plist", check_info_plist(&info, table, executable))?;

    let binary = app.join("Contents/MacOS").join(executable);
    let archs = Command::new("lipo")
        .arg("-archs")
        .arg(&binary)
        .output()
        .map_err(|e| format!("couldn't run lipo: {e}"))?;
    let archs = String::from_utf8_lossy(&archs.stdout).trim().to_owned();
    if archs != MACOS_ARCH {
        return Err(format!(
            "{} is built for `{archs}`, not `{MACOS_ARCH}`",
            binary.display()
        ));
    }
    eprintln!("architecture: {archs}");

    check_version(&mut Command::new(&binary), expected_version)
}

/// A mounted disk image, detached when dropped.
struct Mounted(PathBuf);

impl Drop for Mounted {
    fn drop(&mut self) {
        // Spotlight or a scan can hold the volume for a moment after use.
        for attempt in 0..5 {
            let mut detach = Command::new("hdiutil");
            detach.arg("detach");
            if attempt == 4 {
                detach.arg("-force");
            }
            let detached = detach
                .arg(&self.0)
                .stdout(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if detached {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
        eprintln!("warning: couldn't detach {}", self.0.display());
    }
}

/// Extracts the MSI with an administrative install, which copies its files
/// without installing anything, and runs the binary.
fn smoke_test_msi(
    msi: &Path,
    scratch: &Path,
    executable: &str,
    expected_version: &str,
) -> Result<(), String> {
    let target = scratch.join("msi");
    fs::create_dir_all(&target)
        .map_err(|e| format!("couldn't create {}: {e}", target.display()))?;
    let log = scratch.join("msiexec.log");
    let mut msiexec = Command::new("msiexec");
    msiexec.arg("/a").arg(msi).arg("/qn").arg("/l*v").arg(&log);
    target_dir_arg(&mut msiexec, &target);
    if let Err(message) = run(&mut msiexec, "msiexec /a") {
        let log = fs::read(&log)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        return Err(format!("{message}\n{log}"));
    }

    let name = format!("{executable}{}", std::env::consts::EXE_SUFFIX);
    let binary = find_file(&target, &name).ok_or_else(|| format!("the MSI has no {name}"))?;
    check_version(&mut Command::new(binary), expected_version)
}

/// msiexec reads `PROPERTY="value"` itself, so on Windows the quotes go in as
/// written rather than through the usual argument quoting.
#[cfg(windows)]
fn target_dir_arg(command: &mut Command, dir: &Path) {
    use std::os::windows::process::CommandExt;
    command.raw_arg(format!("TARGETDIR=\"{}\"", dir.display()));
}

#[cfg(not(windows))]
fn target_dir_arg(command: &mut Command, dir: &Path) {
    command.arg(format!("TARGETDIR={}", dir.display()));
}

/// The pinned AppImage tools, ready for cargo-packager.
struct AppImageTools {
    /// Given to cargo-packager as `XDG_CACHE_HOME`: its tools folder is
    /// `.cargo-packager/AppImage` inside this.
    cache_home: PathBuf,
    /// Given to appimagetool as `LDAI_RUNTIME_FILE`.
    runtime: PathBuf,
}

impl AppImageTools {
    fn tools_dir(cache_home: &Path) -> PathBuf {
        cache_home.join(".cargo-packager").join("AppImage")
    }

    /// Fails if cargo-packager added or replaced a tool, which would mean it
    /// downloaded one instead of using the pinned copies.
    fn check_untouched(&self) -> Result<(), String> {
        let dir = Self::tools_dir(&self.cache_home);
        let mut found: Vec<String> = fs::read_dir(&dir)
            .map_err(|e| format!("couldn't read {}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .map(|entry| file_name(&entry.path()))
            .collect();
        found.sort();
        let mut expected = vec![LINUXDEPLOY.file, APPRUN.file, APPIMAGE_PLUGIN];
        expected.sort_unstable();
        if found == expected && dir.join(APPIMAGE_PLUGIN).is_dir() {
            eprintln!("cargo-packager used the pinned AppImage tools");
            Ok(())
        } else {
            Err(format!(
                "cargo-packager changed its AppImage tools in {}: found {found:?}",
                dir.display()
            ))
        }
    }
}

/// Puts checked copies of the pinned AppImage tools where cargo-packager looks
/// for them, so it downloads none of its own.
///
/// cargo-packager downloads a tool only if its tools folder has nothing by
/// that name. linuxdeploy and AppRun go there as checked copies, fresh each
/// run, because cargo-packager's script edits linuxdeploy's header in place.
/// The AppImage plug-in's place gets an empty folder: cargo-packager then
/// skips that download, and linuxdeploy, which passes over folders when it
/// looks for plug-ins, falls back to the plug-in bundled in its own pinned
/// release. The runtime stays in the download folder, for appimagetool.
fn prepare_appimage_tools(target_dir: &Path) -> Result<AppImageTools, String> {
    if std::env::consts::ARCH != "x86_64" {
        return Err(format!(
            "the AppImage tools are pinned for x86_64 only, not {}",
            std::env::consts::ARCH
        ));
    }
    let downloads = target_dir.join("appimage-tools");
    let cache_home = downloads.join("cache");
    let tools = AppImageTools::tools_dir(&cache_home);
    remove_dir_if_present(&cache_home)?;
    fs::create_dir_all(tools.join(APPIMAGE_PLUGIN))
        .map_err(|e| format!("couldn't create {}: {e}", tools.display()))?;

    for tool in [&LINUXDEPLOY, &APPRUN] {
        let source = fetch_pinned(tool, &downloads)?;
        let destination = tools.join(tool.file);
        fs::copy(&source, &destination)
            .map_err(|e| format!("couldn't copy {}: {e}", source.display()))?;
        make_executable(&destination)?;
    }
    let runtime = fetch_pinned(&APPIMAGE_RUNTIME, &downloads)?;
    Ok(AppImageTools {
        cache_home,
        runtime,
    })
}

/// The pinned tool's file in `dir`, downloaded first if it isn't there, and
/// checked against its SHA-256 every time. A file that doesn't match is
/// deleted, so a later run downloads it again rather than trusting it.
fn fetch_pinned(tool: &PinnedTool, dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(tool.file);
    if !path.exists() {
        eprintln!("Downloading {}", tool.url);
        let downloaded = run(
            Command::new("curl")
                .args(["--fail", "--silent", "--show-error", "--location"])
                .args(["--retry", "3", "--output"])
                .arg(&path)
                .arg(tool.url),
            "curl",
        );
        if let Err(message) = downloaded {
            let _ = fs::remove_file(&path);
            return Err(message);
        }
    }
    let digest = sha256(&path)?;
    if digest == tool.sha256 {
        eprintln!("{}: SHA-256 matches the pin", tool.file);
        Ok(path)
    } else {
        let _ = fs::remove_file(&path);
        Err(format!(
            "{} from {} has SHA-256 {digest}, not the pinned {}",
            tool.file, tool.url, tool.sha256
        ))
    }
}

/// A file's SHA-256, from coreutils' `sha256sum`.
fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("couldn't run sha256sum: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_digest(&stdout) {
        Some(digest) if output.status.success() => Ok(digest.to_owned()),
        _ => Err(format!(
            "sha256sum couldn't hash {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// The digest at the start of a `sha256sum` line, in lowercase hex.
fn parse_digest(line: &str) -> Option<&str> {
    let digest = line.split_whitespace().next()?;
    // sha256sum marks a line whose file name it had to escape with a backslash.
    let digest = digest.strip_prefix('\\').unwrap_or(digest);
    (digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(digest)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("couldn't make {} executable: {e}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Makes `<main>_<version>_<arch>.tar.gz`, named like the AppImage, holding
/// one folder with the binaries and the resources.
fn make_tarball(
    out_dir: &Path,
    binaries_dir: &Path,
    binaries: &[String],
    resources: &[PathBuf],
    version: &str,
) -> Result<PathBuf, String> {
    let name = format!("{}_{version}_{}", binaries[0], std::env::consts::ARCH);
    let staging = out_dir.join(".tarball");
    let folder = staging.join(&name);
    fs::create_dir_all(&folder)
        .map_err(|e| format!("couldn't create {}: {e}", folder.display()))?;

    let binaries = binaries.iter().map(|b| binaries_dir.join(b));
    for source in binaries.chain(resources.iter().cloned()) {
        let destination = folder.join(source.file_name().unwrap_or_default());
        // fs::copy keeps the permission bits, so binaries stay executable.
        fs::copy(&source, &destination)
            .map_err(|e| format!("couldn't copy {}: {e}", source.display()))?;
    }

    let tarball = out_dir.join(format!("{name}.tar.gz"));
    run(
        Command::new("tar")
            .arg("-C")
            .arg(&staging)
            .arg("-czf")
            .arg(&tarball)
            .arg(&name)
            // Stops macOS tar adding AppleDouble files, if run there.
            .env("COPYFILE_DISABLE", "1"),
        "tar",
    )?;
    remove_dir_if_present(&staging)?;
    Ok(tarball)
}

fn smoke_test_tarball(
    tarball: &Path,
    scratch: &Path,
    executable: &str,
    expected_version: &str,
) -> Result<(), String> {
    eprintln!("\n==> check {}", file_name(tarball));
    let target = scratch.join("tarball");
    fs::create_dir_all(&target)
        .map_err(|e| format!("couldn't create {}: {e}", target.display()))?;
    run(
        Command::new("tar")
            .arg("-xzf")
            .arg(tarball)
            .arg("-C")
            .arg(&target),
        "tar",
    )?;
    let binary =
        find_file(&target, executable).ok_or_else(|| format!("the tarball has no {executable}"))?;
    check_version(&mut Command::new(binary), expected_version)
}

/// Runs a packaged binary with `--version` and checks what it prints.
fn check_version(command: &mut Command, expected: &str) -> Result<(), String> {
    let program = Path::new(command.get_program()).to_owned();
    let output = command
        .arg("--version")
        .output()
        .map_err(|e| format!("couldn't run {}: {e}", program.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!("$ {} --version\n{}", program.display(), stdout.trim_end());
    if output.status.success() && stdout.trim() == expected {
        Ok(())
    } else {
        Err(format!(
            "expected `{expected}` from {} --version, got {}: {}{}",
            program.display(),
            output.status,
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The one file in `dir` whose name ends with `suffix`.
fn find_one(dir: &Path, suffix: &str) -> Result<PathBuf, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("couldn't read {}: {e}", dir.display()))?;
    let found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && file_name(path).ends_with(suffix))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!(
            "expected one {suffix} file in {}, found {}",
            dir.display(),
            found.len()
        )),
    }
}

/// Searches `dir` and its subfolders for a file called `name`.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if file_name(&path) == name {
            return Some(path);
        }
    }
    None
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn remove_dir_if_present(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("couldn't remove {}: {e}", dir.display()))?;
    }
    Ok(())
}

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|e| format!("couldn't run {what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table() -> Value {
        json!({
            "product-name": "Beatlayer",
            "identifier": "io.loombridge.beatlayer",
            "binaries": [{ "path": "beatlayer", "main": true }, { "path": "bl-cli" }],
            "macos": { "minimum-system-version": "13.0" },
        })
    }

    #[test]
    fn config_adds_version_and_directories() {
        let config = packager_config(
            &table(),
            "1.2.3",
            Path::new("/t/release"),
            Path::new("/t/packages"),
            &Signing::default(),
        )
        .unwrap();
        assert_eq!(config["version"], "1.2.3");
        assert_eq!(config["binariesDir"], "/t/release");
        assert_eq!(config["outDir"], "/t/packages");
        assert_eq!(config["identifier"], "io.loombridge.beatlayer");
        assert!(config["macos"].get("signingIdentity").is_none());
        assert!(config.get("windows").is_none());
    }

    #[test]
    fn config_signs_only_with_an_identity() {
        let signing = Signing {
            macos_identity: Some("Developer ID Application: Someone (TEAM)".into()),
            windows_command: Some("sign %1".into()),
        };
        let config = packager_config(
            &table(),
            "1.0.0",
            Path::new("/r"),
            Path::new("/p"),
            &signing,
        )
        .unwrap();
        assert_eq!(
            config["macos"]["signingIdentity"],
            "Developer ID Application: Someone (TEAM)"
        );
        assert_eq!(config["macos"]["minimum-system-version"], "13.0");
        assert_eq!(config["windows"]["signCommand"], "sign %1");
    }

    #[test]
    fn config_refuses_keys_set_by_the_task() {
        for (key, value) in [
            ("version", json!("9.9.9")),
            ("out-dir", json!("dist")),
            ("macos", json!({ "signing-identity": "x" })),
            ("windows", json!({ "signCommand": "x" })),
        ] {
            let mut table = table();
            table[key] = value;
            let error = packager_config(
                &table,
                "1.0.0",
                Path::new("/r"),
                Path::new("/p"),
                &Signing::default(),
            )
            .unwrap_err();
            assert!(error.contains("set by `cargo xtask package`"), "{error}");
        }
    }

    #[test]
    fn empty_variables_count_as_unset() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("  ".into())), None);
        assert_eq!(non_empty(Some("x".into())), Some("x".into()));
    }

    #[test]
    fn lookup_accepts_both_spellings() {
        let table = json!({ "macos": { "minimumSystemVersion": "13.0" } });
        assert_eq!(
            lookup(&table, &["macos", "minimum-system-version"]),
            Some(&json!("13.0"))
        );
        assert_eq!(camel_case("info-plist-path"), "infoPlistPath");
    }

    #[test]
    fn binaries_put_the_main_one_first() {
        let table =
            json!({ "binaries": [{ "path": "bl-cli" }, { "path": "beatlayer", "main": true }] });
        assert_eq!(binaries(&table).unwrap(), ["beatlayer", "bl-cli"]);
        let no_main = json!({ "binaries": [{ "path": "bl-cli" }] });
        assert!(binaries(&no_main).is_err());
    }

    #[test]
    fn appimage_tools_come_from_fixed_releases_with_digests() {
        for tool in [&LINUXDEPLOY, &APPRUN, &APPIMAGE_RUNTIME] {
            let release = tool
                .url
                .strip_prefix("https://github.com/")
                .and_then(|rest| rest.split("/releases/download/").nth(1))
                .unwrap_or_else(|| panic!("{} isn't a GitHub release asset", tool.url));
            let (tag, file) = release.split_once('/').unwrap();
            assert_ne!(tag, "continuous", "{} comes from a moving tag", tool.url);
            assert_eq!(file, tool.file, "{}", tool.url);
            assert_eq!(
                parse_digest(tool.sha256),
                Some(tool.sha256),
                "{}",
                tool.file
            );
        }
    }

    #[test]
    fn reads_sha256sum_output() {
        let digest = "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d";
        assert_eq!(parse_digest(&format!("{digest}  file\n")), Some(digest));
        assert_eq!(parse_digest(&format!("\\{digest}  a\\nb\n")), Some(digest));
        assert_eq!(parse_digest(&digest.to_uppercase()), None);
        assert_eq!(parse_digest("abc  file"), None);
        assert_eq!(parse_digest(""), None);
    }

    #[test]
    fn reads_the_installed_packager_version() {
        assert_eq!(installed_version("cargo-packager 0.11.8\n"), Some("0.11.8"));
        assert_eq!(installed_version("something else"), None);
        assert_eq!(installed_version(""), None);
    }

    fn good_info_plist() -> Value {
        json!({
            "CFBundleIdentifier": "io.loombridge.beatlayer",
            "CFBundleExecutable": "beatlayer",
            "LSMinimumSystemVersion": "13.0",
            "LSArchitecturePriority": ["arm64"],
            "NSMicrophoneUsageDescription": "Beatlayer records audio.",
            "CFBundleDocumentTypes": [
                { "LSItemContentTypes": ["io.loombridge.beatlayer.project"] }
            ],
            "UTExportedTypeDeclarations": [{
                "UTTypeIdentifier": "io.loombridge.beatlayer.project",
                "UTTypeTagSpecification": { "public.filename-extension": ["blset"] }
            }],
        })
    }

    #[test]
    fn info_plist_check_passes_a_complete_plist() {
        assert_eq!(
            check_info_plist(&good_info_plist(), &table(), "beatlayer"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn info_plist_check_names_each_problem() {
        let mut info = good_info_plist();
        info["CFBundleIdentifier"] = json!("com.example.other");
        info["LSArchitecturePriority"] = json!(["arm64", "x86_64"]);
        info.as_object_mut()
            .unwrap()
            .remove("NSMicrophoneUsageDescription");
        info["UTExportedTypeDeclarations"][0]["UTTypeTagSpecification"]["public.filename-extension"] =
            json!(["als"]);
        let problems = check_info_plist(&info, &table(), "beatlayer");
        assert_eq!(problems.len(), 4, "{problems:#?}");
    }

    #[test]
    fn entitlements_check_needs_both_keys() {
        let both = json!({
            "com.apple.security.device.audio-input": true,
            "com.apple.security.cs.disable-library-validation": true,
        });
        assert!(check_entitlements(&both).is_empty());
        let one = json!({ "com.apple.security.device.audio-input": true });
        assert_eq!(check_entitlements(&one).len(), 1);
    }

    /// The packaging files are checked as text here, on every platform; on
    /// macOS, `cargo xtask package` checks the built app with `plutil`.
    #[test]
    fn packaging_files_declare_what_the_app_needs() {
        let macos = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/app/packaging/macos");
        let read = |name: &str| fs::read_to_string(macos.join(name)).unwrap();
        let info = read("Info.plist");
        for needle in [
            "<key>LSArchitecturePriority</key>",
            "<string>arm64</string>",
            "<key>NSMicrophoneUsageDescription</key>",
            "<key>CFBundleDocumentTypes</key>",
            "<key>UTExportedTypeDeclarations</key>",
            "<string>io.loombridge.beatlayer.project</string>",
            "<string>blset</string>",
        ] {
            assert!(info.contains(needle), "Info.plist lacks {needle}");
        }
        let entitlements = read("entitlements.plist");
        for key in [
            "com.apple.security.device.audio-input",
            "com.apple.security.cs.disable-library-validation",
        ] {
            assert!(
                entitlements.contains(&format!("<key>{key}</key>\n\t<true/>")),
                "entitlements.plist doesn't set {key}"
            );
        }
    }
}
