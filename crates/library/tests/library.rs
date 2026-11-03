//! Spec: core-library R1 to R4, R11, R12.

use std::path::{Path, PathBuf};

use bl_library::{Manifest, PACKAGE_FILE, Package, build, credits_text, sha256_hex};

/// A change to the fixture manifest's text.
type Change = fn(&str) -> String;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn no_fetch(url: &str, _: &Path) -> Result<(), String> {
    Err(format!("nothing is fetched here: {url}"))
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The fixture manifest with `change` applied to its text.
fn changed(change: impl Fn(&str) -> String) -> Manifest {
    let text = std::fs::read_to_string(fixtures().join("test.toml")).unwrap();
    toml_manifest(&change(&text))
}

fn toml_manifest(text: &str) -> Manifest {
    let dir = out("manifest");
    std::fs::write(dir.join("m.toml"), text).unwrap();
    Manifest::load(&dir).unwrap()
}

fn refused(manifest: &Manifest) -> String {
    build(manifest, &fixtures(), "core", &out("refused"), &no_fetch)
        .expect_err("the package should be refused")
}

// R3, R4: the package holds each item's file and a manifest of their SHA-256.
#[test]
fn a_package_is_built_from_the_test_manifest() {
    let manifest = Manifest::load(&fixtures()).unwrap();
    let dir = out("built");
    let built = build(&manifest, &fixtures(), "core", &dir, &no_fetch).unwrap();
    assert_eq!(built.package.items.len(), 2);
    let bytes = std::fs::read(&built.archive).unwrap();
    assert_eq!(sha256_hex(&bytes), built.sha256);
    assert_eq!(bytes.len() as u64, built.size);
    // Nothing but the archive is left.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let mut names = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        let mut content = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut content).unwrap();
        if name != PACKAGE_FILE {
            let item = built.package.items.iter().find(|i| i.file == name).unwrap();
            assert_eq!(sha256_hex(&content), item.sha256);
            assert_eq!(content.len() as u64, item.size);
            assert_eq!(&content[..4], b"RIFF");
        }
        names.push(name);
    }
    assert_eq!(
        names,
        [PACKAGE_FILE, "tone-a3/tone-a3.wav", "tone-a4/tone-a4.wav"]
    );
    let tags = &built.package.items[0].tags;
    assert_eq!(tags, &["Genre|Test", "Character|Pure", "Source|Generated"]);
}

// R3: the same manifest and sources give the same package byte for byte.
#[test]
fn the_same_manifest_builds_the_same_bytes() {
    let manifest = Manifest::load(&fixtures()).unwrap();
    let a = build(&manifest, &fixtures(), "core", &out("first"), &no_fetch).unwrap();
    let b = build(&manifest, &fixtures(), "core", &out("second"), &no_fetch).unwrap();
    assert_eq!(
        std::fs::read(a.archive).unwrap(),
        std::fs::read(b.archive).unwrap()
    );
}

// R2: an item without an allowed licence, a checksum or tags refuses the
// whole package, naming the item.
#[test]
fn an_item_without_a_licence_a_checksum_or_tags_refuses_the_package() {
    let cases: [(&str, Change); 5] = [
        ("licence", |t| t.replace("CC0-1.0", "CC-BY-NC-4.0")),
        ("licence", |t| t.replace("licence = \"CC0-1.0\"\n", "")),
        ("checksum", |t| {
            t.replacen(
                "sha256 = \"cf52830fa2eea129278ce557793c3008a8c2474de0dc8e18e50cc198dec3e4a3\"\n",
                "",
                1,
            )
        }),
        ("tag", |t| {
            t.replacen("tags = [\"Genre|Test\", \"Character|Pure\"]\n", "", 1)
        }),
        ("tag", |t| {
            t.replacen("[\"Genre|Test\", \"Character|Pure\"]", "[\"Test\"]", 1)
        }),
    ];
    for (what, change) in cases {
        let message = refused(&changed(change));
        assert!(message.contains("tone-a"), "{what}: {message}");
        assert!(message.contains(what), "{what}: {message}");
    }
}

// R2: a script that has changed since its checksum was set isn't run.
#[test]
fn a_script_that_changed_is_refused() {
    let manifest = changed(|t| t.replace("cf52830f", "cf52830e"));
    assert!(refused(&manifest).contains("sine.py has changed"));
}

// R1, R2: CC BY needs its attribution.
#[test]
fn cc_by_needs_its_attribution() {
    let manifest = changed(|t| t.replacen("CC0-1.0", "CC-BY-4.0", 1));
    assert!(refused(&manifest).contains("CC BY 4.0 needs an attribution changes"));
}

// R11, R12: a credit gives title, creator, licence with its link and changes.
#[test]
fn credits_name_the_creator_the_licence_and_the_changes() {
    let manifest = Manifest::load(&fixtures()).unwrap();
    let built = build(&manifest, &fixtures(), "core", &out("credits"), &no_fetch).unwrap();
    let text = credits_text([("Test Library", built.package.items.as_slice())]);
    assert!(text.starts_with("Test Library\n"));
    assert!(text.contains("\u{201c}Tone A3\u{201d} by Beatlayer, CC0 1.0 (https://creativecommons.org/publicdomain/zero/1.0/)"));
    let package: Package =
        serde_json::from_str(&serde_json::to_string(&built.package).unwrap()).unwrap();
    assert_eq!(package, built.package);
}

// R14: a path names the package, its version and the item.
#[test]
fn an_installed_files_path_names_its_item() {
    let path = Path::new("/x/Library/library-core-2/tone-a3/tone-a3.wav");
    let found = bl_library::reference_of(path).unwrap();
    assert_eq!(
        (
            found.package.as_str(),
            found.version.as_str(),
            found.item.as_str()
        ),
        ("library-core", "2", "tone-a3")
    );
    assert!(bl_library::reference_of(Path::new("/x/Samples/kick/kick.wav")).is_none());
    assert!(bl_library::reference_of(Path::new("kick.wav")).is_none());
}
