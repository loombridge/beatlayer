//! Take lanes in the Arrangement's view, driven headless through
//! `Shell::run` (WP-6.22; D17): showing and hiding them, selecting on them,
//! the Arrangement's edits on them, and the comp commands on the selection.
//! The behaviour is in the comping spec, R15 and R23 to R30, and the
//! arrangement editing and audio clip editing specs, R29.

use std::path::{Path, PathBuf};

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, ContentHash, Document, Editor, MidiData, Note, TakeLaneId,
    TrackId,
};
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

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

/// The factory default set: tracks 0 and 1 are MIDI, 2 and 3 audio.
struct Song {
    shell: Shell,
    tracks: Vec<TrackId>,
}

fn song() -> Song {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Lanes.blset" }))
        .unwrap();
    Song { shell, tracks }
}

fn media(n: u8) -> ContentHash {
    ContentHash([n; 32])
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn state(&self) -> bl_shell::arrangement_commands::ArrangementState {
        self.shell.arrangement().state(self.doc())
    }

    /// An audio clip of file `file` on `track`'s main lane.
    fn main_clip(&mut self, track: usize, file: u8, start: i64, bars: i64) -> ClipId {
        let id: ClipId = self.shell.new_id();
        let clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData::new(media(file))),
        );
        let track = self.tracks[track];
        self.shell
            .run("clip.create", json!({ "track": track, "clip": clip }))
            .unwrap();
        id
    }

    /// A take lane on `track` with one audio clip of file `file`.
    fn lane(&mut self, track: usize, file: u8, start: i64, bars: i64) -> TakeLaneId {
        let track = self.tracks[track];
        let name = format!(
            "Take {}",
            self.doc().track(track).unwrap().take_lanes().len() + 1
        );
        let lane: TakeLaneId = self.shell.new_id();
        self.shell
            .run(
                "track.add_take_lane",
                json!({ "track": track, "lane": { "id": lane, "name": name, "clips": [] } }),
            )
            .unwrap();
        self.lane_clip(lane, file, start, bars);
        lane
    }

    fn lane_clip(&mut self, lane: TakeLaneId, file: u8, start: i64, bars: i64) -> ClipId {
        let id: ClipId = self.shell.new_id();
        let clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Audio(AudioData::new(media(file))),
        );
        self.shell
            .run(
                "take_lane.create_clip",
                json!({ "lane": lane, "clip": clip }),
            )
            .unwrap();
        id
    }

    /// The track's main-lane clips as `(start bar, end bar)`, in order.
    fn main(&self, track: usize) -> Vec<(i64, i64)> {
        let mut clips: Vec<(i64, i64)> = self
            .doc()
            .track(self.tracks[track])
            .unwrap()
            .clips()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect();
        clips.sort_unstable();
        clips
    }

    /// A lane's clips as `(start bar, end bar)`, in order.
    fn on_lane(&self, lane: TakeLaneId) -> Vec<(i64, i64)> {
        let mut clips: Vec<(i64, i64)> = self
            .doc()
            .take_lane(lane)
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.start.get() / BAR, c.end().get() / BAR))
            .collect();
        clips.sort_unstable();
        clips
    }

    /// Runs `id` and checks it is one undo step that undo and redo restore
    /// exactly, leaving it done.
    fn one_step(&mut self, id: &str, args: Value) -> Value {
        let before = self.doc().clone();
        let steps = self.steps();
        let result = self
            .shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        let after = self.doc().clone();
        assert_ne!(after, before, "{id} changed nothing");
        assert_eq!(self.steps(), steps + 1, "{id} is one undo step");
        assert!(after.check_invariants().is_ok(), "{id}");
        self.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(*self.doc(), before, "undoing {id} restores the set exactly");
        self.shell.run("edit.redo", json!({})).unwrap();
        assert_eq!(*self.doc(), after, "redoing {id} restores it exactly");
        result
    }

    /// Runs a view command, which is never an undo step.
    fn view(&mut self, id: &str, args: Value) {
        let before = self.doc().clone();
        let steps = self.steps();
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(*self.doc(), before, "{id} changes no song state");
        assert_eq!(self.steps(), steps, "{id} is never an undo step");
    }

    /// Selects time on take lanes of the given tracks.
    fn select_on(&mut self, lanes: Value, start: i64, end: i64) {
        self.view(
            "arrangement.select_time",
            json!({ "start": start * BAR, "end": end * BAR, "lanes": lanes }),
        );
    }
}

// Spec: comping R23, R24. Show Take Lanes adds a row under the main lane for
// each lane, 44 points tall, and hides them again; it is view state and never
// an undo step.
#[test]
fn showing_and_hiding_take_lanes_is_view_state_that_adds_their_rows() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two) = (s.lane(2, 1, 0, 2), s.lane(2, 2, 0, 2));
    let rows = |s: &Song| s.shell.arrangement().rows(s.doc());
    let before = rows(&s).iter().find(|r| r.track == track).copied().unwrap();
    assert_eq!(before.take, 0.0, "a track's take lanes start hidden");
    s.view("arrangement.select_tracks", json!({ "tracks": [track] }));
    s.view("arrangement.show_take_lanes", json!({}));
    assert_eq!(s.state().shown_take_lanes, [track]);
    let row = rows(&s).iter().find(|r| r.track == track).copied().unwrap();
    assert_eq!(row.take, 88.0, "two lanes of 44 points");
    let lanes: Vec<_> = s
        .shell
        .arrangement()
        .take_lane_rows(s.doc(), &row)
        .collect();
    assert_eq!(
        lanes.iter().map(|l| l.lane).collect::<Vec<_>>(),
        [one, two],
        "in the lanes' order"
    );
    assert_eq!(lanes[0].top, row.top + row.height);
    assert_eq!(lanes[1].top, lanes[0].top + 44.0);
    assert_eq!(row.bottom(), row.top + row.height + 88.0);

    // Hidden in Automation Mode; Show Take Lanes leaves it again.
    s.view("arrangement.automation_mode", json!({ "on": true }));
    let row = rows(&s).iter().find(|r| r.track == track).copied().unwrap();
    assert_eq!(row.take, 0.0);
    assert!(
        s.shell.arrangement().shows_take_lanes(track),
        "the switch is kept"
    );
    s.view("arrangement.show_take_lanes", json!({ "on": true }));
    assert!(!s.shell.arrangement().automation_mode());
    assert_eq!(
        rows(&s).iter().find(|r| r.track == track).unwrap().take,
        88.0
    );

    // Folded tracks show none, and toggling hides them.
    s.view("arrangement.fold_tracks", json!({ "fold": true }));
    assert_eq!(
        rows(&s).iter().find(|r| r.track == track).unwrap().take,
        0.0
    );
    s.view("arrangement.fold_tracks", json!({ "fold": false }));
    s.view("arrangement.show_take_lanes", json!({}));
    assert!(s.state().shown_take_lanes.is_empty());
    assert!(s.shell.arrangement().take_toggle_shown(s.doc(), track));
    assert!(
        !s.shell
            .arrangement()
            .take_toggle_shown(s.doc(), s.tracks[3])
    );
}

// Spec: comping R24, R4. Inserting a take lane shows the track's take lanes,
// leaving Automation Mode to do it.
#[test]
fn inserting_a_take_lane_shows_the_lanes_and_leaves_automation_mode() {
    let mut s = song();
    s.view(
        "arrangement.select_tracks",
        json!({ "tracks": [s.tracks[2]] }),
    );
    s.view("arrangement.automation_mode", json!({ "on": true }));
    let result = s.one_step("take_lane.insert", json!({}));
    let made: Vec<TakeLaneId> = serde_json::from_value(result["lanes"].clone()).unwrap();
    assert_eq!(made.len(), 1);
    assert!(s.shell.arrangement().shows_take_lanes(s.tracks[2]));
    assert!(!s.shell.arrangement().automation_mode());
}

// Spec: comping R28. Option with + or - and the wheel resize the selected
// take lanes through `arrangement.set_track_height`, from 18 to 800 points.
#[test]
fn take_lane_heights_follow_set_track_height_with_a_lane() {
    let mut s = song();
    let (one, two) = (s.lane(2, 1, 0, 2), s.lane(2, 2, 0, 2));
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    s.view("arrangement.set_track_height", json!({ "delta": 16 }));
    assert_eq!(s.shell.arrangement().take_lane_height(one), 60.0);
    assert_eq!(s.shell.arrangement().take_lane_height(two), 44.0);
    s.view(
        "arrangement.set_track_height",
        json!({ "take_lanes": [two], "height": 5000 }),
    );
    assert_eq!(s.shell.arrangement().take_lane_height(two), 800.0);
    s.view("arrangement.set_track_height", json!({ "height": 1 }));
    assert_eq!(s.shell.arrangement().take_lane_height(one), 18.0);
    // The track's own height was left alone.
    assert_eq!(s.shell.arrangement().track_height(s.tracks[2]), 88.0);
}

// Spec: comping R26. Selecting a take lane's header selects it (Cmd-click
// adds or takes away); its track is selected with it; selecting is never an
// undo step.
#[test]
fn clicking_a_take_lane_header_selects_it_and_its_track() {
    let mut s = song();
    let (one, two) = (s.lane(2, 1, 0, 2), s.lane(2, 2, 0, 2));
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    let state = s.state();
    assert_eq!(state.selected_take_lanes, [one]);
    assert_eq!(state.selected_tracks, [s.tracks[2]]);
    s.view(
        "arrangement.select_take_lanes",
        json!({ "lanes": [two], "toggle": true }),
    );
    assert_eq!(s.state().selected_take_lanes, [one, two]);
    s.view(
        "arrangement.select_take_lanes",
        json!({ "lanes": [one], "toggle": true }),
    );
    assert_eq!(s.state().selected_take_lanes, [two]);
    s.view("arrangement.select_take_lanes", json!({ "lanes": [] }));
    assert!(s.state().selected_take_lanes.is_empty());
    assert!(s.shell.active_contexts().contains(&Context::Arrangement));
    assert!(!s.shell.active_contexts().contains(&Context::TakeLanes));
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    assert_eq!(
        s.shell.active_contexts(),
        [Context::TakeLanes, Context::Arrangement, Context::Global],
        "the take-lanes context applies with a take lane selected"
    );
}

// Spec: comping R26. Time selected over take lanes names them; clips on take
// lanes select the lanes they sit on.
#[test]
fn time_and_clips_on_take_lanes_select_the_lanes() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two) = (s.lane(2, 1, 0, 4), s.lane(2, 2, 0, 4));
    s.select_on(
        json!([
            { "type": "take", "track": track, "lane": one },
            { "type": "take", "track": track, "lane": two },
        ]),
        1,
        3,
    );
    let state = s.state();
    assert_eq!(state.selected_tracks, [track]);
    assert_eq!(
        state.time_selection_lanes.len(),
        2,
        "both lanes, not the main lane"
    );
    assert_eq!(s.shell.arrangement().selection_take_lanes(), [one, two]);
    assert!(s.shell.active_contexts().contains(&Context::TakeLanes));
    // A clip on a lane selects the lane and the time it spans.
    let clip = s.doc().take_lane(two).unwrap().clips[0].id;
    s.view("arrangement.select_clips", json!({ "clips": [clip] }));
    assert_eq!(s.shell.arrangement().selection_take_lanes(), [two]);
    assert_eq!(s.state().time_selection.unwrap().end, Ticks(4 * BAR));
    // A bad lane for its track is refused.
    let other = s.tracks[3];
    let error = s
        .shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "lanes": [{ "type": "take", "track": other, "lane": one }] }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("isn't one of"), "{error}");
}

// Spec: comping R26. Esc with no time selection moves the focus to the
// selected lanes' headers; clicking a take lane's lane selects it with the
// insert marker.
#[test]
fn escape_and_the_insert_marker_on_a_take_lane() {
    let mut s = song();
    let track = s.tracks[2];
    let one = s.lane(2, 1, 0, 4);
    s.view(
        "arrangement.set_insert_marker",
        json!({ "at": BAR, "lane": one }),
    );
    assert_eq!(s.shell.arrangement().selection_take_lanes(), [one]);
    assert_eq!(s.state().selected_tracks, [track]);
    assert!(s.state().time_selection.is_none());
    s.view("arrangement.focus_title_bar", json!({}));
    assert_eq!(s.state().selected_take_lanes, [one]);
    assert!(!s.shell.arrangement().title_bars_selected());
}

// Spec: comping R25, S-COMP-007. Left from a take lane's header selects the
// track's title bar and hides its take lanes.
#[test]
fn left_from_a_take_lane_header_selects_the_title_bar_and_hides_the_lanes() {
    let mut s = song();
    let track = s.tracks[2];
    let one = s.lane(2, 1, 0, 4);
    s.view("arrangement.select_tracks", json!({ "tracks": [track] }));
    s.view("arrangement.show_take_lanes", json!({ "on": true }));
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    // The key binding's arguments.
    let args = json!({ "on": false, "title_bars": true });
    assert!(s.shell.can_invoke("arrangement.show_take_lanes", &args));
    s.view("arrangement.show_take_lanes", args);
    assert!(s.shell.arrangement().title_bars_selected());
    assert!(s.state().selected_take_lanes.is_empty());
    assert!(s.state().shown_take_lanes.is_empty());
    // Without a header selected it can't run.
    assert!(!s.shell.can_invoke(
        "arrangement.show_take_lanes",
        &json!({ "on": false, "title_bars": true })
    ));
}

// Spec: comping R17, R27, R28. Enter copies the time selected on a take lane
// to the main lane in one undo step, and an agent reads the main lane back.
#[test]
fn enter_copies_the_time_selected_on_a_take_lane_to_the_main_lane() {
    let mut s = song();
    let track = s.tracks[2];
    let lane = s.lane(2, 1, 0, 4);
    s.main_clip(2, 9, 0, 4);
    s.select_on(
        json!([{ "type": "take", "track": track, "lane": lane }]),
        1,
        3,
    );
    assert!(
        s.shell
            .can_invoke("take_lane.copy_to_main_lane", &json!({}))
    );
    let result = s.one_step("take_lane.copy_to_main_lane", json!({}));
    let created: Vec<ClipId> = serde_json::from_value(result["created"].clone()).unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(
        s.main(2),
        [(0, 1), (1, 3), (3, 4)],
        "the main lane, read back"
    );
    assert_eq!(s.on_lane(lane), [(0, 4)], "the take was left as it was");
    assert_eq!(
        s.shell.arrangement().selection_take_lanes(),
        [lane],
        "the selection stays"
    );
    // Without a selection on a take lane it can't run.
    s.view("arrangement.select_tracks", json!({ "tracks": [track] }));
    assert!(
        !s.shell
            .can_invoke("take_lane.copy_to_main_lane", &json!({}))
    );
}

// Spec: comping R17. With a selection over several take lanes of a track the
// last selected is copied from, and clips on a take lane copy as their span.
#[test]
fn enter_copies_from_the_last_selected_lane_and_from_selected_clips() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two) = (s.lane(2, 1, 0, 4), s.lane(2, 2, 0, 4));
    s.select_on(
        json!([
            { "type": "take", "track": track, "lane": one },
            { "type": "take", "track": track, "lane": two },
        ]),
        0,
        2,
    );
    s.one_step("take_lane.copy_to_main_lane", json!({}));
    let main = s.doc().track(track).unwrap().clips().next().unwrap();
    let ClipData::Audio(audio) = &main.data else {
        panic!("an audio clip");
    };
    assert_eq!(audio.media, media(2), "from the last selected lane");
    // Selected clips: their span.
    let clip = s.doc().take_lane(one).unwrap().clips[0].id;
    s.view("arrangement.select_clips", json!({ "clips": [clip] }));
    s.one_step("take_lane.copy_to_main_lane", json!({}));
    assert_eq!(s.main(2), [(0, 4)]);
}

// Spec: comping R19. Cmd+Down on a take lane replaces the main lane with the
// next take and moves the selection to the lane taken from.
#[test]
fn cmd_down_replaces_with_the_next_take_and_moves_the_selection() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two, three) = (s.lane(2, 1, 0, 4), s.lane(2, 2, 0, 4), s.lane(2, 3, 0, 4));
    s.main_clip(2, 9, 0, 4);
    s.select_on(
        json!([{ "type": "take", "track": track, "lane": one }]),
        0,
        2,
    );
    let args = json!({ "direction": "next" });
    assert!(s.shell.can_invoke("take_lane.replace_with_take", &args));
    s.one_step("take_lane.replace_with_take", args.clone());
    assert_eq!(s.main(2), [(0, 2), (2, 4)]);
    assert_eq!(
        s.shell.arrangement().selection_take_lanes(),
        [two],
        "the selection moved to the take it came from"
    );
    s.one_step("take_lane.replace_with_take", args);
    assert_eq!(s.shell.arrangement().selection_take_lanes(), [three]);
    // From the main lane it counts from the lane each part uses.
    s.view("arrangement.select_tracks", json!({ "tracks": [track] }));
    s.view(
        "arrangement.select_time",
        json!({ "start": 0, "end": 2 * BAR }),
    );
    s.one_step(
        "take_lane.replace_with_take",
        json!({ "direction": "previous" }),
    );
    assert_eq!(
        s.shell.arrangement().selection_take_lanes(),
        Vec::<TakeLaneId>::new()
    );
}

// Spec: comping R15. T auditions each track's selected lane, the last of
// several, and switches them off when each is auditioned already.
#[test]
fn t_auditions_the_selected_lane_of_each_track_and_toggles() {
    let mut s = song();
    let (a1, a2) = (s.lane(2, 1, 0, 4), s.lane(2, 2, 0, 4));
    let b1 = s.lane(3, 3, 0, 4);
    s.select_on(
        json!([
            { "type": "take", "track": s.tracks[2], "lane": a1 },
            { "type": "take", "track": s.tracks[2], "lane": a2 },
            { "type": "take", "track": s.tracks[3], "lane": b1 },
        ]),
        0,
        2,
    );
    let result = s.one_step("take_lane.audition", json!({}));
    let acted: Vec<TakeLaneId> = serde_json::from_value(result["lanes"].clone()).unwrap();
    assert_eq!(acted, [a2, b1]);
    assert_eq!(s.doc().track(s.tracks[2]).unwrap().audition(), Some(a2));
    assert_eq!(s.doc().track(s.tracks[3]).unwrap().audition(), Some(b1));
    s.one_step("take_lane.audition", json!({}));
    assert_eq!(s.doc().track(s.tracks[2]).unwrap().audition(), None);
    // The explicit forms.
    s.one_step("take_lane.audition", json!({ "lanes": [a1], "on": true }));
    assert_eq!(s.doc().track(s.tracks[2]).unwrap().audition(), Some(a1));
}

// Spec: comping R5, R7, R28. Cmd+D and Delete with take lane headers
// selected duplicate and delete the lanes, one undo step each; the copies are
// selected.
#[test]
fn duplicate_and_delete_act_on_selected_take_lane_headers() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two) = (s.lane(2, 1, 0, 2), s.lane(2, 2, 0, 2));
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    assert!(s.shell.can_invoke("arrangement.duplicate", &json!({})));
    // Undo and redo let go of the selection, so check the copy's first.
    s.shell.run("arrangement.duplicate", json!({})).unwrap();
    let copy = s.state().selected_take_lanes;
    assert_eq!(copy.len(), 1, "the copy is selected");
    s.shell.run("edit.undo", json!({})).unwrap();
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    s.one_step("arrangement.duplicate", json!({}));
    let lanes: Vec<TakeLaneId> = s
        .doc()
        .track(track)
        .unwrap()
        .take_lanes()
        .map(|l| l.id)
        .collect();
    assert_eq!(lanes.len(), 3);
    assert_eq!(lanes[0], one);
    assert_eq!(lanes[2], two);
    s.view(
        "arrangement.select_take_lanes",
        json!({ "lanes": [lanes[1]] }),
    );
    assert!(s.shell.can_invoke("arrangement.delete", &json!({})));
    s.one_step("arrangement.delete", json!({}));
    assert_eq!(s.doc().track(track).unwrap().take_lanes().len(), 2);
    assert!(s.state().selected_take_lanes.is_empty());
}

// Spec: comping R6, R28. Rename with a header selected types the name in
// place: without a name it asks the UI to, and with one it renames.
#[test]
fn rename_on_a_take_lane_header_types_in_place() {
    let mut s = song();
    let one = s.lane(2, 1, 0, 2);
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    assert_eq!(s.shell.menu_command("clip.rename"), "take_lane.rename");
    assert!(!s.shell.can_invoke("clip.rename", &json!({})));
    assert!(s.shell.can_invoke("take_lane.rename", &json!({})));
    s.shell.invoke("take_lane.rename", json!({})).unwrap();
    let effects = s.shell.take_effects();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, bl_shell::Effect::RenameTakeLane(lane) if *lane == one)),
        "{effects:?}"
    );
    s.one_step("take_lane.rename", json!({ "lane": one, "name": "Verse" }));
}

// Spec: track-context-menu R10. Delete All Take Lanes and Delete All Unused
// Take Lanes act on the selected tracks when no track is named.
#[test]
fn the_track_menus_deletes_act_on_the_selected_tracks() {
    let mut s = song();
    let track = s.tracks[2];
    s.lane(2, 1, 0, 2);
    s.lane(2, 2, 0, 2);
    s.view(
        "arrangement.select_tracks",
        json!({ "tracks": [s.tracks[3]] }),
    );
    assert!(!s.shell.can_invoke("take_lane.delete_all", &json!({})));
    s.view("arrangement.select_tracks", json!({ "tracks": [track] }));
    assert!(s.shell.can_invoke("take_lane.delete_all", &json!({})));
    assert!(s.shell.can_invoke("take_lane.delete_unused", &json!({})));
    s.shell
        .invoke("take_lane.delete_unused", json!({}))
        .unwrap();
    assert_eq!(s.doc().track(track).unwrap().take_lanes().len(), 0);
    assert!(!s.shell.can_invoke("take_lane.delete_all", &json!({})));
}

// Spec: comping R27. A time selection on a take lane is deleted, duplicated,
// copied, cut and pasted on that lane, each placing and clearing there alone.
#[test]
fn the_edits_act_on_time_selected_on_a_take_lane() {
    let mut s = song();
    let track = s.tracks[2];
    let lane = s.lane(2, 1, 0, 4);
    s.main_clip(2, 9, 0, 8);
    let on = json!([{ "type": "take", "track": track, "lane": lane }]);
    // Delete.
    s.select_on(on.clone(), 1, 2);
    s.one_step("arrangement.delete", json!({}));
    assert_eq!(s.on_lane(lane), [(0, 1), (2, 4)]);
    assert_eq!(s.main(2), [(0, 8)], "the main lane was left alone");
    // Duplicate: a copy right after, on the lane.
    s.select_on(on.clone(), 2, 3);
    s.one_step("arrangement.duplicate", json!({}));
    assert_eq!(s.on_lane(lane), [(0, 1), (2, 3), (3, 4)]);
    assert_eq!(s.main(2), [(0, 8)]);
    assert_eq!(s.shell.arrangement().selection_take_lanes(), [lane]);
    // Copy and paste further along, on the same lane.
    s.select_on(on.clone(), 0, 1);
    s.shell.run("arrangement.copy", json!({})).unwrap();
    s.view(
        "arrangement.set_insert_marker",
        json!({ "at": 6 * BAR, "lane": lane }),
    );
    s.one_step("arrangement.paste", json!({}));
    assert_eq!(s.on_lane(lane), [(0, 1), (2, 3), (3, 4), (6, 7)]);
    assert_eq!(s.main(2), [(0, 8)]);
    // Cut.
    s.select_on(on, 6, 7);
    s.one_step("arrangement.cut", json!({}));
    assert_eq!(s.on_lane(lane), [(0, 1), (2, 3), (3, 4)]);
    // Paste with the main lane selected puts it on the main lane.
    s.view(
        "arrangement.set_insert_marker",
        json!({ "at": 5 * BAR, "track": track }),
    );
    s.one_step("arrangement.paste", json!({}));
    assert_eq!(s.main(2), [(0, 5), (5, 6), (6, 8)]);
}

// Spec: comping R27. Split acts on the clips of the lanes the time selection
// covers, and Delete by clip ID on a take lane's clip.
#[test]
fn split_and_delete_reach_take_lane_clips() {
    let mut s = song();
    let track = s.tracks[2];
    let lane = s.lane(2, 1, 0, 4);
    s.main_clip(2, 9, 0, 4);
    s.select_on(
        json!([{ "type": "take", "track": track, "lane": lane }]),
        2,
        3,
    );
    s.one_step("arrangement.split", json!({}));
    assert_eq!(s.on_lane(lane), [(0, 2), (2, 3), (3, 4)]);
    assert_eq!(s.main(2), [(0, 4)]);
    let clip = s.doc().take_lane(lane).unwrap().clips[0].id;
    s.view("arrangement.select_clips", json!({ "clips": [clip] }));
    s.one_step("arrangement.delete", json!({}));
    assert_eq!(s.on_lane(lane).len(), 2);
}

// Spec: comping R27. Clips move between lanes and tracks, and a take lane's
// clip moved to the main lane is moved, not copied; one undo step.
#[test]
fn clips_move_and_copy_between_lanes_and_tracks() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two) = (s.lane(2, 1, 0, 2), s.lane(2, 2, 4, 2));
    let clip = s.doc().take_lane(one).unwrap().clips[0].id;
    // To another take lane of the track.
    s.one_step(
        "arrangement.move_clips",
        json!({ "clips": [clip], "lane": { "type": "take", "track": track, "lane": two } }),
    );
    assert_eq!(s.doc().clip_lane(clip), Some(two));
    assert_eq!(s.on_lane(one), []);
    // To the main lane: moved, not copied.
    s.one_step(
        "arrangement.move_clips",
        json!({ "clips": [clip], "lane": { "type": "track", "track": track } }),
    );
    assert_eq!(s.doc().clip_lane(clip), None);
    assert_eq!(s.main(2), [(0, 2)]);
    assert_eq!(s.on_lane(two), [(4, 6)]);
    // A copy to a take lane of the next track, which has none, is refused;
    // with the lane there, it lands on it.
    let b1 = s.lane(3, 3, 8, 1);
    let before = s.doc().clone();
    s.shell
        .run(
            "arrangement.move_clips",
            json!({ "clips": [clip], "lane": { "type": "take", "track": s.tracks[3], "lane": b1 }, "copy": true }),
        )
        .unwrap();
    let copies = s.doc().take_lane(b1).unwrap().clips.len();
    assert_eq!(
        copies, 2,
        "a copy sits on the lane beside the clip that was there"
    );
    s.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(*s.doc(), before);
    // Moving onto a take lane of the wrong kind is refused.
    let midi_lane: TakeLaneId = s.shell.new_id();
    s.shell
        .run(
            "track.add_take_lane",
            json!({ "track": s.tracks[0], "lane": { "id": midi_lane, "name": "Take 1", "clips": [] } }),
        )
        .unwrap();
    let error = s
        .shell
        .run(
            "arrangement.move_clips",
            json!({ "clips": [clip], "lane": { "type": "take", "track": s.tracks[0], "lane": midi_lane } }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("another kind"), "{error}");
}

// Spec: comping R27, arrangement editing R13. A drag of a clip between lanes
// is one undo step, and Esc during it cancels it.
#[test]
fn a_drag_between_lanes_is_one_step_and_escape_cancels_it() {
    let mut s = song();
    let track = s.tracks[2];
    let (one, two) = (s.lane(2, 1, 0, 2), s.lane(2, 2, 4, 2));
    let clip = s.doc().take_lane(one).unwrap().clips[0].id;
    let before = s.doc().clone();
    let steps = s.steps();
    let frame = |by: i64| {
        Invocation::new(
            "arrangement.move_clips",
            json!({
                "clips": [clip], "by": by,
                "lane": { "type": "take", "track": track, "lane": two },
            }),
        )
    };
    for by in [BAR / 2, BAR] {
        s.shell
            .gesture(GesturePhase::Update, Some(&frame(by)))
            .unwrap();
    }
    assert_eq!(s.doc().clip_lane(clip), Some(two));
    s.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(*s.doc(), before, "Esc put it back");
    assert_eq!(s.steps(), steps);
    for by in [BAR / 2, BAR] {
        s.shell
            .gesture(GesturePhase::Update, Some(&frame(by)))
            .unwrap();
    }
    s.shell
        .gesture(GesturePhase::Commit, Some(&frame(BAR)))
        .unwrap();
    assert_eq!(s.steps(), steps + 1, "the whole drag is one step");
    assert_eq!(s.doc().clip_lane(clip), Some(two));
}

// Spec: audio clip editing R29. Consolidate makes one clip over the time
// selected on a take lane, on that lane, replacing what it covers there.
#[test]
fn consolidate_works_on_a_take_lane() {
    let mut s = song();
    let track = s.tracks[0];
    let lane: TakeLaneId = s.shell.new_id();
    s.shell
        .run(
            "track.add_take_lane",
            json!({ "track": track, "lane": { "id": lane, "name": "Take 1", "clips": [] } }),
        )
        .unwrap();
    for (start, note) in [(0, 60u8), (BAR, 62)] {
        let clip = Clip::new(
            s.shell.new_id(),
            Ticks(start),
            Ticks(BAR),
            ClipData::Midi(MidiData {
                notes: vec![Note::new(note, Ticks::ZERO, Ticks::QUARTER, 100)].into(),
                ..MidiData::default()
            }),
        );
        s.shell
            .run(
                "take_lane.create_clip",
                json!({ "lane": lane, "clip": clip }),
            )
            .unwrap();
    }
    s.select_on(
        json!([{ "type": "take", "track": track, "lane": lane }]),
        0,
        2,
    );
    s.one_step("arrangement.consolidate", json!({}));
    assert_eq!(s.on_lane(lane), [(0, 2)], "one clip, on the lane");
    let clip = &s.doc().take_lane(lane).unwrap().clips[0];
    let ClipData::Midi(midi) = &clip.data else {
        panic!("a MIDI clip");
    };
    assert_eq!(midi.notes.len(), 2);
    assert!(s.main(0).is_empty(), "the main lane has none");
}

// Spec: comping R27. Crop acts on the clip a time selection on a take lane
// covers.
#[test]
fn crop_works_on_a_take_lane_clip() {
    let mut s = song();
    let track = s.tracks[0];
    let lane: TakeLaneId = s.shell.new_id();
    s.shell
        .run(
            "track.add_take_lane",
            json!({ "track": track, "lane": { "id": lane, "name": "Take 1", "clips": [] } }),
        )
        .unwrap();
    let clip = Clip::new(
        s.shell.new_id(),
        Ticks::ZERO,
        Ticks(4 * BAR),
        ClipData::Midi(MidiData {
            notes: vec![
                Note::new(60, Ticks::ZERO, Ticks::QUARTER, 100),
                Note::new(62, Ticks(3 * BAR), Ticks::QUARTER, 100),
            ]
            .into(),
            ..MidiData::default()
        }),
    );
    let id = clip.id;
    s.shell
        .run(
            "take_lane.create_clip",
            json!({ "lane": lane, "clip": clip }),
        )
        .unwrap();
    s.select_on(
        json!([{ "type": "take", "track": track, "lane": lane }]),
        0,
        2,
    );
    s.one_step("clip.crop", json!({}));
    let cropped = s.doc().clip(id).unwrap();
    assert_eq!(cropped.end(), Ticks(2 * BAR));
}

// Spec: comping R26, R28. The view forgets a take lane the document no longer
// has: its header selection and its height.
#[test]
fn view_state_follows_deleted_lanes() {
    let mut s = song();
    let track = s.tracks[2];
    let one = s.lane(2, 1, 0, 2);
    s.view("arrangement.select_take_lanes", json!({ "lanes": [one] }));
    s.view("arrangement.set_track_height", json!({ "delta": 10 }));
    s.shell
        .run("take_lane.delete", json!({ "lanes": [one] }))
        .unwrap();
    assert!(s.state().selected_take_lanes.is_empty());
    assert_eq!(s.shell.arrangement().take_lane_height(one), 44.0);
    let _ = track;
}
