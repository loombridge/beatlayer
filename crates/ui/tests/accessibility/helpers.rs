//! The set, key and tree helpers the accessibility tests share.

use crate::common::{Harness, Window};
use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, TrackId};
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use egui::accesskit::{self};
use egui::{FullOutput, Id, Key, Modifiers};
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) const BAR: i64 = Ticks::WHOLE.get();

/// The factory set with a clip called "Kick" on 3 Audio over bars 3 to 7,
/// drawn with Live's title bar at the reference size.
pub(crate) fn set() -> (Harness, Vec<TrackId>, ClipId) {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let id: ClipId = editor.new_id();
    let mut clip = Clip::new(
        id,
        Ticks(2 * BAR),
        Ticks(4 * BAR),
        ClipData::Audio(AudioData::new(benchmark::media_hash(0))),
    );
    clip.name = "Kick".to_owned();
    editor
        .apply(Command::CreateClip {
            track: tracks[2],
            clip,
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    (h, tracks, id)
}

/// The accessibility tree's nodes, by ID.
pub(crate) fn tree(output: &FullOutput) -> BTreeMap<accesskit::NodeId, accesskit::Node> {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("an AccessKit tree update")
        .nodes
        .iter()
        .cloned()
        .collect()
}

/// The node a screen reader reads as focused: the tree's focus, or its
/// active descendant (R2).
pub(crate) fn focused(output: &FullOutput) -> Option<(accesskit::NodeId, accesskit::Node)> {
    let update = output.platform_output.accesskit_update.as_ref()?;
    let nodes = tree(output);
    let focus = update.focus;
    let node = nodes.get(&focus)?;
    let id = node.active_descendant().unwrap_or(focus);
    nodes.get(&id).map(|n| (id, n.clone()))
}

/// The label of what a screen reader reads as focused.
pub(crate) fn focused_label(output: &FullOutput) -> String {
    // A text field without a label is read by its placeholder.
    focused(output)
        .and_then(|(_, n)| n.label().or_else(|| n.placeholder()).map(str::to_owned))
        .unwrap_or_default()
}

pub(crate) fn press(h: &mut Harness, key: Key, modifiers: Modifiers) -> FullOutput {
    h.key(key, modifiers);
    h.settle()
}

/// Option on macOS, Ctrl elsewhere: the neighbour keys' modifier
/// (S-FOCUS-013).
pub(crate) fn neighbour() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    }
}

pub(crate) fn focused_id(h: &Harness) -> Option<Id> {
    h.ctx.memory(egui::Memory::focused)
}

pub(crate) fn tab_navigation_on(h: &mut Harness) {
    if !h.shell.tab_navigation() {
        h.shell
            .run("focus.toggle_tab_navigation", json!({}))
            .unwrap();
    }
}
