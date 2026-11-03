//! The extension host (extensions R1 to R16), headless, with test
//! components written as WebAssembly text.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_ext::{Error, Host, Options, Val};
use zip::write::SimpleFileOptions;

/// A folder for one test, removed when it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-ext-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn host(&self) -> Host {
        Host::new(
            self.path("Extensions"),
            self.path("Cache"),
            Options::default(),
        )
        .unwrap()
    }

    /// Writes a package of `files` and gives its path.
    fn package(&self, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let path = self.path(name);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (file, bytes) in files {
            zip.start_file(*file, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A component of `kind` at world version `world`, whose interface has
/// `add`, `spin` (never returns), `trap` and `grow` (asks for 64 MiB more).
/// `imports` is more component text, before the module.
fn component(kind: &str, world: &str, imports: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component
  {imports}
  (core module $m
    (memory (export "memory") 1)
    (func (export "add") (param i32 i32) (result i32) (i32.add (local.get 0) (local.get 1)))
    (func (export "spin") (loop $l (br $l)))
    (func (export "trap") unreachable)
    (func (export "oob") (drop (i32.load (i32.const 0x7ffffff0))))
    (func (export "grow") (drop (memory.grow (i32.const 1024)))))
  (core instance $i (instantiate $m))
  (func $add (param "a" u32) (param "b" u32) (result u32) (canon lift (core func $i "add")))
  (func $spin (canon lift (core func $i "spin")))
  (func $trap (canon lift (core func $i "trap")))
  (func $oob (canon lift (core func $i "oob")))
  (func $grow (canon lift (core func $i "grow")))
  (instance $api
    (export "add" (func $add)) (export "spin" (func $spin)) (export "trap" (func $trap))
    (export "oob" (func $oob)) (export "grow" (func $grow)))
  (export "beatlayer:ext/{kind}@{world}" (instance $api)))"#
    ))
    .unwrap()
}

/// A manifest for `id` at `version` with one `midi-tool` component, `extra`
/// being more of its lines.
fn manifest(id: &str, version: &str, extra: &str) -> String {
    format!(
        r#"id = "{id}"
name = "Test Tools"
version = "{version}"
author = "A. Person"
licence = "MIT"
description = "Tools for tests."

[[components]]
file = "tools.wasm"
kind = "midi-tool"
world = "0.1.0"
{extra}
"#
    )
}

/// The usual package: `org.example.tools` at `version`.
fn tools(scratch: &Scratch, name: &str, id: &str, version: &str) -> PathBuf {
    let manifest = manifest(id, version, "");
    scratch.package(
        name,
        &[
            ("extension.toml", manifest.as_bytes()),
            ("tools.wasm", &component("midi-tool", "0.1.0", "")),
        ],
    )
}

fn add(instance: &mut bl_ext::Instance) -> Result<Vec<Val>, Error> {
    instance.call("add", &[Val::U32(2), Val::U32(3)], Duration::from_secs(1))
}

/// Installing `package` is refused, and the reason says `says`; nothing was
/// kept in the extensions folder or the cache.
#[track_caller]
fn refused(scratch: &Scratch, package: &Path, says: &str) {
    let host = scratch.host();
    match host.install(package) {
        Err(Error::Refused(reason)) => {
            assert!(
                reason.contains(says),
                "refused with `{reason}`, not `{says}`"
            );
        }
        other => panic!(
            "expected a refusal naming `{says}`, got {:?}",
            other.map(|e| e.manifest.id)
        ),
    }
    for folder in ["Extensions", "Cache"] {
        let kept = std::fs::read_dir(scratch.path(folder)).map_or(0, Iterator::count);
        assert_eq!(kept, 0, "a refused package left something in {folder}");
    }
}

#[test]
fn a_package_installs_lists_and_removes_each_read_back() {
    let s = Scratch::new("lifecycle");
    let host = s.host();
    assert!(host.installed().is_empty());

    let installed = host
        .install(&tools(&s, "a.blext", "org.example.tools", "1.0.0"))
        .unwrap();
    assert_eq!(installed.manifest.id, "org.example.tools");
    assert_eq!(installed.folder, s.path("Extensions/org.example.tools"));
    assert!(
        installed.folder.join("tools.wasm").is_file(),
        "the package is unpacked"
    );
    let listed = host.installed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].manifest.name, "Test Tools");
    assert_eq!(listed[0].manifest.version.to_string(), "1.0.0");
    assert_eq!(listed[0].manifest.components[0].kind.name(), "midi-tool");

    let mut instance = host.instantiate("org.example.tools", "tools.wasm").unwrap();
    assert_eq!(add(&mut instance).unwrap(), vec![Val::U32(5)]);
    drop(instance);

    host.remove("org.example.tools").unwrap();
    assert!(host.installed().is_empty());
    assert!(!installed.folder.exists());
    assert_eq!(
        std::fs::read_dir(s.path("Cache")).unwrap().count(),
        0,
        "its compiled code goes too"
    );
    assert!(matches!(
        host.remove("org.example.tools"),
        Err(Error::Refused(_))
    ));
}

#[test]
fn a_later_version_replaces_an_earlier_and_an_equal_or_earlier_one_is_refused() {
    let s = Scratch::new("versions");
    let host = s.host();
    host.install(&tools(&s, "1.blext", "org.example.tools", "1.0.0"))
        .unwrap();
    let manifest = manifest("org.example.tools", "1.1.0", "");
    let changed = s.package(
        "2.blext",
        &[
            ("extension.toml", manifest.as_bytes()),
            (
                "tools.wasm",
                &component("midi-tool", "0.1.0", "(core module $other)"),
            ),
        ],
    );
    host.install(&changed).unwrap();
    let listed = host.installed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].manifest.version.to_string(), "1.1.0");
    let leftovers = std::fs::read_dir(s.path("Extensions")).unwrap().count();
    assert_eq!(leftovers, 1, "nothing but the extension's folder");
    assert_eq!(
        std::fs::read_dir(s.path("Cache")).unwrap().count(),
        1,
        "the replaced component's compiled code went"
    );

    for (name, version) in [("same.blext", "1.1.0"), ("older.blext", "1.0.5")] {
        let package = tools(&s, name, "org.example.tools", version);
        match host.install(&package) {
            Err(Error::Refused(reason)) => {
                assert!(reason.contains("already installed"), "{reason}")
            }
            other => panic!("{version} should be refused, got {:?}", other.is_ok()),
        }
    }
    assert_eq!(host.installed()[0].manifest.version.to_string(), "1.1.0");
}

#[test]
fn a_package_that_fails_a_check_is_refused_whole_with_its_reason() {
    let s = Scratch::new("refusals");
    let good = component("midi-tool", "0.1.0", "");
    let toml = |extra: &str| manifest("org.example.tools", "1.0.0", extra);
    let case = |name: &str, files: &[(&str, &[u8])], says: &str| {
        let package = s.package(name, files);
        refused(&s, &package, says);
    };

    case(
        "zip.blext",
        &[("not a package", b"text")],
        "no extension.toml",
    );
    let text = std::fs::read(s.path("zip.blext")).unwrap();
    std::fs::write(s.path("junk.blext"), &text[..text.len() / 2]).unwrap();
    refused(&s, &s.path("junk.blext"), "isn't a ZIP file");

    let m = toml("");
    case(
        "syntax.blext",
        &[("extension.toml", b"id = ")],
        "doesn't parse",
    );
    let missing = m.replace("author = \"A. Person\"\n", "");
    case(
        "field.blext",
        &[("extension.toml", missing.as_bytes())],
        "author",
    );
    let empty = m.replace("A. Person", " ");
    case(
        "empty.blext",
        &[("extension.toml", empty.as_bytes())],
        "empty author",
    );
    let id = m.replace("org.example.tools", "Not An ID");
    case(
        "id.blext",
        &[("extension.toml", id.as_bytes()), ("tools.wasm", &good)],
        "isn't an extension ID",
    );
    let nothing = "id = \"org.example.tools\"\nname = \"n\"\nversion = \"1.0.0\"\nauthor = \"a\"\nlicence = \"l\"\ndescription = \"d\"\ncomponents = []\n";
    case(
        "none.blext",
        &[("extension.toml", nothing.as_bytes())],
        "names no components",
    );

    for world in ["0.2.0", "1.0.0", "0.1.1"] {
        let m = m.replace("world = \"0.1.0\"", &format!("world = \"{world}\""));
        case(
            "world.blext",
            &[("extension.toml", m.as_bytes()), ("tools.wasm", &good)],
            &format!("world version {world}"),
        );
    }
    let asks = toml("imports = [\"files\"]");
    case(
        "import.blext",
        &[("extension.toml", asks.as_bytes()), ("tools.wasm", &good)],
        "import `files`",
    );
    let big = toml("memory-mib = 65");
    case(
        "memory.blext",
        &[("extension.toml", big.as_bytes()), ("tools.wasm", &good)],
        "65 MiB",
    );

    case(
        "lacks.blext",
        &[("extension.toml", m.as_bytes())],
        "which the package lacks",
    );
    case(
        "garbage.blext",
        &[
            ("extension.toml", m.as_bytes()),
            ("tools.wasm", b"not wasm"),
        ],
        "isn't a valid component",
    );
    let other = component("audio-effect", "0.1.0", "");
    case(
        "kind.blext",
        &[("extension.toml", m.as_bytes()), ("tools.wasm", &other)],
        "doesn't export `beatlayer:ext/midi-tool@0.1.0`",
    );

    // R8: a component reaches nothing beyond its world's imports.
    let clock = r#"(import "wasi:clocks/monotonic-clock@0.2.0" (instance (export "now" (func (result u64)))))"#;
    let reaching = component("midi-tool", "0.1.0", clock);
    case(
        "clock.blext",
        &[("extension.toml", m.as_bytes()), ("tools.wasm", &reaching)],
        "imports `wasi:clocks/monotonic-clock@0.2.0`",
    );
    let set = r#"(import "beatlayer:ext/set@0.1.0" (instance (export "now" (func (result u64)))))"#;
    let asking = component("midi-tool", "0.1.0", set);
    let with_set = toml("imports = [\"set\"]");
    case(
        "set.blext",
        &[
            ("extension.toml", with_set.as_bytes()),
            ("tools.wasm", &asking),
        ],
        "imports `beatlayer:ext/set@0.1.0`",
    );

    for path in [
        "../evil.wasm",
        "/abs.wasm",
        "a/../b.wasm",
        "dir\\file.wasm",
        "con.wasm",
        "x//y.wasm",
    ] {
        case(
            "path.blext",
            &[
                ("extension.toml", m.as_bytes()),
                ("tools.wasm", &good),
                (path, b"x"),
            ],
            "the package's",
        );
    }
    let outside = toml("").replace("tools.wasm", "../tools.wasm");
    case(
        "named.blext",
        &[
            ("extension.toml", outside.as_bytes()),
            ("../tools.wasm", &good),
        ],
        "climbs out of the package",
    );
    case(
        "case.blext",
        &[
            ("extension.toml", m.as_bytes()),
            ("tools.wasm", &good),
            ("Tools.wasm", &good),
        ],
        "twice",
    );
}

#[test]
fn a_component_that_traps_is_disabled_alone_and_the_rest_go_on() {
    let s = Scratch::new("traps");
    let host = s.host();
    host.install(&tools(&s, "a.blext", "org.example.broken", "1.0.0"))
        .unwrap();
    host.install(&tools(&s, "b.blext", "org.example.sound", "1.0.0"))
        .unwrap();
    let mut broken = host
        .instantiate("org.example.broken", "tools.wasm")
        .unwrap();
    let mut sound = host.instantiate("org.example.sound", "tools.wasm").unwrap();

    let Err(Error::Failed(reason)) = broken.call("trap", &[], Duration::from_secs(1)) else {
        panic!("the call should fail");
    };
    assert!(reason.contains("trapped"), "{reason}");
    assert!(broken.is_off());
    let failure = host.failure("org.example.broken").unwrap();
    assert!(failure.starts_with("tools.wasm: trapped"), "{failure}");

    // The failed instance stays off, its extension can't make another, and
    // the other extension goes on.
    assert!(matches!(add(&mut broken), Err(Error::Failed(_))));
    assert!(matches!(
        host.instantiate("org.example.broken", "tools.wasm"),
        Err(Error::Refused(_))
    ));
    assert_eq!(add(&mut sound).unwrap(), vec![Val::U32(5)]);
    assert_eq!(host.failure("org.example.sound"), None);

    // R12: switched on again, it runs.
    host.clear_failure("org.example.broken");
    assert_eq!(host.failure("org.example.broken"), None);
    let mut again = host
        .instantiate("org.example.broken", "tools.wasm")
        .unwrap();
    assert_eq!(add(&mut again).unwrap(), vec![Val::U32(5)]);

    // An out-of-bounds access traps the same way.
    let mut oob = host.instantiate("org.example.sound", "tools.wasm").unwrap();
    assert!(matches!(
        oob.call("oob", &[], Duration::from_secs(1)),
        Err(Error::Failed(_))
    ));
    assert!(host.failure("org.example.sound").is_some());
}

#[test]
fn a_component_that_overruns_its_limit_is_stopped_with_the_reason() {
    let s = Scratch::new("overrun");
    let host = s.host();
    host.install(&tools(&s, "a.blext", "org.example.tools", "1.0.0"))
        .unwrap();
    let mut instance = host.instantiate("org.example.tools", "tools.wasm").unwrap();
    let Err(Error::Failed(reason)) = instance.call("spin", &[], Duration::from_millis(5)) else {
        panic!("a call that never returns should be stopped");
    };
    assert_eq!(reason, "ran past its time limit of 5 ms");
    assert!(instance.is_off());
    assert_eq!(
        host.failure("org.example.tools").unwrap(),
        "tools.wasm: ran past its time limit of 5 ms"
    );
}

#[test]
fn memory_is_limited_to_what_the_manifest_asks_for() {
    let s = Scratch::new("memory");
    let host = s.host();
    let m = manifest("org.example.tools", "1.0.0", "memory-mib = 1");
    let package = s.package(
        "a.blext",
        &[
            ("extension.toml", m.as_bytes()),
            ("tools.wasm", &component("midi-tool", "0.1.0", "")),
        ],
    );
    host.install(&package).unwrap();
    let mut instance = host.instantiate("org.example.tools", "tools.wasm").unwrap();
    // 64 MiB more than the 1 MiB it asked for: memory exhausted is a trap.
    assert!(matches!(
        instance.call("grow", &[], Duration::from_secs(1)),
        Err(Error::Failed(_))
    ));
    assert!(host.failure("org.example.tools").is_some());
}

#[test]
fn compiled_code_is_reused_after_a_restart_and_rebuilt_when_the_component_changes() {
    let s = Scratch::new("cache");
    let first = s.host();
    first
        .install(&tools(&s, "a.blext", "org.example.tools", "1.0.0"))
        .unwrap();
    let cached = |s: &Scratch| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(s.path("Cache"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    let before = cached(&s);
    assert_eq!(before.len(), 1, "installing compiles ahead of time (R5)");
    assert!(before[0].ends_with("-wasmtime-50.0.1.cwasm"), "{before:?}");
    first
        .instantiate("org.example.tools", "tools.wasm")
        .unwrap();
    assert_eq!(first.compiled(), 0);
    drop(first);

    // A restart: a new host over the same folders loads the cached code.
    let restarted = s.host();
    let mut instance = restarted
        .instantiate("org.example.tools", "tools.wasm")
        .unwrap();
    assert_eq!(add(&mut instance).unwrap(), vec![Val::U32(5)]);
    assert_eq!(restarted.compiled(), 0, "nothing was compiled again");

    // Code that won't load is compiled again, and kept. The instance maps
    // the file, so it goes first: Windows refuses to write a mapped file,
    // and Linux faults on a truncated one.
    drop(instance);
    std::fs::write(s.path("Cache").join(&before[0]), b"damaged").unwrap();
    restarted
        .instantiate("org.example.tools", "tools.wasm")
        .unwrap();
    assert_eq!(restarted.compiled(), 1);
    restarted
        .instantiate("org.example.tools", "tools.wasm")
        .unwrap();
    assert_eq!(restarted.compiled(), 1, "the new code is kept");

    // A changed component has another hash, so another file.
    let m = manifest("org.example.tools", "1.0.1", "");
    let changed = s.package(
        "b.blext",
        &[
            ("extension.toml", m.as_bytes()),
            (
                "tools.wasm",
                &component("midi-tool", "0.1.0", "(core module $other)"),
            ),
        ],
    );
    restarted.install(&changed).unwrap();
    let after = cached(&s);
    assert_eq!(after.len(), 1);
    assert_ne!(after, before);
    restarted
        .instantiate("org.example.tools", "tools.wasm")
        .unwrap();
    assert_eq!(restarted.compiled(), 1);
}

#[test]
fn the_cache_is_keyed_by_the_wasmtime_the_crate_pins() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest.contains("wasmtime = { version = \"=50.0.1\""),
        "update WASMTIME in host.rs with the pinned wasmtime"
    );
}

#[test]
fn a_full_pool_refuses_an_instance_without_failing_the_extension() {
    let s = Scratch::new("pool");
    let host = Host::new(
        s.path("Extensions"),
        s.path("Cache"),
        Options { instances: 1 },
    )
    .unwrap();
    host.install(&tools(&s, "a.blext", "org.example.tools", "1.0.0"))
        .unwrap();
    let first = host.instantiate("org.example.tools", "tools.wasm").unwrap();
    assert!(matches!(
        host.instantiate("org.example.tools", "tools.wasm"),
        Err(Error::Refused(_))
    ));
    assert_eq!(host.failure("org.example.tools"), None);
    drop(first);
    host.instantiate("org.example.tools", "tools.wasm").unwrap();
}
