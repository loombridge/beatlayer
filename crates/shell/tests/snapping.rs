//! Snapping a drag to clip edges, locators and time-signature markers as
//! well as the grid, headless (WP-6.60). The behaviour is in the arrangement
//! editing spec, R21 to R23.

use std::path::{Path, PathBuf};

use bl_core::time::{Ticks, TimeSignature};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, Locator, LocatorId,
};
use bl_shell::Shell;
use bl_shell::arrangement::snapping::Dragged;
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// 60 points to a bar, so 6 points is a tenth of a bar.
const TICKS_PER_POINT: f64 = BAR as f64 / 60.0;

/// A position `bars` bars from 1.1.1.
fn bars(bars: f64) -> Ticks {
    Ticks((bars * BAR as f64).round() as i64)
}

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

struct Song {
    shell: Shell,
    /// The clip being dragged, over bars 14.25 to 15.25 on track 2.
    mine: ClipId,
    locator: LocatorId,
}

/// The factory set with a clip on track 3 over bars 7.25 to 9.25 (another
/// track's), the clip being dragged on track 2 over 14.25 to 15.25, locators
/// at 3.25 and 6.05, and a 3/4 marker at 12.5; the grid is whole bars.
fn song() -> Song {
    let mut editor = factory_default_set();
    let tracks: Vec<_> = editor.document().top_level().collect();
    let mut clip = |track: usize, start: f64, length: f64, seed: u8| {
        let id: ClipId = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track: tracks[track],
                clip: Clip::new(
                    id,
                    bars(start),
                    bars(length),
                    ClipData::Audio(AudioData::new(ContentHash([seed; 32]))),
                ),
            })
            .unwrap();
        id
    };
    clip(3, 7.25, 2.0, 1);
    let mine = clip(2, 14.25, 1.0, 2);
    let mut locator = |at: f64| {
        let id: LocatorId = editor.new_id();
        let locator = Locator::new(id, bars(at), "L".to_owned());
        editor.apply(Command::AddLocator { locator }).unwrap();
        id
    };
    let first = locator(3.25);
    locator(6.05);
    editor
        .apply(Command::SetTimeSignatureChange {
            at: bars(12.5),
            signature: TimeSignature::new(3, 4).unwrap(),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Snap.blset" }))
        .unwrap();
    Song {
        shell,
        mine,
        locator: first,
    }
}

impl Song {
    /// Where a drag whose position is at `at` bars lands, in bars, and the
    /// item it snapped to.
    fn landing(&self, at: f64, length: f64, invert: bool) -> (f64, Option<f64>) {
        let clips = [self.mine];
        let snapped = self.shell.arrangement().snap_to_items(
            self.shell.editor().document(),
            bars(at),
            bars(length),
            Dragged {
                clips: &clips,
                ..Dragged::default()
            },
            TICKS_PER_POINT,
            invert,
        );
        let in_bars = |t: Ticks| t.get() as f64 / BAR as f64;
        (in_bars(snapped.at), snapped.line.map(in_bars))
    }
}

#[test]
fn a_locator_a_clip_edge_and_a_signature_marker_within_six_points_win() {
    // Spec: arrangement-editing R21. Within 6 points (a tenth of a bar here)
    // of the start, an item's edge wins; beyond it the grid line holds.
    let s = song();
    // A locator at 3.25: 0.05 bar away, then 0.15.
    assert_eq!(s.landing(3.2, 0.0, false), (3.25, Some(3.25)));
    assert_eq!(s.landing(3.4, 0.0, false), (3.0, None));
    // Another track's clip starts at 7.25.
    assert_eq!(s.landing(7.3, 0.0, false), (7.25, Some(7.25)));
    assert_eq!(s.landing(7.4, 0.0, false), (7.0, None));
    // A 3/4 marker at 12.5.
    assert_eq!(s.landing(12.55, 0.0, false), (12.5, Some(12.5)));
    assert_eq!(s.landing(12.2, 0.0, false), (12.0, None));
}

#[test]
fn a_clips_end_snaps_as_well_as_its_start() {
    // Spec: arrangement-editing R21. A one-bar clip starting at 8.2 ends at
    // 9.2, 0.05 bar from the other clip's end at 9.25: it starts at 8.25.
    let s = song();
    assert_eq!(s.landing(8.2, 1.0, false), (8.25, Some(9.25)));
    // Its end at 9.4 is 0.15 away, as is its start from every item: the
    // grid line under the start holds.
    assert_eq!(s.landing(8.4, 1.0, false), (8.0, None));
    // The locator at 3.25 under the end of a two-bar clip.
    assert_eq!(s.landing(1.2, 2.0, false), (1.25, Some(3.25)));
}

#[test]
fn an_item_wins_over_a_grid_line_within_the_same_distance() {
    // Spec: arrangement-editing R21. The locator at 6.05 is 0.04 bar from
    // 6.01 and the grid line at 6.0 is 0.01 away.
    let s = song();
    assert_eq!(s.landing(6.01, 0.0, false), (6.05, Some(6.05)));
}

#[test]
fn a_drag_never_snaps_to_the_edges_of_the_clips_it_carries() {
    // Spec: arrangement-editing R21, R22. The dragged clip over 14.25 to
    // 15.25 is 0.05 bar from 14.3, but its own start isn't a place to snap
    // to.
    let s = song();
    assert_eq!(s.landing(14.3, 0.0, false), (14.0, None));
}

#[test]
fn a_locator_snaps_to_the_others_but_not_to_itself() {
    // Spec: arrangement-editing R22.
    let s = song();
    let locators = [s.locator];
    let at = |dragged: Dragged<'_>, to: f64| {
        s.shell
            .arrangement()
            .snap_to_items(
                s.shell.editor().document(),
                bars(to),
                Ticks::ZERO,
                dragged,
                TICKS_PER_POINT,
                false,
            )
            .at
    };
    // The 6.05 locator is within reach of 6.1, the clip edge at 7.25 of 7.2.
    let carrying = Dragged {
        locators: &locators,
        ..Dragged::default()
    };
    assert_eq!(at(carrying, 6.1), bars(6.05));
    assert_eq!(at(carrying, 7.2), bars(7.25));
    // The locator being dragged at 3.25 isn't snapped to: 3.3 goes to the
    // grid's 3.0.
    assert_eq!(at(carrying, 3.3), bars(3.0));
    assert_eq!(at(Dragged::default(), 3.3), bars(3.25));
    // The marker being dragged isn't either.
    let marker = Dragged {
        marker: Some(bars(12.5)),
        ..Dragged::default()
    };
    assert_eq!(at(marker, 12.55), bars(12.5), "the grid line is there too");
    assert_eq!(at(marker, 12.58), bars(12.5));
}

#[test]
fn snapping_off_leaves_the_drag_free_and_cmd_flips_it() {
    // Spec: arrangement-editing R23.
    let mut s = song();
    // Snapping on: Cmd held snaps to nothing.
    assert_eq!(s.landing(3.2, 0.0, true), (3.2, None));
    s.shell
        .run("arrangement.snap_to_grid", json!({ "on": false }))
        .unwrap();
    // Off: items don't snap either.
    assert_eq!(s.landing(3.2, 0.0, false), (3.2, None));
    assert_eq!(s.landing(7.3, 0.0, false), (7.3, None));
    // Off with Cmd held: snapping, items included.
    assert_eq!(s.landing(3.2, 0.0, true), (3.25, Some(3.25)));
    assert_eq!(s.landing(3.4, 0.0, true), (3.0, None));
}
