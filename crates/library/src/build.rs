//! Building a package outside the repository (spec: core-library R3).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::{ItemDef, Manifest, sha256_hex};
use crate::package::{Item, PACKAGE_FILE, Package};

/// A package that was built.
#[derive(Clone, Debug)]
pub struct Built {
    /// The archive.
    pub archive: PathBuf,
    /// Its size in bytes.
    pub size: u64,
    /// Its SHA-256, in hex.
    pub sha256: String,
    /// What's inside it.
    pub package: Package,
}

/// The interpreter scripts run under: `BL_LIBRARY_PYTHON`, else Python.
fn python() -> String {
    std::env::var("BL_LIBRARY_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.to_owned())
}

/// Runs `script` in isolated mode with the seed, the file to write and, for
/// a conversion, the downloaded input.
fn run_script(
    dir: &Path,
    work: &Path,
    item: &ItemDef,
    script: &str,
    out: &Path,
    input: Option<&Path>,
) -> Result<(), String> {
    let mut command = Command::new(python());
    command
        .current_dir(work)
        .arg("-I")
        .arg(dir.join("scripts").join(script))
        .arg(item.source.seed.to_string())
        .arg(out);
    command.args(input);
    let output = command
        .output()
        .map_err(|e| format!("item {}: couldn't run {}: {e}", item.id, python()))?;
    if output.status.success() && out.is_file() {
        return Ok(());
    }
    Err(format!(
        "item {}: {script} failed: {}",
        item.id,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Makes an item's file in `work` and gives its record for the package.
fn make(
    dir: &Path,
    work: &Path,
    item: &ItemDef,
    fetch: &dyn Fn(&str, &Path) -> Result<(), String>,
) -> Result<Item, String> {
    let s = &item.source;
    let out = work.join(&item.file);
    if let Some(url) = &s.url {
        let got = work.join(format!("{}.source", item.id));
        fetch(url, &got).map_err(|e| format!("item {}: {e}", item.id))?;
        let bytes = std::fs::read(&got).map_err(|e| format!("item {}: {e}", item.id))?;
        if sha256_hex(&bytes) != s.sha256.to_lowercase() {
            return Err(format!(
                "item {}: what {url} gives doesn't match its checksum",
                item.id
            ));
        }
        match &s.script {
            Some(script) => run_script(dir, work, item, script, &out, Some(&got))?,
            None => std::fs::write(&out, bytes).map_err(|e| format!("item {}: {e}", item.id))?,
        }
    } else if let Some(script) = &s.script {
        let bytes = std::fs::read(dir.join("scripts").join(script))
            .map_err(|e| format!("item {}: {e}", item.id))?;
        if sha256_hex(&bytes) != s.sha256.to_lowercase() {
            return Err(format!(
                "item {}: {script} has changed since its checksum was set",
                item.id
            ));
        }
        run_script(dir, work, item, script, &out, None)?;
    }
    let bytes = std::fs::read(&out).map_err(|e| format!("item {}: {e}", item.id))?;
    // The Source tag is Beatlayer's, from where the item comes from (R9).
    let source = if s.recorded {
        "Recorded"
    } else if s.url.is_some() {
        "Open-licensed"
    } else {
        "Generated"
    };
    let mut tags = item.tags.clone();
    tags.push(format!("Source|{source}"));
    Ok(Item {
        id: item.id.clone(),
        name: item.name.clone(),
        kind: item.kind,
        label: item
            .label
            .clone()
            .unwrap_or_else(|| item.kind.label().to_owned()),
        file: format!("{}/{}", item.id, item.file),
        sha256: sha256_hex(&bytes),
        size: bytes.len() as u64,
        licence: item.licence.clone(),
        attribution: item.attribution.clone(),
        tags,
    })
}

/// Builds `package` of the manifest in `dir` as an archive in `out`, which
/// must already exist: fetches or generates each item, checks its checksum,
/// writes the package manifest and archives the package. The same manifest
/// and sources give the same archive byte for byte: entries are in name
/// order with no dates, owners or permissions to differ.
///
/// # Errors
///
/// The problems with the package's items (R2), or what went wrong building.
pub fn build(
    manifest: &Manifest,
    dir: &Path,
    package: &str,
    out: &Path,
    fetch: &dyn Fn(&str, &Path) -> Result<(), String>,
) -> Result<Built, String> {
    let problems = manifest.problems(package, dir);
    if !problems.is_empty() {
        return Err(format!(
            "refusing to build {package}:\n  {}",
            problems.join("\n  ")
        ));
    }
    let def = manifest
        .package
        .iter()
        .find(|p| p.id == package)
        .expect("checked above");
    let work = out.join(format!(".work-{package}"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let result = (|| {
        let mut items = Vec::new();
        for item in manifest.item.iter().filter(|i| i.package == package) {
            items.push((make(dir, &work, item, fetch)?, item.file.clone()));
        }
        items.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        let built = Package {
            id: def.id.clone(),
            name: def.name.clone(),
            version: def.version.clone(),
            items: items.iter().map(|(i, _)| i.clone()).collect(),
        };
        let mut entries: Vec<(String, Vec<u8>)> = vec![(
            PACKAGE_FILE.to_owned(),
            serde_json::to_vec_pretty(&built).map_err(|e| e.to_string())?,
        )];
        for (item, file) in &items {
            let bytes = std::fs::read(work.join(file)).map_err(|e| e.to_string())?;
            entries.push((item.file.clone(), bytes));
        }
        entries.sort();
        let archive = out.join(format!("library-{package}.tar.gz"));
        let bytes = archive_of(&entries)?;
        std::fs::write(&archive, &bytes).map_err(|e| format!("{}: {e}", archive.display()))?;
        Ok(Built {
            archive,
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            package: built,
        })
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn archive_of(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        tar.append_data(&mut header, name, bytes.as_slice())
            .map_err(|e| e.to_string())?;
    }
    let tar = tar.into_inner().map_err(|e| e.to_string())?;
    let mut gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::best());
    gz.write_all(&tar).map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}
