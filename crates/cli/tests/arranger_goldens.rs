//! WP-6.45's acceptance: a set rendered after a section is moved matches,
//! sample for sample, a render of the same arrangement built by hand (the
//! arranger track spec, R8).
//!
//! The null tests against the stored references are in `golden.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_cli::golden::{BLOCK, SAMPLE_RATE, fixture};
use bl_cli::render::{self, Cue, Settings};
use bl_core::time::Ticks;
use bl_engine::MediaMap;
use bl_model::{AudioData, Clip, ClipData, ContentHash, Command, Editor, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::SetStore;
use bl_shell::settings::Preferences;
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A set with one audio track and a one-bar clip of the chirp at each of
/// `parts`, in bars from the song's start, each playing from `file_start`
/// seconds into the file, so that the clips sound different.
fn set(file: ContentHash, parts: &[(i64, f64)]) -> Editor {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    for &(bar, file_start) in parts {
        let mut audio = AudioData::new(file);
        audio.start = file_start;
        let clip = Clip::new(editor.new_id(), Ticks(bar * BAR), Ticks(BAR), ClipData::Audio(audio));
        editor.apply(Command::CreateClip { track, clip }).unwrap();
    }
    editor
}

/// Renders the first `bars` bars of `editor`'s song with the chirp as the
/// file its clips play.
fn render(editor: &Editor, media: &MediaMap, bars: i64) -> Vec<f32> {
    let settings = Settings {
        sample_rate: SAMPLE_RATE,
        block: BLOCK,
        frames: SAMPLE_RATE.hz() as u64 * 2 * bars as u64,
    };
    let mut samples = Vec::new();
    render::render_with(
        editor.document(),
        Arc::new(media.clone()),
        &[],
        &[Cue::PLAY],
        &settings,
        |output| render::interleave(output, &mut samples),
    );
    samples
}

#[test]
fn a_set_after_a_section_move_renders_as_the_arrangement_built_by_hand() {
    // Spec: arranger-track R8. Three bars: A, B and C, each playing a
    // different second of the chirp; moving C's section before A's leaves
    // C, A, B.
    let base = fixture("clip-fades").expect("the fixture exists").scene();
    let file = base.files[0].hash;
    let (a, b, c) = ((0, 0.0), (1, 1.0), (2, 2.0));
    let editor = set(file, &[a, b, c]);
    // The set built by hand has no fades at its clips' edges.
    let mut preferences = Preferences::default();
    preferences.create_fades_on_clip_edges.0 = false;
    let mut shell = Shell::new(
        preferences,
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Sections.blset" }))
        .unwrap();
    for (from, to) in [(0, 1), (1, 2), (2, 3)] {
        shell
            .run(
                "arranger.add_section",
                json!({ "start": Ticks(from * BAR), "end": Ticks(to * BAR) }),
            )
            .unwrap();
    }
    let third = shell.editor().document().sections()[2].id;
    shell
        .run(
            "arranger.move_section",
            json!({ "section": third, "to": Ticks::ZERO }),
        )
        .unwrap();
    let moved = render(shell.editor(), &base.media, 4);
    let by_hand = render(&set(file, &[(0, c.1), (1, a.1), (2, b.1)]), &base.media, 4);
    assert!(moved.iter().any(|&s| s != 0.0), "the render is silent");
    assert_eq!(moved.len(), by_hand.len());
    assert!(
        moved
            .iter()
            .zip(&by_hand)
            .all(|(moved, by_hand)| moved.to_bits() == by_hand.to_bits()),
        "the render differs from the arrangement built by hand"
    );
    // And it is not the render before the move.
    let before = render(&set(file, &[a, b, c]), &base.media, 4);
    assert!(moved.iter().zip(&before).any(|(m, b)| m != b));
}
