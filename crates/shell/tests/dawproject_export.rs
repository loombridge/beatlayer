//! Writing the open set as a DAWproject file (WP-6.37), as the behaviour
//! behind `set.export_dawproject`: the file's name, the refusal of a file
//! already there, a write that stops leaving the old file whole, and the
//! media going in or being referred to.

use std::path::PathBuf;

use bl_interop::dawproject::{read_container, read_container_strict};
use bl_shell::Shell;
use bl_shell::testing::rig;
use bl_shell_spec::dawproject::{
    EXPORT_DAWPROJECT, ExportDawProjectResult, IMPORT_DAWPROJECT, ImportDawProjectResult,
};
use serde_json::json;

/// The folder the fixtures are in.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dawproject")
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-shell-dawproject-export-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell holding the fixture `name`, with the files it holds written under
/// `folder`.
fn shell_with(name: &str, folder: &TempDir) -> Shell {
    let (mut shell, _control) = rig();
    let temporary = folder.0.join("Temporary");
    std::fs::create_dir_all(&temporary).expect("the temporary folder");
    shell
        .run(
            "settings.set_temporary_folder",
            json!({ "path": temporary.to_string_lossy() }),
        )
        .expect("the temporary folder is set");
    let path = fixtures().join(format!("{name}.dawproject"));
    shell
        .run(IMPORT_DAWPROJECT, json!({ "path": path.to_string_lossy() }))
        .expect("the fixture imports");
    shell
}

fn export(shell: &mut Shell, args: serde_json::Value) -> Result<ExportDawProjectResult, String> {
    shell
        .run(EXPORT_DAWPROJECT, args)
        .map(|value| serde_json::from_value(value).expect("the result"))
        .map_err(|error| error.to_string())
}

/// Spec: dawproject R65. The file gets `.dawproject` when it lacks it, the
/// result names it, the file reads in strict mode, and the set is as it was.
#[test]
fn an_export_writes_the_file_and_leaves_the_set_alone() {
    let folder = TempDir::new("writes");
    let mut shell = shell_with("clips", &folder);
    let before = shell.editor().document().clone();
    let target = folder.0.join("Song");
    let result = export(&mut shell, json!({ "path": target.to_string_lossy() })).expect("exported");
    let written = folder.0.join("Song.dawproject");
    assert_eq!(PathBuf::from(&result.path), written);
    let bytes = std::fs::read(&written).expect("the file is there");
    let container = read_container_strict(&bytes).expect("it reads in strict mode");
    assert_eq!(container.project.version.as_deref(), Some("1.0"));
    assert_eq!(shell.editor().document(), &before);
    assert!(!shell.unsaved_changes());
    // No hidden file is left beside it.
    let hidden: Vec<_> = std::fs::read_dir(&folder.0)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(hidden.is_empty(), "{hidden:?}");
}

/// Spec: dawproject R65. A file already there is refused unless `replace`
/// is set.
#[test]
fn an_existing_file_is_refused_without_replace_and_replaced_with_it() {
    let folder = TempDir::new("replace");
    let mut shell = shell_with("clips", &folder);
    let target = folder.0.join("Song.dawproject");
    std::fs::write(&target, b"old").unwrap();
    let args = json!({ "path": target.to_string_lossy() });
    let error = export(&mut shell, args).expect_err("refused");
    assert!(error.contains("already there"), "{error}");
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    export(
        &mut shell,
        json!({ "path": target.to_string_lossy(), "replace": true }),
    )
    .expect("replaced");
    assert!(read_container_strict(&std::fs::read(&target).unwrap()).is_ok());
}

/// Spec: dawproject R65. A write that stops leaves the old file whole.
#[test]
fn an_interrupted_write_leaves_the_old_file_whole() {
    let folder = TempDir::new("interrupted");
    let mut shell = shell_with("clips", &folder);
    let target = folder.0.join("Song.dawproject");
    std::fs::write(&target, b"old").unwrap();
    // The hidden file the write goes through can't be made: a folder is in
    // its place.
    std::fs::create_dir(folder.0.join(".Song.dawproject.saving")).unwrap();
    let error = export(
        &mut shell,
        json!({ "path": target.to_string_lossy(), "replace": true }),
    )
    .expect_err("the write fails");
    assert!(!error.is_empty());
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
}

/// Spec: dawproject R73. The media go into the file under their names, or
/// are referred to by their absolute paths.
#[test]
fn the_media_are_embedded_or_referred_to_by_their_paths() {
    let folder = TempDir::new("media");
    let mut shell = shell_with("media", &folder);
    let embedded = folder.0.join("Embedded.dawproject");
    export(&mut shell, json!({ "path": embedded.to_string_lossy() })).expect("exported");
    let container = read_container(&std::fs::read(&embedded).unwrap()).unwrap();
    assert!(
        container.files.keys().any(|path| path == "audio/drum.wav"),
        "{:?}",
        container.files.keys().collect::<Vec<_>>()
    );

    let referred = folder.0.join("Referred.dawproject");
    export(
        &mut shell,
        json!({ "path": referred.to_string_lossy(), "embed_media": false }),
    )
    .expect("exported");
    let container = read_container(&std::fs::read(&referred).unwrap()).unwrap();
    assert!(
        !container
            .files
            .keys()
            .any(|path| path.starts_with("audio/")),
        "{:?}",
        container.files.keys().collect::<Vec<_>>()
    );
    let audio = container
        .project
        .arrangement
        .as_ref()
        .expect("an Arrangement")
        .lanes
        .iter()
        .any(|lane| format!("{lane:?}").contains("external: true"));
    assert!(audio, "no clip refers to its file by path");
}

/// Spec: dawproject R82. A set exported and imported again is the same set:
/// the file the shell writes imports as the tracks and clips it was made
/// from.
#[test]
fn an_exported_file_imports_as_the_same_set() {
    let folder = TempDir::new("round");
    let mut shell = shell_with("bitwig-shaped", &folder);
    let before = shell.editor().document().clone();
    let target = folder.0.join("Round.dawproject");
    export(&mut shell, json!({ "path": target.to_string_lossy() })).expect("exported");
    let imported: ImportDawProjectResult = serde_json::from_value(
        shell
            .run(
                IMPORT_DAWPROJECT,
                json!({ "path": target.to_string_lossy() }),
            )
            .expect("imports again"),
    )
    .unwrap();
    assert_eq!(imported.name, "Round");
    let after = shell.editor().document();
    let names = |document: &bl_model::Document| -> Vec<String> {
        document.tracks().map(|t| t.name().to_owned()).collect()
    };
    assert_eq!(names(after), names(&before));
    assert_eq!(after.clip_count(), before.clip_count());
}

mod render {
    //! The acceptance in a render: a set exported and imported again plays
    //! the same samples as the set it was made from, at two block sizes.

    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use bl_core::time::Ticks;
    use bl_model::{
        AudioData, Clip, ClipData, Command, DeviceId, DeviceKind, Document, MidiData, Note, Output,
        PluginFormat, TrackId, TrackKind, WarpMarker, WarpMode,
    };
    use bl_shell::export::{self, Length, Render};
    use bl_shell::plugin_commands as plugin_ids;
    use bl_shell::{Shell, browser_commands};
    use bl_shell_spec::dawproject::{EXPORT_DAWPROJECT, IMPORT_DAWPROJECT};
    use serde_json::json;

    use super::TempDir;

    /// Half a second at 48 kHz, plus a bar.
    const FRAMES: u64 = 96_000;

    /// A 16-bit mono WAV file of one second at 48 kHz.
    fn wav(path: &Path, period: i16) {
        let frames = 48_000u32;
        let data = frames * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for i in 0..frames {
            let sample = ((i as i16 % period) - period / 2) * 150;
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn render(shell: &Shell, document: &Document, block: usize) -> Vec<f32> {
        let spec = Render {
            start: Ticks::ZERO,
            length: Length::Frames(FRAMES),
            sample_rate: 48_000,
            engine_rate: 48_000,
            block,
        };
        let mut samples = Vec::new();
        let rendered = export::render(
            document,
            shell.media_store(),
            shell.plugin_hosts(),
            &spec,
            60.0,
            &AtomicBool::new(false),
            &mut |_| {},
            &mut |frames| {
                samples.extend_from_slice(frames);
                Ok(())
            },
            None,
        )
        .expect("the set renders");
        assert!(rendered.unhosted.is_empty(), "{:?}", rendered.unhosted);
        assert!(rendered.missing_media.is_empty());
        samples
    }

    /// Polls the shell until the plug-in of every plug-in device is running.
    fn until_running(shell: &mut Shell) {
        let devices: Vec<DeviceId> = shell
            .editor()
            .document()
            .tracks()
            .flat_map(|track| track.devices_deep().collect::<Vec<_>>())
            .filter(|device| matches!(device.kind, DeviceKind::Plugin { .. }))
            .map(|device| device.id)
            .collect();
        assert!(!devices.is_empty());
        let until = Instant::now() + Duration::from_secs(10);
        for device in devices {
            loop {
                shell.poll();
                if shell.plugins().status(device).is_some_and(|s| s.active) {
                    break;
                }
                assert!(Instant::now() < until, "the plug-in never ran");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Spec: dawproject R82. A set with warped audio, MIDI through the test
    /// CLAP plug-in, a group and a send renders the same after it is
    /// exported and imported again (a null test at block sizes 64 and 512).
    #[test]
    fn an_exported_and_imported_set_renders_the_same_samples() {
        let folder = TempDir::new("render");
        let mut shell = Shell::headless();
        shell.start_audio();
        let temporary = folder.0.join("Temporary");
        std::fs::create_dir_all(&temporary).unwrap();
        shell
            .run(
                "settings.set_temporary_folder",
                json!({ "path": temporary.to_string_lossy() }),
            )
            .unwrap();
        shell
            .run(
                plugin_ids::SCAN,
                json!({ "paths": [bl_test_clap::bundle()] }),
            )
            .unwrap();

        // Warped audio, in a group, with a send to a return.
        let (loop_file, drum_file) = (folder.0.join("loop.wav"), folder.0.join("drums.wav"));
        wav(&loop_file, 100);
        wav(&drum_file, 64);
        for file in [&loop_file, &drum_file] {
            shell
                .run(
                    browser_commands::ADD_TO_ARRANGEMENT,
                    json!({ "paths": [file.to_string_lossy()], "at": 0 }),
                )
                .unwrap();
        }
        let track_of = |shell: &Shell, name: &str| -> TrackId {
            shell
                .editor()
                .document()
                .tracks()
                .find(|track| track.clips().next().is_some() && track.name().contains(name))
                .map(bl_model::Track::id)
                .unwrap_or_else(|| panic!("no track {name}"))
        };
        let looped = track_of(&shell, "loop");
        let drums = track_of(&shell, "drums");
        let group: TrackId = shell.new_id();
        let reverb: TrackId = shell.new_id();
        let lead: TrackId = shell.new_id();
        let clip = shell
            .editor()
            .document()
            .track(looped)
            .unwrap()
            .clips()
            .next()
            .cloned()
            .unwrap();
        let q = Ticks::QUARTER;
        shell.edit(|editor| {
            let mut apply = |command| editor.apply(command).expect("a valid command");
            apply(Command::CreateTrack {
                id: group,
                kind: TrackKind::Group,
                name: "Band".into(),
                parent: None,
                index: 0,
            });
            apply(Command::MoveTrack {
                track: looped,
                parent: Some(group),
                index: 0,
            });
            apply(Command::CreateTrack {
                id: reverb,
                kind: TrackKind::Return,
                name: "Reverb".into(),
                parent: None,
                index: 0,
            });
            apply(Command::SetSend {
                track: drums,
                to: reverb,
                level: 0.5,
            });
            apply(Command::SetVolume {
                track: looped,
                volume: 0.75,
            });
            apply(Command::SetPan {
                track: drums,
                pan: -0.25,
            });
            let ClipData::Audio(data) = &clip.data else {
                unreachable!()
            };
            apply(Command::SetClipData {
                clip: clip.id,
                data: ClipData::Audio(AudioData {
                    warp: true,
                    warp_markers: vec![
                        WarpMarker {
                            time: 0.0,
                            beat: Ticks::ZERO,
                        },
                        WarpMarker {
                            time: 1.0,
                            beat: q * 3,
                        },
                    ],
                    tempo: Some(180.0),
                    warp_mode: WarpMode::RePitch,
                    ..data.clone()
                }),
            });
            apply(Command::CreateTrack {
                id: lead,
                kind: TrackKind::Midi,
                name: "Lead".into(),
                parent: None,
                index: 3,
            });
            apply(Command::SetOutput {
                track: drums,
                output: Output::Main,
            });
        });
        // MIDI through the test plug-in, placed before the instrument.
        shell
            .run(
                "device.add",
                json!({ "track": lead,
                        "device": { "kind": { "native": { "device_type": "beatlayer.cascade" } },
                                    "name": "Cascade" } }),
            )
            .unwrap();
        let loaded = shell
            .run(
                plugin_ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": lead }),
            )
            .unwrap();
        let device: DeviceId = serde_json::from_value(loaded["device"].clone()).unwrap();
        shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": bl_test_clap::GAIN, "value": 0.5 }),
            )
            .unwrap();
        shell
            .run(
                "device.set_role",
                json!({ "device": device, "role": "midi_effect" }),
            )
            .unwrap();
        let notes = [(60u8, 0i64), (64, 1), (67, 2)]
            .map(|(key, beat)| Note::new(key, q * beat, q, 100 - key / 2));
        let midi = shell.new_id();
        shell
            .edit(|editor| {
                editor.apply(Command::CreateClip {
                    track: lead,
                    clip: Clip::new(
                        midi,
                        Ticks::ZERO,
                        q * 4,
                        ClipData::Midi(MidiData::new(notes)),
                    ),
                })
            })
            .unwrap();
        until_running(&mut shell);
        let original = shell.editor().document().clone();
        let before: Vec<Vec<f32>> = [64, 512]
            .into_iter()
            .map(|block| render(&shell, &original, block))
            .collect();
        for samples in &before {
            assert!(samples.iter().any(|s| s.abs() > 0.01), "silence");
        }

        // Export it, throw the set away, and import the file.
        let target = folder.0.join("Render.dawproject");
        shell
            .run(
                EXPORT_DAWPROJECT,
                json!({ "path": target.to_string_lossy() }),
            )
            .expect("the set exports");
        shell.run("set.discard_changes", json!({})).unwrap();
        shell
            .run(
                IMPORT_DAWPROJECT,
                json!({ "path": target.to_string_lossy() }),
            )
            .expect("the file imports");
        until_running(&mut shell);
        let imported = shell.editor().document().clone();
        assert!(
            imported
                .tracks()
                .any(|track| track.devices().any(|d| matches!(
                    d.kind,
                    DeviceKind::Plugin {
                        format: PluginFormat::Clap,
                        ..
                    }
                ))),
            "the plug-in came back"
        );
        for (index, block) in [64, 512].into_iter().enumerate() {
            let after = render(&shell, &imported, block);
            assert_eq!(after.len(), before[index].len(), "block {block}: length");
            let worst = after
                .iter()
                .zip(&before[index])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert_eq!(
                worst, 0.0,
                "block size {block}: the renders differ by {worst}"
            );
        }
    }
}
