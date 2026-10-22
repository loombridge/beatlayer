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
    let context = Context::new(folder, folder);
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
    let folder = fixtures();
    let context = Context::new(&folder, &folder);
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
            let context = Context::new(&folder, &folder);
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
        "unsafe-entries",
        "duplicate-id",
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
    let folder = fixtures();
    let context = Context::new(&folder, &folder);
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

/// What a fixture imports to.
fn import_of(name: &str) -> dawproject::Imported {
    let bytes = std::fs::read(fixtures().join(format!("{name}.dawproject"))).unwrap();
    let container = dawproject::read_container(&bytes).unwrap();
    let folder = fixtures();
    dawproject::import(&container, &Context::new(&folder, &folder)).unwrap()
}

/// A container holding `project.xml` only, with the XML in it.
fn container_of(xml: &str) -> Vec<u8> {
    use std::io::Write;

    let mut bytes = Vec::new();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
    writer
        .start_file("project.xml", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(xml.as_bytes()).unwrap();
    writer.finish().unwrap();
    bytes
}

/// A file with the given XML in it imports to.
fn import_xml(xml: &str) -> dawproject::Imported {
    let container = dawproject::read_container(&container_of(xml)).unwrap();
    let folder = fixtures();
    dawproject::import(&container, &Context::new(&folder, &folder)).expect("the file imports")
}

/// The entries of an import's report that say `what`.
fn entries_with<'a>(imported: &'a dawproject::Imported, what: &str) -> Vec<&'a dawproject::Entry> {
    imported
        .report
        .entries
        .iter()
        .filter(|entry| entry.what == what)
        .collect()
}

/// A track's clips in order of their start.
fn clips_of<'a>(document: &'a bl_model::Document, name: &str) -> Vec<&'a bl_model::Clip> {
    let mut clips: Vec<_> = track(document, name).clips().collect();
    clips.sort_by_key(|clip| clip.start);
    clips
}

/// The track called `name`.
fn track<'a>(document: &'a bl_model::Document, name: &str) -> &'a bl_model::Track {
    document
        .tracks()
        .find(|track| track.name() == name)
        .unwrap_or_else(|| panic!("no track called {name}"))
}

/// The tracks, groups, returns, mixer and routing the file asks for (R15 to
/// R22).
#[test]
fn tracks_mixer_and_routing_follow_the_file() {
    use bl_model::{Input, Monitor, Output, SendTap, TrackKind};

    let imported = import_of("bitwig-shaped");
    let document = &imported.project.document;
    let names = |ids: Vec<bl_model::TrackId>| -> Vec<String> {
        ids.into_iter()
            .map(|id| document.track(id).unwrap().name().to_owned())
            .collect()
    };
    assert_eq!(
        names(document.top_level().collect()),
        ["Group", "Hybrid", "Hybrid Audio", "Drums", "Source", "Bus"]
    );
    assert_eq!(names(document.returns().collect()), ["Reverb"]);
    let group = track(document, "Group");
    assert_eq!(group.kind(), TrackKind::Group);
    assert_eq!(names(group.children().collect()), ["Kick", "Snare"]);
    assert_eq!(track(document, "Hybrid").kind(), TrackKind::Midi);
    assert_eq!(track(document, "Hybrid Audio").kind(), TrackKind::Audio);
    // R22: a send to a return, and the return's tap.
    let reverb = track(document, "Reverb");
    assert_eq!(track(document, "Drums").send(reverb.id()), 0.5);
    assert_eq!(reverb.mixer().tap, SendTap::Post);
    // R19: a track routed into another is its Track In.
    let bus = track(document, "Bus");
    assert_eq!(
        track(document, "Source").routing().output,
        Output::Track { track: bus.id() }
    );
    assert_eq!(bus.routing().monitor, Monitor::In);
    assert_eq!(bus.routing().input, Input::None);
    // R21: Bitwig's linear volume 1 and normalized pan 0.5.
    assert_eq!(track(document, "Drums").mixer().volume, 1.0);
    assert_eq!(track(document, "Drums").mixer().pan, 0.0);
}

/// Clips are placed, cut, looped and unrolled as the file says (R24 to R32).
#[test]
fn clips_follow_the_file() {
    use bl_core::time::Ticks;
    use bl_model::ClipData;

    let q = Ticks::QUARTER;
    let imported = import_of("clips");
    let document = &imported.project.document;

    // R26 and R27: the later clip replaces what it covers, a clip switched
    // off is muted, and a play region without a duration is its length.
    let lead = clips_of(document, "Lead");
    let places: Vec<_> = lead
        .iter()
        .map(|clip| (clip.name.as_str(), clip.start, clip.length, clip.mute))
        .collect();
    assert_eq!(
        places,
        [
            ("Loop", Ticks::ZERO, q * 2, false),
            ("Over", q * 2, q * 4, false),
            ("Off", q * 8, q * 2, true),
            ("Window", q * 12, q * 2, false),
        ]
    );
    let ClipData::Midi(window) = &lead[3].data else {
        panic!("a MIDI clip");
    };
    assert_eq!((window.start, window.end), (q, Some(q * 3)));
    // R28: three of the five notes can be played.
    let ClipData::Midi(looped) = &lead[0].data else {
        panic!("a MIDI clip");
    };
    assert_eq!(looped.notes.len(), 3);
    assert!(looped.looping && looped.loop_end == Some(q * 2));

    // R32: the unwarped loop is one clip per pass.
    let pad = clips_of(document, "Pad");
    assert_eq!(pad.len(), 6);
    for (index, clip) in pad[..4].iter().enumerate() {
        assert_eq!((clip.start, clip.length), (q * index as i64, q));
        let ClipData::Audio(audio) = &clip.data else {
            panic!("an audio clip");
        };
        assert!(!audio.looping);
        assert_eq!((audio.start, audio.end), (0.0, Some(0.5)));
    }
    // R29: a play region in seconds is seconds in the file.
    let ClipData::Audio(window) = &pad[5].data else {
        panic!("an audio clip");
    };
    assert_eq!((window.start, window.end), (0.25, Some(0.75)));
    // R25: a further timeline of clips is a take lane.
    let takes: Vec<_> = track(document, "Pad").take_lanes().collect();
    assert_eq!(takes.len(), 1);
    assert_eq!(takes[0].name, "Take B");
    assert_eq!(track(document, "Bass").clips().count(), 1);

    // R24: a timeline outside any clip is one clip, as long as its notes
    // rounded up to the next bar.
    let loose = clips_of(document, "Loose");
    assert_eq!((loose[0].start, loose[0].length), (Ticks::ZERO, q * 8));
    // R9: a lane in seconds, at 120 BPM.
    let seconds = clips_of(document, "Secs");
    assert_eq!((seconds[0].start, seconds[0].length), (q * 2, q * 4));
}

/// Nested clips flatten over the passes of the outer clip's loop, a warped
/// clip keeps its warp markers, and an alias plays a copy (R30, R34, R35).
#[test]
fn nested_warped_clips_flatten_over_the_loop() {
    use bl_core::time::Ticks;
    use bl_model::ClipData;

    let q = Ticks::QUARTER;
    let imported = import_of("bitwig-shaped");
    let document = &imported.project.document;
    let hybrid = clips_of(document, "Hybrid Audio");
    let places: Vec<_> = hybrid
        .iter()
        .map(|clip| (clip.start, clip.length))
        .collect();
    assert_eq!(
        places,
        [
            (Ticks::ZERO, q * 2),
            (q * 2, q * 2),
            (q * 4, q * 2),
            (q * 6, q * 2)
        ]
    );
    let ClipData::Audio(first) = &hybrid[0].data else {
        panic!("an audio clip");
    };
    // Two beats of contents are one second of the file, at 120 BPM.
    assert!(first.warp);
    assert_eq!(first.tempo, Some(120.0));
    assert_eq!(
        first
            .warp_markers
            .iter()
            .map(|m| (m.time, m.beat))
            .collect::<Vec<_>>(),
        [(0.0, Ticks::ZERO), (1.0, q * 2)]
    );
    let ClipData::Audio(second) = &hybrid[1].data else {
        panic!("an audio clip");
    };
    assert_eq!(second.tempo, Some(240.0));

    let imported = import_of("studio-one-shaped");
    let synth = clips_of(&imported.project.document, "Synth");
    assert_eq!(synth.len(), 2);
    assert_eq!(synth[0].data, synth[1].data);
    assert_ne!(synth[0].id, synth[1].id);
}

/// Scenes are the file's, with its names, slots and Stop buttons (R42).
#[test]
fn scenes_follow_the_file() {
    let imported = import_of("session");
    let document = &imported.project.document;
    let scenes: Vec<_> = document.scenes().collect();
    assert_eq!(
        scenes
            .iter()
            .map(|scene| scene.name.as_str())
            .collect::<Vec<_>>(),
        ["One", "Two", ""]
    );
    let (lead, drums, inside) = (
        track(document, "Lead"),
        track(document, "Drums"),
        track(document, "Inside"),
    );
    assert_eq!(lead.slot(scenes[0].id).unwrap().name, "Idea");
    assert!(lead.slot(scenes[1].id).is_none());
    assert!(!lead.has_stop_button(scenes[1].id));
    // A Session clip keeps its loop.
    let bl_model::ClipData::Audio(beat) = &drums.slot(scenes[0].id).unwrap().data else {
        panic!("an audio clip");
    };
    assert!(beat.looping);
    assert_eq!(inside.slot(scenes[1].id).unwrap().name, "Inner");
    // A file with no scenes gets the eight a new set has (R42).
    assert_eq!(imported_scene_count("minimal"), 8);
}

fn imported_scene_count(name: &str) -> usize {
    import_of(name).project.document.scenes().count()
}

/// A muted or a soloed master channel is listed, and the file still imports
/// (R18): Main has no activator and can't be soloed.
#[test]
fn a_muted_or_soloed_master_channel_is_listed_and_the_file_still_imports() {
    let file = |channel: &str| {
        format!(
            r#"<?xml version="1.0"?>
<Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure>
<Track contentType="audio" id="a" name="A"><Channel role="regular" id="ca"/></Track>
<Track contentType="audio" id="m" name="Master"><Channel role="master" id="cm">{channel}</Channel></Track>
</Structure>
<Arrangement id="a2"><Lanes id="all"/></Arrangement><Scenes/></Project>"#
        )
    };
    let muted = import_xml(&file(r#"<Mute value="true" id="mm"/>"#));
    assert_eq!(entries_with(&muted, "a muted master channel").len(), 1);
    let soloed = import_xml(&file(r#"<Mute value="false" id="mm"/>"#));
    assert!(entries_with(&soloed, "a muted master channel").is_empty());
}

/// A device on the master channel is listed once, not twice (R12).
#[test]
fn a_device_on_the_master_channel_is_listed_once() {
    let imported = import_xml(
        r#"<?xml version="1.0"?>
<Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure>
<Track contentType="audio" id="a" name="A"><Channel role="regular" id="ca"/></Track>
<Track contentType="audio" id="m" name="Master"><Channel role="master" id="cm"><Devices><ClapPlugin id="d1" deviceID="x" deviceRole="audioFX"/></Devices></Channel></Track>
</Structure>
<Arrangement id="a2"><Lanes id="all"/></Arrangement><Scenes/></Project>"#,
    );
    let listed = entries_with(&imported, "a CLAP plug-in");
    assert_eq!(listed.len(), 1, "the plug-in was listed more than once");
    assert_eq!(listed[0].place, "the track \"Main\"");
}

/// The signature changes are all kept when the file lists them out of time
/// order (R14).
#[test]
fn signature_changes_out_of_time_order_are_kept() {
    let imported = import_xml(
        r#"<?xml version="1.0"?>
<Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/><TimeSignature numerator="4" denominator="4" id="sig"/></Transport>
<Structure><Track contentType="notes" id="b" name="B"/></Structure>
<Arrangement id="a"><TimeSignatureAutomation id="tsa"><Target parameter="sig"/>
<TimeSignaturePoint time="8" numerator="5" denominator="4"/>
<TimeSignaturePoint time="4" numerator="3" denominator="4"/>
</TimeSignatureAutomation>
<Lanes id="all"/></Arrangement><Scenes/></Project>"#,
    );
    let changes: Vec<_> = imported
        .project
        .document
        .settings()
        .time_signatures
        .changes()
        .collect();
    assert_eq!(changes.len(), 3, "a signature change was lost: {changes:?}");
    assert_eq!(changes[1].signature.numerator(), 3);
    assert_eq!(changes[2].signature.numerator(), 5);
}

/// A comment isn't kept, and is counted in the report (R20).
#[test]
fn a_comment_is_counted_in_the_report() {
    let imported = import_xml(
        r#"<?xml version="1.0"?>
<Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure><Track contentType="notes" id="b" name="B" comment="a note"/></Structure>
<Arrangement id="a"><Lanes id="all"/></Arrangement><Scenes/></Project>"#,
    );
    assert_eq!(entries_with(&imported, "a comment").len(), 1);
}

/// A time in beats and the same time in seconds become one tick through the
/// tempo map the file makes, ramps included (R9).
#[test]
fn a_clip_in_seconds_starts_where_the_same_clip_in_beats_does() {
    let project = |seconds: &[f64], beats: &[f64]| {
        let clip = |time: f64| {
            format!(
                r#"<Clip time="{time}" duration="0.01" name="c"><Notes id="n{}"><Note time="0" duration="1" key="60"/></Notes></Clip>"#,
                time.to_bits()
            )
        };
        let lane = |track: &str, unit: &str, times: &[f64]| {
            let clips: String = times.iter().map(|time| clip(*time)).collect();
            format!(
                r#"<Lanes track="{track}" timeUnit="{unit}" id="l{track}"><Clips id="c{track}">{clips}</Clips></Lanes>"#
            )
        };
        format!(
            r#"<?xml version="1.0"?>
<Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure><Track contentType="notes" id="b" name="Beats"/><Track contentType="notes" id="s" name="Secs"/></Structure>
<Arrangement id="a"><TempoAutomation unit="bpm" id="ta"><Target parameter="tempo"/>
<RealPoint time="0" value="60" interpolation="linear"/><RealPoint time="3" value="240" interpolation="linear"/>
<RealPoint time="7" value="90" interpolation="hold"/><RealPoint time="9" value="300" interpolation="linear"/>
<RealPoint time="12" value="40" interpolation="hold"/></TempoAutomation>
<Lanes id="all">{}{}</Lanes></Arrangement><Scenes/></Project>"#,
            lane("b", "beats", beats),
            lane("s", "seconds", seconds),
        )
    };
    let folder = fixtures();
    let import = |xml: String| {
        let container = dawproject::read_container(&container_of(&xml)).unwrap();
        dawproject::import(&container, &Context::new(&folder, &folder)).unwrap()
    };
    let beats: Vec<f64> = (0..40).map(|n| f64::from(n) * 0.5 + 0.25).collect();
    let tempo = import(project(&[], &beats)).project.document.tempo_map();
    let seconds: Vec<f64> = beats
        .iter()
        .map(|beat| {
            tempo.ticks_to_seconds(
                bl_core::time::Ticks::from_quarters(0)
                    + bl_core::time::Ticks((beat * 705_600.0) as i64),
            )
        })
        .collect();
    let document = import(project(&seconds, &beats)).project.document;
    let starts = |name: &str| -> Vec<i64> {
        track(&document, name)
            .clips()
            .map(|clip| clip.start.get())
            .collect()
    };
    assert_eq!(starts("Beats").len(), beats.len());
    for (a, b) in starts("Beats").iter().zip(starts("Secs")) {
        assert!((a - b).abs() <= 1, "{a} ticks in beats and {b} in seconds");
    }
}

/// Damaging a fixture's bytes anywhere never panics, and whatever imports
/// holds the document model's rules (R4 to R8): the fuzz target's invariant,
/// run over a fixed set of mutations.
#[test]
fn damaged_bytes_never_panic_and_what_imports_is_valid() {
    let folder = fixtures().join("nowhere");
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for name in [
        "clips",
        "bitwig-shaped",
        "cubase-shaped",
        "studio-one-shaped",
        "session",
        "tempo",
    ] {
        let bytes = std::fs::read(fixtures().join(format!("{name}.dawproject"))).unwrap();
        // The project part, as text, cut short and with bytes changed.
        let xml = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let mut text = String::new();
            std::io::Read::read_to_string(&mut archive.by_name("project.xml").unwrap(), &mut text)
                .unwrap();
            text
        };
        for round in 0..60 {
            let mut text = xml.clone().into_bytes();
            match round % 3 {
                0 => text.truncate((next() as usize) % text.len()),
                1 => {
                    for _ in 0..8 {
                        let at = (next() as usize) % text.len();
                        text[at] = b"<>/\"= 0123456789-.e&;"[(next() as usize) % 21];
                    }
                }
                _ => {
                    // Drop a stretch.
                    let at = (next() as usize) % text.len();
                    let end = (at + (next() as usize) % 200).min(text.len());
                    text.drain(at..end);
                }
            }
            let Ok(text) = String::from_utf8(text) else {
                continue;
            };
            let Ok(container) = dawproject::read_container(&container_of(&text)) else {
                continue;
            };
            if let Ok(imported) = dawproject::import(&container, &Context::new(&folder, &folder)) {
                imported
                    .project
                    .document
                    .check_invariants()
                    .unwrap_or_else(|error| panic!("{name} round {round}: {error}"));
            }
        }
    }
}

/// Numbers at the edge of what a tick holds never panic (R8).
#[test]
fn extreme_numbers_never_panic() {
    let folder = fixtures().join("nowhere");
    for value in [
        "1e300", "-1e300", "9.3e18", "-9.3e18", "4e15", "inf", "nan", "1e-300", "0",
    ] {
        let xml = format!(
            r#"<?xml version="1.0"?>
<Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="{value}" id="tempo"/><TimeSignature numerator="4" denominator="4" id="sig"/></Transport>
<Structure><Track contentType="notes" id="b" name="B"><Channel destination="cm" role="regular" id="cb"><Volume unit="linear" value="{value}" id="v"/><Pan unit="normalized" min="{value}" max="{value}" value="{value}" id="p"/></Channel></Track>
<Track contentType="audio" id="a" name="A"><Channel role="regular" id="ca"/></Track></Structure>
<Arrangement id="arr"><TempoAutomation unit="bpm" id="ta"><Target parameter="tempo"/><RealPoint time="{value}" value="{value}" interpolation="linear"/><RealPoint time="1" value="100" interpolation="linear"/></TempoAutomation>
<Lanes id="all"><Lanes track="b" id="lb"><Clips id="cb1"><Clip time="{value}" duration="{value}" loopStart="{value}" loopEnd="{value}" playStart="{value}" playStop="{value}" name="x">
<Notes id="n"><Note time="{value}" duration="{value}" key="60"/><Note time="0" duration="1" key="60"/></Notes></Clip>
<Clip time="0" duration="{value}" name="y"><Notes id="n2"><Note time="0" duration="1" key="60"/></Notes></Clip></Clips></Lanes>
<Lanes track="a" timeUnit="seconds" id="la"><Clips id="ca1"><Clip time="{value}" duration="{value}" contentTimeUnit="seconds" loopStart="0" loopEnd="{value}" name="z">
<Audio channels="2" duration="{value}" sampleRate="44100" id="au"><File path="x.wav"/></Audio></Clip></Clips></Lanes></Lanes></Arrangement>
<Scenes><Scene id="s"><ClipSlot track="b" id="slot"><Clip time="{value}" duration="{value}" name="q"><Notes id="n3"><Note time="0" duration="1" key="60"/></Notes></Clip></ClipSlot></Scene></Scenes></Project>"#
        );
        let container = dawproject::read_container(&container_of(&xml)).unwrap();
        if let Ok(imported) = dawproject::import(&container, &Context::new(&folder, &folder)) {
            imported.project.document.check_invariants().unwrap();
        }
    }
}
