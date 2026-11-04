//! Beatlayer's library (the core library spec, R1 to R4): `cargo xtask
//! library` builds the packages the manifest under `library/` lists, outside
//! the repository, and checks the manifest and that no audio entered it.

use std::path::{Path, PathBuf};
use std::process::Command;

use bl_library::Manifest;
use serde_json::json;

/// The manifest and its scripts, from the repository's root.
const DIR: &str = "library";

/// The files the library's folder may hold: its manifest, its scripts and
/// their notes. Anything else, such as a sample, is the library's audio.
const ALLOWED: [&str; 3] = ["toml", "py", "md"];

/// Where the packages are hosted, until the maintainer publishes them.
const URL_BASE: &str = "https://github.com/loombridge/beatlayer/releases/download/library";

/// What `cargo xtask library` was asked.
#[derive(Default)]
pub struct Options {
    check: bool,
    manifest: Option<PathBuf>,
    out: Option<PathBuf>,
    url_base: Option<String>,
    packages: Vec<String>,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value =
                |flag: &str| args.next().cloned().ok_or(format!("{flag} needs a value"));
            match arg.as_str() {
                "--check" => options.check = true,
                "--manifest" => options.manifest = Some(value("--manifest")?.into()),
                "--out" => options.out = Some(value("--out")?.into()),
                "--url-base" => options.url_base = Some(value("--url-base")?),
                flag if flag.starts_with('-') => return Err(format!("unexpected `{flag}`")),
                package => options.packages.push(package.to_owned()),
            }
        }
        Ok(options)
    }
}

/// Fails if the repository's manifest breaks the rules, or the library's folder
/// holds anything but the manifest and its scripts (R2, R4).
pub fn check(root: &Path) -> Result<(), String> {
    let dir = root.join(DIR);
    let manifest = Manifest::load(&dir)?;
    let mut problems = manifest.all_problems(&dir);
    problems.extend(
        not_manifest(&dir)?
            .into_iter()
            .map(|p| format!("{}: audio and other files aren't committed", p.display())),
    );
    if problems.is_empty() {
        eprintln!(
            "{} items in {} packages follow the library's rules",
            manifest.item.len(),
            manifest.package.len()
        );
        return Ok(());
    }
    Err(format!(
        "the library's manifest is wrong:\n  {}",
        problems.join("\n  ")
    ))
}

/// The files under `dir` that aren't manifest, script or note.
fn not_manifest(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                folders.push(path);
            } else if !path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ALLOWED.contains(&e))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Downloads `url` into `to` with curl, which every supported system has.
fn fetch(url: &str, to: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(to)
        .arg(url)
        .status()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or(format!("couldn't download {url}"))
}

/// Builds the packages (R3). With `--check`, only checks the manifest.
pub fn run(root: &Path, options: &Options) -> Result<(), String> {
    if options.check {
        return check(root);
    }
    let dir = options.manifest.clone().unwrap_or_else(|| root.join(DIR));
    let manifest = Manifest::load(&dir)?;
    let out = options
        .out
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("beatlayer-library"));
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // The library's files are never committed (R4): refuse to write them
    // anywhere git would see them.
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    if out.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
        return Err(format!(
            "{} is inside the repository: use --out elsewhere",
            out.display()
        ));
    }
    let url_base = options.url_base.as_deref().unwrap_or(URL_BASE);
    let ids: Vec<&str> = if options.packages.is_empty() {
        manifest.package.iter().map(|p| p.id.as_str()).collect()
    } else {
        options.packages.iter().map(String::as_str).collect()
    };
    for id in ids {
        if !manifest.item.iter().any(|i| i.package == id) && options.packages.is_empty() {
            eprintln!("{id}: no items yet, not built");
            continue;
        }
        let built = bl_library::build(&manifest, &dir, id, &out, &fetch)?;
        let name = format!("library-{id}");
        // The entry the Packages manager's manifest lists the package by.
        let entry = json!({
            "id": name, "name": built.package.name, "version": built.package.version,
            "project": "https://github.com/loombridge/beatlayer",
            "purpose": manifest.package.iter().find(|p| p.id == id).map_or("", |p| p.purpose.as_str()),
            "platform": "any", "role": name,
            "url": format!("{url_base}/{name}.tar.gz"),
            "size": built.size, "sha256": built.sha256,
            "unpack": "tar_gz", "keep_all": true,
            "folder": format!("{name}-{}", built.package.version),
            "files": [{ "name": bl_library::PACKAGE_FILE }],
        });
        let entry_file = out.join(format!("{name}.entry.json"));
        std::fs::write(
            &entry_file,
            serde_json::to_vec_pretty(&entry).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("{}: {e}", entry_file.display()))?;
        eprintln!(
            "{}: {} items, {} bytes, sha256 {}",
            built.archive.display(),
            built.package.items.len(),
            built.size,
            built.sha256
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_library::sha256_hex;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/library/tests/fixtures")
    }

    fn options(args: &[&str]) -> Options {
        Options::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>()).unwrap()
    }

    // Spec: core-library R3. A package builds from a test manifest, outside
    // the repository, with the entry the Packages manager lists it by.
    #[test]
    fn a_package_builds_from_a_test_manifest_outside_the_repository() {
        let out = std::env::temp_dir().join(format!("bl-xtask-library-{}", std::process::id()));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let fixtures = fixtures();
        let (manifest, out_arg) = (fixtures.to_string_lossy(), out.to_string_lossy());
        run(
            &root,
            &options(&["--manifest", &manifest, "--out", &out_arg, "core"]),
        )
        .unwrap();
        let archive = std::fs::read(out.join("library-core.tar.gz")).unwrap();
        let entry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("library-core.entry.json")).unwrap())
                .unwrap();
        assert_eq!(entry["sha256"], sha256_hex(&archive));
        assert_eq!(entry["size"], archive.len());
        assert_eq!(entry["role"], "library-core");
        let inside = root.join("target").join("library-out");
        let inside_arg = inside.to_string_lossy();
        let error = run(
            &root,
            &options(&["--manifest", &manifest, "--out", &inside_arg, "core"]),
        );
        assert!(error.unwrap_err().contains("inside the repository"));
        let _ = std::fs::remove_dir_all(inside);
    }

    // Spec: core-library R4. Audio in the library's folder fails the check;
    // the repository's own manifest passes.
    #[test]
    fn audio_in_the_librarys_folder_fails_the_check() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        check(&root).unwrap();
        let dir = std::env::temp_dir().join(format!("bl-xtask-audio-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("library")).unwrap();
        std::fs::write(dir.join("library/a.toml"), "").unwrap();
        std::fs::write(dir.join("library/kick.wav"), b"RIFF").unwrap();
        let error = check(&dir).unwrap_err();
        assert!(error.contains("kick.wav"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
