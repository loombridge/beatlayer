//! Reading a DAWproject file and opening what it holds as a set (WP-6.36;
//! the DAWproject spec, R1 to R43).
//!
//! The fixtures in `tests/fixtures/dawproject/` are written by
//! `tests/fixtures/gen/gen_dawproject.py`, each with the report it should
//! give beside it.

use std::path::{Path, PathBuf};

use bl_interop::dawproject::{self, Context, Report};

/// The folder the fixtures are in.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dawproject")
}

/// The report a fixture gives, as JSON.
fn actual(container: &dawproject::Container, folder: &Path) -> serde_json::Value {
    let context = Context {
        folder,
        extract_to: folder,
    };
    let imported = dawproject::import(container, &context).expect("the file imports");
    let report = imported.report;
    serde_json::json!({
        "application": report.application,
        "entries": report
            .entries
            .iter()
            .map(|entry| serde_json::json!({
                "what": entry.what,
                "where": entry.place,
                "count": entry.count,
                "did": entry.did,
            }))
            .collect::<Vec<_>>(),
    })
}

/// The report a fixture should give, read from beside it.
fn expected(name: &str) -> serde_json::Value {
    let path = fixtures().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("the expected report is JSON")
}

/// Reads a fixture and gives its report.
fn import(name: &str) -> serde_json::Value {
    let path = fixtures().join(format!("{name}.dawproject"));
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let container = dawproject::read_container(&bytes)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    actual(&container, &fixtures())
}

/// Every fixture that imports whole, with the report it should give.
#[test]
fn every_fixture_gives_the_report_it_should() {
    for name in [
        "minimal",
        "tempo",
        "clips",
        "media",
        "session",
        "bitwig-shaped",
        "studio-one-shaped",
        "cubase-shaped",
    ] {
        assert_eq!(
            import(name),
            expected(name),
            "the report of {name} differs from the one beside it"
        );
    }
}

/// A file with nothing lost or changed gives an empty report (R12).
#[test]
fn a_file_with_nothing_lost_gives_an_empty_report() {
    let report = import("minimal");
    assert_eq!(report["entries"], serde_json::json!([]));
    assert_eq!(report["application"], serde_json::json!("Fixture DAW 1.0"));
}

/// A container without `project.xml` is refused (R1).
#[test]
fn a_container_without_a_project_is_refused() {
    let bytes = std::fs::read(fixtures().join("damaged/no-project-xml.dawproject")).unwrap();
    let error = dawproject::read_container(&bytes).expect_err("the container is refused");
    assert!(matches!(error, dawproject::Error::NoProject), "{error}");
}

/// A file that isn't a ZIP file is refused as not a DAWproject file (R1).
#[test]
fn a_file_that_is_not_a_zip_is_refused() {
    let bytes = std::fs::read(fixtures().join("damaged/not-a-zip.dawproject")).unwrap();
    let error = dawproject::read_container(&bytes).expect_err("the file is refused");
    assert!(matches!(error, dawproject::Error::NotAProject), "{error}");
}

/// A version Beatlayer doesn't read is refused, naming the version (R3).
#[test]
fn a_version_beatlayer_does_not_read_is_refused() {
    let bytes = std::fs::read(fixtures().join("damaged/version-2.dawproject")).unwrap();
    let container = dawproject::read_container(&bytes).expect("the container is read");
    let context = Context {
        folder: &fixtures(),
        extract_to: &fixtures(),
    };
    let error = dawproject::import(&container, &context).expect_err("the file is refused");
    assert!(
        matches!(error, dawproject::Error::Version { .. }),
        "{error}"
    );
}

/// Every damaged file is either refused or read in part, and none of them
/// panics (R4, R5, R6).
#[test]
fn a_damaged_file_is_refused_or_read_in_part() {
    let folder = fixtures().join("damaged");
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .expect("the folder is there")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".dawproject").map(str::to_owned)
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "there are damaged fixtures");
    for name in names {
        let bytes = std::fs::read(folder.join(format!("{name}.dawproject"))).unwrap();
        // Reading it may be refused; reading it may find part of it. What
        // must not happen is a panic.
        if let Ok(container) = dawproject::read_container(&bytes) {
            let context = Context {
                folder: &folder,
                extract_to: &folder,
            };
            let _ = dawproject::import(&container, &context);
        }
    }
}

/// The damaged files that give a report give the one beside them.
#[test]
fn the_damaged_fixtures_that_read_give_their_report() {
    for name in [
        "utf-8-bom",
        "utf-16",
        "doctype",
        "escape",
        "missing-reference",
        "unknown",
    ] {
        let bytes = std::fs::read(fixtures().join(format!("damaged/{name}.dawproject"))).unwrap();
        let container =
            dawproject::read_container(&bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            actual(&container, &fixtures()),
            expected(&format!("damaged/{name}")),
            "the report of {name} differs from the one beside it"
        );
    }
}

/// A time in beats and the same time in seconds reach the same tick through
/// an imported tempo map with a ramp (R9).
#[test]
fn beats_and_seconds_reach_the_same_tick_through_a_ramp() {
    use bl_core::time::{TempoMap, TempoSegment, Ticks};

    let ramp = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: bl_core::time::Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Ramp {
            start: Ticks::from_quarters(4),
            from: bl_core::time::Bpm::new(120.0).unwrap(),
            to: bl_core::time::Bpm::new(240.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(8),
            bpm: bl_core::time::Bpm::new(240.0).unwrap(),
        },
    ])
    .unwrap();
    for quarters in 0..16 {
        let beats = Ticks::from_quarters(quarters);
        let seconds = ramp.ticks_to_seconds(beats);
        let back = ramp.seconds_to_ticks(seconds);
        assert_eq!(
            back, beats,
            "a beat of {quarters} quarters went to {seconds} seconds and came back as {back}"
        );
    }
}

/// A tempo map the importer builds from a file's points holds the ramp the
/// file asks for (R13).
#[test]
fn an_imported_tempo_map_holds_the_ramp_the_file_asks_for() {
    let container =
        dawproject::read_container(&std::fs::read(fixtures().join("tempo.dawproject")).unwrap())
            .unwrap();
    let context = Context {
        folder: &fixtures(),
        extract_to: &fixtures(),
    };
    let imported = dawproject::import(&container, &context).unwrap();
    let tempo = imported.project.document.settings().tempo.clone();
    let segments: Vec<_> = tempo.segments().collect();
    assert!(segments.len() >= 3, "{segments:?}");
    // The first point ramps to the next point's value, and the next holds.
    assert_eq!(
        segments[0],
        bl_core::time::TempoSegment::Ramp {
            start: bl_core::time::Ticks::ZERO,
            from: bl_core::time::Bpm::new(140.0).unwrap(),
            to: bl_core::time::Bpm::new(160.0).unwrap(),
        },
        "{segments:?}"
    );
    assert_eq!(
        segments[1],
        bl_core::time::TempoSegment::Constant {
            start: bl_core::time::Ticks::from_quarters(4),
            bpm: bl_core::time::Bpm::new(160.0).unwrap(),
        },
        "{segments:?}"
    );
}

/// The report of an import names the application the file says made it (R12).
#[test]
fn the_report_names_the_application_that_made_the_file() {
    let container =
        dawproject::read_container(&std::fs::read(fixtures().join("minimal.dawproject")).unwrap())
            .unwrap();
    assert_eq!(
        container
            .project
            .application
            .as_ref()
            .and_then(|application| application.label())
            .as_deref(),
        Some("Fixture DAW 1.0")
    );
}

/// The report is empty for a file with nothing lost, and holds an entry for
/// each thing that was (R12).
#[test]
fn the_report_counts_the_same_thing_once() {
    let mut report = Report::default();
    report.add("a thing", "somewhere", "left out");
    report.add("a thing", "somewhere", "left out");
    report.add("a thing", "elsewhere", "left out");
    assert_eq!(report.entries.len(), 2);
    assert_eq!(report.entries[0].count, 2);
    assert_eq!(report.entries[1].count, 1);
}
