//! The Arrangement's several automation lanes per track, driven headless
//! through `Shell::run` (WP-6.57): adding lanes and moving the main lane on,
//! removing them, folding them with the track's toggle, the choosers' filter,
//! the view state `arrangement.state` reports, and the lanes saved with the
//! set. The behaviour is in the automation spec, R24 to R27, R32 and R33.

use bl_core::time::Ticks;
use bl_model::{AutomationTarget, MixerParam, TrackId};
use bl_shell::Shell;
use bl_shell::arrangement::rows::AUTOMATION_LANE_HEIGHT;
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState};
use serde_json::{Value, json};

fn mixer(param: MixerParam) -> AutomationTarget {
    AutomationTarget::Mixer { param }
}

fn volume() -> AutomationTarget {
    mixer(MixerParam::Volume)
}

fn pan() -> AutomationTarget {
    mixer(MixerParam::Pan)
}

fn mute() -> AutomationTarget {
    mixer(MixerParam::Mute)
}

/// A headless shell with Automation Mode on, and the first track.
struct Song {
    shell: Shell,
    track: TrackId,
}

fn song() -> Song {
    let mut shell = Shell::headless();
    let track = shell.editor().document().top_level().next().unwrap();
    shell
        .run(view_ids::AUTOMATION_MODE, json!({ "on": true }))
        .unwrap();
    Song { shell, track }
}

impl Song {
    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    /// Gives the track an envelope for `target`.
    fn envelope(&mut self, target: AutomationTarget) {
        self.run(
            "automation.set_breakpoint",
            json!({ "track": self.track, "target": target, "at": Ticks(0), "value": 0.5 }),
        );
    }

    fn add(&mut self, args: Value) -> ArrangementState {
        let mut args = args;
        args["track"] = json!(self.track);
        serde_json::from_value(self.run(view_ids::ADD_AUTOMATION_LANE, args)).unwrap()
    }

    /// The track's lanes: what its main lane shows, and what its own lanes
    /// show.
    fn lanes(&self) -> (Option<AutomationTarget>, Vec<Option<AutomationTarget>>) {
        let state = self
            .shell
            .arrangement()
            .state(self.shell.editor().document());
        match state
            .automation_lanes
            .iter()
            .find(|lane| lane.track == self.track)
        {
            Some(lane) => (lane.target, lane.lanes.iter().map(|l| l.target).collect()),
            None => (Some(volume()), Vec::new()),
        }
    }

    fn own_height(&self) -> f32 {
        let doc = self.shell.editor().document();
        let rows = self.shell.arrangement().rows(doc);
        rows.iter()
            .find(|r| r.track == self.track)
            .unwrap()
            .automation
    }

    fn steps(&self) -> usize {
        self.shell.editor().history().len()
    }
}

// Spec: automation R25. A new track has its main lane only, showing volume.
#[test]
fn a_new_track_has_its_main_lane_only_showing_its_volume() {
    let song = song();
    assert_eq!(song.lanes(), (Some(volume()), Vec::new()));
    assert_eq!(song.own_height(), 0.0);
}

// Spec: automation R25, R32. The lane button moves the envelope the main lane
// shows into a lane of its own, and the main lane takes the next parameter in
// the choosers' order that has an envelope and no lane, or None.
#[test]
fn adding_a_lane_moves_the_envelope_into_it_and_the_main_lane_moves_on() {
    let mut song = song();
    song.envelope(volume());
    song.envelope(pan());
    song.add(json!({}));
    assert_eq!(song.lanes(), (Some(pan()), vec![Some(volume())]));
    assert_eq!(song.own_height(), AUTOMATION_LANE_HEIGHT);
    song.add(json!({}));
    assert_eq!(song.lanes(), (None, vec![Some(volume()), Some(pan())]));
    assert_eq!(song.own_height(), 2.0 * AUTOMATION_LANE_HEIGHT);
}

// Spec: automation R25. The button does nothing for None, and a parameter
// that has a lane already.
#[test]
fn the_lane_button_refuses_none_and_a_parameter_that_has_a_lane() {
    let mut song = song();
    song.envelope(volume());
    song.add(json!({}));
    assert!(
        song.shell
            .run(
                view_ids::ADD_AUTOMATION_LANE,
                json!({ "track": song.track })
            )
            .is_err()
    );
    song.add(json!({ "target": volume() }));
    assert_eq!(song.lanes().1, vec![Some(volume())]);
}

// Spec: automation R29. Show Automation In New Lane selects the lane the
// parameter has instead of adding another.
#[test]
fn showing_a_parameter_in_a_new_lane_selects_the_lane_it_has() {
    let mut song = song();
    song.envelope(volume());
    song.envelope(pan());
    song.add(json!({ "target": pan() }));
    assert_eq!(song.lanes(), (Some(volume()), vec![Some(pan())]));
    song.add(json!({ "target": pan() }));
    assert_eq!(song.lanes().1, vec![Some(pan())]);
    assert_eq!(
        song.shell.arrangement().keyboard_lane_number(),
        Some((song.track, 1))
    );
}

// Spec: automation R25. With Alt or Cmd held, the button gives every
// automated parameter that has no lane one, in the choosers' order.
#[test]
fn the_modifier_gives_every_automated_parameter_a_lane_in_the_choosers_order() {
    let mut song = song();
    // Made out of order: the choosers list volume, pan, then the Track
    // Activator.
    song.envelope(mute());
    song.envelope(volume());
    song.envelope(pan());
    song.add(json!({ "all": true }));
    assert_eq!(
        song.lanes(),
        (None, vec![Some(volume()), Some(pan()), Some(mute())])
    );
}

// Spec: automation R25. The main lane stays on a parameter with no envelope
// when only other parameters go to lanes of their own.
#[test]
fn the_modifier_leaves_a_main_lane_without_an_envelope_where_it_is() {
    let mut song = song();
    song.envelope(pan());
    song.add(json!({ "all": true }));
    assert_eq!(song.lanes(), (Some(volume()), vec![Some(pan())]));
}

// Spec: automation R26. The hide button removes the lane from view, and with
// Alt or Cmd the lanes after it too; the envelopes keep driving.
#[test]
fn removing_a_lane_takes_it_out_of_view_and_with_the_modifier_the_lanes_after_it() {
    let mut song = song();
    song.envelope(volume());
    song.envelope(pan());
    song.envelope(mute());
    song.add(json!({ "all": true }));
    song.run(
        view_ids::REMOVE_AUTOMATION_LANE,
        json!({ "track": song.track, "lane": 2 }),
    );
    assert_eq!(song.lanes().1, vec![Some(volume()), Some(mute())]);
    song.run(
        view_ids::REMOVE_AUTOMATION_LANE,
        json!({ "track": song.track, "lane": 1, "and_after": true }),
    );
    assert_eq!(song.lanes().1, Vec::new());
    assert_eq!(song.own_height(), 0.0);
    // The envelopes are still the track's.
    let doc = song.shell.editor().document();
    let track = doc.track(song.track).unwrap();
    assert!(track.automation_for(&volume()).is_some());
    assert!(track.automation_for(&mute()).is_some());
    for lane in [0, 1] {
        assert!(
            song.shell
                .run(
                    view_ids::REMOVE_AUTOMATION_LANE,
                    json!({ "track": song.track, "lane": lane })
                )
                .is_err()
        );
    }
}

// Spec: automation R26. The track's toggle hides all its lanes of its own and
// shows them again; the lanes are kept.
#[test]
fn the_tracks_toggle_hides_and_shows_its_lanes() {
    let mut song = song();
    song.envelope(volume());
    song.add(json!({}));
    song.run(
        view_ids::SHOW_AUTOMATION_LANES,
        json!({ "tracks": [song.track] }),
    );
    assert_eq!(song.own_height(), 0.0);
    assert_eq!(song.lanes().1, vec![Some(volume())]);
    song.run(
        view_ids::SHOW_AUTOMATION_LANES,
        json!({ "tracks": [song.track], "on": true }),
    );
    assert_eq!(song.own_height(), AUTOMATION_LANE_HEIGHT);
    // Adding a lane shows the track's lanes again.
    song.run(
        view_ids::SHOW_AUTOMATION_LANES,
        json!({ "tracks": [song.track], "on": false }),
    );
    song.envelope(pan());
    song.add(json!({ "target": pan() }));
    assert_eq!(song.own_height(), 2.0 * AUTOMATION_LANE_HEIGHT);
}

// Spec: automation R26. A folded track, and Automation Mode off, draw no lane
// of their own.
#[test]
fn a_folded_track_and_automation_mode_off_draw_no_lanes_of_their_own() {
    let mut song = song();
    song.envelope(volume());
    song.add(json!({}));
    song.run(view_ids::AUTOMATION_MODE, json!({ "on": false }));
    assert_eq!(song.own_height(), 0.0);
    song.run(view_ids::AUTOMATION_MODE, json!({ "on": true }));
    song.run(
        view_ids::FOLD_TRACKS,
        json!({ "tracks": [song.track], "fold": true }),
    );
    assert_eq!(song.own_height(), 0.0);
}

// Spec: automation R14, R32. The choosers pick a lane's parameter: the main
// lane's, or a lane of its own by its number.
#[test]
fn the_choosers_pick_the_parameter_of_a_lane_by_its_number() {
    let mut song = song();
    song.envelope(volume());
    song.add(json!({}));
    song.run(
        view_ids::SHOW_AUTOMATION,
        json!({ "track": song.track, "target": pan(), "lane": 1 }),
    );
    song.run(
        view_ids::SHOW_AUTOMATION,
        json!({ "track": song.track, "target": null }),
    );
    assert_eq!(song.lanes(), (None, vec![Some(pan())]));
    assert!(
        song.shell
            .run(
                view_ids::SHOW_AUTOMATION,
                json!({ "track": song.track, "target": pan(), "lane": 5 })
            )
            .is_err()
    );
}

// Spec: automation R27, R32. Show Automated Parameters Only is per lane.
#[test]
fn the_filter_belongs_to_each_lane() {
    let mut song = song();
    song.envelope(volume());
    song.add(json!({}));
    song.run(
        view_ids::SHOW_AUTOMATED_ONLY,
        json!({ "track": song.track, "lane": 1 }),
    );
    let state: ArrangementState = serde_json::from_value(song.run(
        view_ids::SHOW_AUTOMATED_ONLY,
        json!({ "track": song.track, "on": true }),
    ))
    .unwrap();
    let lane = &state.automation_lanes[0];
    assert!(lane.only_automated);
    assert!(lane.lanes[0].only_automated);
    let state: ArrangementState = serde_json::from_value(song.run(
        view_ids::SHOW_AUTOMATED_ONLY,
        json!({ "track": song.track, "lane": 1 }),
    ))
    .unwrap();
    assert!(!state.automation_lanes[0].lanes[0].only_automated);
}

// Spec: automation R26, R32. None of the lane commands is an undo step.
#[test]
fn the_lane_commands_change_no_undo_step() {
    let mut song = song();
    song.envelope(volume());
    song.envelope(pan());
    let before = song.steps();
    song.add(json!({}));
    song.run(
        view_ids::SHOW_AUTOMATED_ONLY,
        json!({ "track": song.track, "on": true }),
    );
    song.run(
        view_ids::SHOW_AUTOMATION_LANES,
        json!({ "tracks": [song.track], "on": false }),
    );
    song.run(
        view_ids::REMOVE_AUTOMATION_LANE,
        json!({ "track": song.track, "lane": 1 }),
    );
    assert_eq!(song.steps(), before);
}

// Spec: automation R26. A selected lane that is removed gives up the keyboard,
// and the ones after it move up a number.
#[test]
fn removing_a_lane_moves_the_selected_lane_with_its_number() {
    let mut song = song();
    song.envelope(volume());
    song.envelope(pan());
    song.envelope(mute());
    song.add(json!({ "all": true }));
    let select = |song: &mut Song, lane: u32| {
        song.run(
            view_ids::SELECT_TRACKS,
            json!({ "tracks": [song.track], "automation_lane": lane }),
        );
    };
    select(&mut song, 3);
    song.run(
        view_ids::REMOVE_AUTOMATION_LANE,
        json!({ "track": song.track, "lane": 1 }),
    );
    assert_eq!(
        song.shell.arrangement().keyboard_lane(),
        Some((song.track, mute()))
    );
    select(&mut song, 1);
    song.run(
        view_ids::REMOVE_AUTOMATION_LANE,
        json!({ "track": song.track, "lane": 1 }),
    );
    assert_eq!(song.shell.arrangement().keyboard_lane(), None);
}

// Spec: automation R26, R33. The lanes save with the set and open again; a
// track with only the main lane showing its volume has nothing to save.
#[test]
fn the_lanes_are_saved_with_the_set_and_open_again() {
    let mut song = song();
    song.envelope(volume());
    song.envelope(pan());
    song.add(json!({}));
    song.run(
        view_ids::SHOW_AUTOMATED_ONLY,
        json!({ "track": song.track, "lane": 1, "on": true }),
    );
    let dir = std::env::temp_dir().join(format!("bl-lanes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let saved: bl_shell::commands::SetResult = serde_json::from_value(song.run(
        "set.save_as",
        json!({ "path": dir.join("Lanes.blset").to_string_lossy() }),
    ))
    .unwrap();
    let before = song.shell.arrangement().lane_views().clone();
    assert!(!before.is_empty());
    let mut other = Shell::headless();
    other
        .run("set.open", json!({ "path": saved.path.unwrap() }))
        .unwrap();
    assert_eq!(*other.arrangement().lane_views(), before);
    // A new set forgets them.
    other.run("set.new", json!({})).unwrap();
    assert!(other.arrangement().lane_views().is_empty());
}
