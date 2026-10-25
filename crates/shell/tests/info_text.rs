//! The user's own info text, headless (WP-6.58; the Info View spec, R9 to
//! R12): the four commands set and read back an item's text as one undo
//! step, and Edit Info Text opens the Info View's field for the selection.

use bl_model::{ChainId, ClipId, DeviceId, SceneId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::{Area, Effect, InfoTarget, MainView, Shell};
use serde_json::json;

struct Set {
    shell: Shell,
    track: TrackId,
    clip: ClipId,
    scene: SceneId,
    device: DeviceId,
    chain: ChainId,
}

fn set() -> Set {
    let mut shell = Shell::headless();
    let tracks: Vec<TrackId> = shell.editor().document().top_level().collect();
    let (track, audio) = (tracks[0], tracks[2]);
    let clip = bl_model::Clip::new(
        shell.new_id(),
        bl_core::time::Ticks::ZERO,
        bl_core::time::Ticks::WHOLE,
        bl_model::ClipData::Midi(bl_model::MidiData::default()),
    );
    let clip_id = clip.id;
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    shell.set_focused_area(Area::Detail);
    let rack: DeviceAdded = serde_json::from_value(
        shell
            .invoke(
                "device.add",
                json!({
                    "device": {
                        "kind": { "rack": { "kind": "audio_effect" } },
                        "name": "Rack",
                        "role": "audio_effect",
                    },
                }),
            )
            .unwrap(),
    )
    .unwrap();
    let chain = shell.new_id::<ChainId>();
    shell
        .invoke(
            "chain.add",
            json!({ "rack": rack.device, "index": 0, "chain": { "id": chain, "name": "Range", "devices": [] } }),
        )
        .unwrap();
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    Set {
        shell,
        track,
        clip: clip_id,
        scene,
        device: rack.device,
        chain,
    }
}

fn undo_label(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: info-view R9, R12. Each item's text is set by ID, read back, and is
// one undo step, `Change Info Text`.
#[test]
fn each_commands_text_is_one_undo_step_and_reads_back() {
    let mut set = set();
    let doc_text = |set: &Set| {
        let doc = set.shell.editor().document();
        [
            doc.track(set.track).unwrap().info_text().to_owned(),
            doc.clip(set.clip).unwrap().info_text.clone(),
            doc.scene(set.scene).unwrap().info_text.clone(),
            doc.device(set.device).unwrap().info_text.clone(),
        ]
    };
    assert_eq!(doc_text(&set), ["", "", "", ""]);
    let commands = [
        ("track.set_info_text", json!({ "track": set.track })),
        ("clip.set_info_text", json!({ "clip": set.clip })),
        ("scene.set_info_text", json!({ "scene": set.scene })),
        ("device.set_info_text", json!({ "device": set.device })),
    ];
    for (n, (id, mut args)) in commands.into_iter().enumerate() {
        args["info_text"] = json!(format!("note {n}"));
        set.shell.invoke(id, args).unwrap();
        assert_eq!(undo_label(&set.shell), "Change Info Text", "{id}");
    }
    assert_eq!(doc_text(&set), ["note 0", "note 1", "note 2", "note 3"]);
    for expected in [
        ["note 0", "note 1", "note 2", ""],
        ["note 0", "note 1", "", ""],
        ["note 0", "", "", ""],
        ["", "", "", ""],
    ] {
        set.shell.invoke("edit.undo", json!({})).unwrap();
        assert_eq!(doc_text(&set), expected, "one step each");
    }
}

// Spec: info-view R10. A command with no text opens the Info View's field
// for the item, showing a hidden Info View; the text is kept by the same
// command with it.
#[test]
fn a_command_with_no_text_opens_the_field_and_shows_a_hidden_info_view() {
    let mut set = set();
    set.shell
        .invoke("view.toggle_info_view", json!({}))
        .unwrap();
    assert!(!set.shell.info_view_visible());
    set.shell.take_effects();
    for (id, args, target) in [
        (
            "track.set_info_text",
            json!({ "track": set.track }),
            InfoTarget::Track(set.track),
        ),
        (
            "clip.set_info_text",
            json!({ "clip": set.clip }),
            InfoTarget::Clip(set.clip),
        ),
        (
            "scene.set_info_text",
            json!({ "scene": set.scene }),
            InfoTarget::Scene(set.scene),
        ),
        (
            "device.set_info_text",
            json!({ "device": set.device }),
            InfoTarget::Device(set.device),
        ),
        (
            "chain.set_info",
            json!({ "chain": set.chain }),
            InfoTarget::Chain(set.chain),
        ),
        (
            "rack.set_macro_info_text",
            json!({ "rack": set.device, "macro_number": 2 }),
            InfoTarget::Macro(set.device, 2),
        ),
    ] {
        set.shell.invoke(id, args).unwrap();
        assert!(set.shell.info_view_visible(), "{id} shows the Info View");
        assert!(
            matches!(set.shell.take_effects()[..], [Effect::EditInfoText(t)] if t == target),
            "{id}"
        );
        let kept = Shell::info_text_invocation(target, "kept");
        set.shell.invoke(&kept.id, kept.args.clone()).unwrap();
        assert_eq!(
            set.shell.info_text_of(target).as_deref(),
            Some("kept"),
            "{id}"
        );
    }
    let locator = {
        set.shell
            .invoke("locator.add", json!({ "at": "2.1.1" }))
            .unwrap();
        set.shell.editor().document().settings().locators[0].id
    };
    set.shell.take_effects();
    set.shell
        .invoke("locator.set_info_text", json!({ "locator": locator }))
        .unwrap();
    assert!(matches!(
        set.shell.take_effects()[..],
        [Effect::EditInfoText(InfoTarget::Locator(l))] if l == locator
    ));
}

// Spec: info-view R10. Edit ▸ Edit Info Text acts on the selection of the
// area with the focus, and is disabled while nothing that carries info text
// is selected.
#[test]
fn the_edit_menus_item_acts_on_the_selection_and_is_disabled_without_one() {
    let mut set = set();
    let id = "edit.edit_info_text";
    // The Device View has a rack selected: the rack is the device.
    set.shell
        .invoke("device.select", json!({ "device": set.device }))
        .unwrap();
    set.shell.take_effects();
    assert!(set.shell.can_invoke(id, &json!({})));
    set.shell.invoke(id, json!({})).unwrap();
    let effects = set.shell.take_effects();
    assert!(
        matches!(effects[..], [Effect::EditInfoText(InfoTarget::Device(d))] if d == set.device),
        "the Device View's selection: {effects:?}"
    );
    // The Arrangement with a title bar selected.
    set.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [set.track], "title_bars": true }),
        )
        .unwrap();
    set.shell.set_focused_area(Area::MainView);
    assert_eq!(set.shell.main_view(), MainView::Arrangement);
    set.shell.invoke(id, json!({})).unwrap();
    assert!(
        matches!(
            set.shell.take_effects()[..],
            [Effect::EditInfoText(InfoTarget::Track(t))] if t == set.track
        ),
        "the Arrangement's selected track"
    );
    // The Session View with a scene selected.
    set.shell
        .invoke("view.toggle_session_arrangement", json!({}))
        .unwrap();
    set.shell
        .invoke("session.select", json!({ "scene": set.scene }))
        .unwrap();
    set.shell.invoke(id, json!({})).unwrap();
    assert!(
        matches!(
            set.shell.take_effects()[..],
            [Effect::EditInfoText(InfoTarget::Scene(s))] if s == set.scene
        ),
        "the Session View's selected scene"
    );
    // Nothing selected: disabled, and refused.
    let mut empty = Shell::headless();
    empty.set_focused_area(Area::Detail);
    assert!(!empty.can_invoke(id, &json!({})));
    assert!(empty.invoke(id, json!({})).is_err());
}
