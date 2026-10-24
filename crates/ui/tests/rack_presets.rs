//! Rack and chain presets by drag and drop, headless (WP-5.8; the racks
//! spec, R43 and R46): a chain's row dragged onto a folder in the browser is
//! saved there, and presets and devices dragged from the browser land in the
//! Device View. Each gesture runs the registered command an agent runs.

use crate::common;
use std::path::PathBuf;

use bl_engine::testing;
use bl_model::{ChainId, DeviceId, TrackId};
use bl_shell::browser::places::create_user_library;
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_view::layout;
use bl_ui::rack_view;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

/// The list's first row's top, in window coordinates (the browser's tests'): under the Filter View, which shows in a new install.
const LIST_TOP: f32 = 227.0;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-ui-rack-presets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The middle of the browser list's row `index`.
fn row(index: usize) -> Pos2 {
    pos2(160.0, LIST_TOP + 17.0 * index as f32 + 8.5)
}

/// A harness on the default set's first audio track, with a User Library.
fn harness(scratch: &Scratch) -> (Harness, PathBuf) {
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_user_library(Some(root.clone()));
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    (h, root)
}

fn gain(name: &str) -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": name, "role": t.role })
}

fn add(h: &mut Harness, args: Value) -> DeviceId {
    let added: DeviceAdded =
        serde_json::from_value(h.shell.run("device.add", args).unwrap()).unwrap();
    h.settle();
    added.device
}

fn add_chain(h: &mut Harness, rack: DeviceId, name: &str) -> ChainId {
    let id: ChainId = h.shell.new_id();
    let end = h
        .shell
        .editor()
        .document()
        .device(rack)
        .unwrap()
        .chains
        .len();
    h.shell
        .invoke(
            "chain.add",
            json!({ "rack": rack, "index": end, "chain": { "id": id, "name": name, "devices": [] } }),
        )
        .unwrap();
    id
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A press at `from`, six steps, and a release at `to`.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![press(to, false)]);
    h.settle();
}

fn view(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// An audio effect rack on the shown track holding a gain in a chain.
fn rack_with_chain(h: &mut Harness) -> (DeviceId, ChainId) {
    let rack = add(
        h,
        json!({ "device": {
            "kind": { "rack": { "kind": "audio_effect" } }, "name": "Rack", "role": "audio_effect",
        } }),
    );
    let chain = add_chain(h, rack, "Wide");
    add(h, json!({ "device": gain("Gain"), "chain": chain }));
    h.settle();
    (rack, chain)
}

// Spec: racks R46
#[test]
fn a_chain_row_dragged_onto_a_browser_folder_is_saved_there() {
    let scratch = Scratch::new("save");
    let (mut h, root) = harness(&scratch);
    let (rack, chain) = rack_with_chain(&mut h);
    h.shell
        .run("browser.open_place", json!({ "label": "user_library" }))
        .unwrap();
    h.settle();
    let view_rect = view(&mut h);
    let layouts = rack_view::layouts(&h.shell, view_rect);
    let list = layouts
        .iter()
        .find(|(id, _)| *id == rack)
        .and_then(|(_, l)| l.chain_list.clone())
        .expect("the chain list shows");
    let names: Vec<String> = {
        let browser = h.shell.browser_view();
        (0..browser.row_count())
            .map(|i| browser.row(i).unwrap().name)
            .collect()
    };
    let presets = names.iter().position(|n| n == "Presets").expect("Presets");
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, list.rows[0].row.center(), row(presets));
    let saved = root.join("Presets").join("Wide.blpreset");
    assert!(saved.is_file(), "{saved:?}");
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps,
        "saving a file is no undo step"
    );
    // The same drag again numbers the name.
    drag(&mut h, list.rows[0].row.center(), row(presets));
    assert!(root.join("Presets").join("Wide 2.blpreset").is_file());
    let _ = chain;
}

// Spec: racks R43, R46
#[test]
fn a_rack_from_the_browser_lands_as_a_chain_or_on_the_track() {
    let scratch = Scratch::new("drop");
    let (mut h, _) = harness(&scratch);
    let (rack, _) = rack_with_chain(&mut h);
    h.shell
        .run("browser.open_place", json!({ "label": "audio_effects" }))
        .unwrap();
    h.settle();
    let view_rect = view(&mut h);
    let drop = rack_view::layouts(&h.shell, view_rect)
        .into_iter()
        .find(|(id, _)| *id == rack)
        .and_then(|(_, l)| l.chain_list)
        .expect("the chain list shows")
        .drop_area;

    // The first row is the empty audio effect rack: dropped on the chain
    // list's drop area it makes a chain holding the rack.
    let before = h
        .shell
        .editor()
        .document()
        .device(rack)
        .unwrap()
        .chains
        .len();
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, row(0), drop.center());
    let doc = h.shell.editor().document();
    let chains = &doc.device(rack).unwrap().chains;
    assert_eq!(chains.len(), before + 1);
    let nested = &chains[before].devices[0];
    assert_eq!(nested.rack_kind(), Some(bl_model::RackKind::AudioEffect));
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);

    // Dropped on the Device View outside any rack, it is added to the track.
    let frame = layout(&h.shell, view_rect)
        .into_iter()
        .find(|(id, _)| *id == rack)
        .unwrap()
        .1;
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    let devices = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .devices()
            .count()
    };
    let count = devices(&h);
    let blank = pos2(frame.right() + 40.0, frame.center().y);
    assert!(view_rect.contains(blank), "{blank:?} in {view_rect:?}");
    drag(&mut h, row(0), blank);
    assert_eq!(devices(&h), count + 1);
}
