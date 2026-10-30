//! Link in the app, headless (WP-7.15, `docs/specs/link.md` R17 to R25): the
//! Control Bar's Link toggle with its peer count and menu, Settings' Link
//! page, Playback ▸ Options ▸ Link Start Stop Sync, and the quantum's
//! progress bar over the Arrangement Position fields. A second Link instance
//! in the test plays each peer over the loopback network, so these tests wait
//! for discovery with a bounded number of frames, and run one at a time with
//! `bl-link`'s (`.config/nextest.toml`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common;
use bl_link::Link;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_shell::settings::SettingsPage;
use bl_ui::keymap::Platform;
use bl_ui::menus::item_state;
use common::{Harness, Node, Window};
use egui::accesskit::{Action, Role};
use egui::{Id, Key, Modifiers, Rect, pos2};
use serde_json::json;

fn bounds(node: &Node) -> Rect {
    let b = node.bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn labelled(nodes: &[Node], label: &str) -> bool {
    nodes.iter().any(|node| node.label == label)
}

/// The Control Bar's node called `label`, or None.
fn bar_node(h: &mut Harness, label: &str) -> Option<Node> {
    Harness::nodes(&h.settle())
        .into_iter()
        .find(|node| node.label == label && node.bounds.is_some_and(|b| b.y1 < 38.0))
}

/// A peer in the session: a second Link instance, on.
fn peer() -> Link {
    let peer = Link::new(120.0);
    peer.enable(true, 120.0);
    peer
}

/// Runs frames, taking Link's news each time, until `done` holds. The peers
/// find each other on a real network, so it lets the threads run between
/// frames.
fn wait_for(h: &mut Harness, what: &str, done: impl Fn(&Harness) -> bool) {
    for _ in 0..1_000 {
        h.shell.poll();
        h.frame(vec![]);
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("waited for {what}");
}

/// A main window with the Link toggle shown, on a shell with a running
/// engine.
fn main_window() -> Harness {
    let mut h = Harness::manual(Window::Main);
    h.shell
        .run("link.set_show_toggle", json!({ "on": true }))
        .unwrap();
    h.settle();
    h
}

/// The Settings window on the Link page, on a shell with a running engine.
fn link_page(mut h: Harness) -> Harness {
    h.shell
        .invoke("app.settings", json!({ "page": SettingsPage::Link }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

fn preferences_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-ui-link-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn click(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    h.click(bounds(Harness::node(&nodes, label)).center());
}

/// The item of an open menu called `label`: not the toggle, which is a switch
/// of the same name.
fn menu_item(nodes: &[Node], label: &str) -> Node {
    nodes
        .iter()
        .find(|node| node.label == label && node.toggled.is_none())
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
        .clone()
}

fn open_menu(h: &mut Harness) -> Vec<Node> {
    click(h, "Link Options");
    Harness::nodes(&h.settle())
}

// Spec: link R17, R18, R23. A click switches Link on and off and the toggle
// lights; it reads Link while off and the number of other peers while on,
// as a switch whose value is what it reads.
#[test]
fn a_click_switches_link_and_the_toggle_reads_the_peers_as_they_come_and_go() {
    let mut h = main_window();
    let toggle = bar_node(&mut h, "Link").expect("shown on a new install's Control Bar");
    assert_eq!(toggle.role, Role::Button);
    assert_eq!(toggle.toggled, Some(false), "off at first");
    assert_eq!(toggle.value, "Link");

    click(&mut h, "Link");
    assert!(h.shell.link_enabled());
    let toggle = bar_node(&mut h, "Link").unwrap();
    assert_eq!(toggle.toggled, Some(true), "lit while on");
    assert_eq!(toggle.value, "0 Links");

    let first = peer();
    wait_for(&mut h, "one peer", |h| h.shell.link_peers() == 1);
    assert_eq!(bar_node(&mut h, "Link").unwrap().value, "1 Link");
    let second = peer();
    wait_for(&mut h, "two peers", |h| h.shell.link_peers() == 2);
    assert_eq!(bar_node(&mut h, "Link").unwrap().value, "2 Links");
    second.enable(false, 120.0);
    wait_for(&mut h, "one peer left", |h| h.shell.link_peers() == 1);
    assert_eq!(bar_node(&mut h, "Link").unwrap().value, "1 Link");
    first.enable(false, 120.0);
    wait_for(&mut h, "no peer", |h| h.shell.link_peers() == 0);
    assert_eq!(bar_node(&mut h, "Link").unwrap().value, "0 Links");

    click(&mut h, "Link");
    assert!(!h.shell.link_enabled());
    let toggle = bar_node(&mut h, "Link").unwrap();
    assert_eq!(toggle.toggled, Some(false));
    assert_eq!(toggle.value, "Link", "reads Link again");
}

// Spec: link R19, R23. The arrow beside the toggle opens a menu of Link,
// Start Stop Sync and Link Audio, then Configure Link…: the first two tick
// while on and run their commands, Link Audio waits for its package, and
// Configure Link… opens Settings on the Link page.
#[test]
fn the_menu_ticks_its_items_runs_their_commands_and_configures_link() {
    let mut h = main_window();
    let arrow = bar_node(&mut h, "Link Options").expect("the arrow is drawn");
    assert_eq!(arrow.role, Role::Button);
    let toggle = bar_node(&mut h, "Link").unwrap();
    assert!(
        bounds(&toggle).right() <= bounds(&arrow).left(),
        "beside the toggle"
    );

    let nodes = open_menu(&mut h);
    assert!(
        menu_item(&nodes, "Link Audio").disabled,
        "waits for Link Audio"
    );
    for label in ["Link", "Start Stop Sync", "Configure Link…"] {
        assert!(!menu_item(&nodes, label).disabled, "{label}");
    }
    h.click(bounds(&menu_item(&nodes, "Start Stop Sync")).center());
    assert!(h.shell.link_start_stop_sync());
    assert!(!h.shell.link_enabled(), "only the item's own command ran");
    let nodes = open_menu(&mut h);
    menu_item(&nodes, "✔ Start Stop Sync");
    h.click(bounds(&menu_item(&nodes, "Link")).center());
    assert!(h.shell.link_enabled());
    let nodes = open_menu(&mut h);
    menu_item(&nodes, "✔ Link");
    h.click(bounds(&menu_item(&nodes, "✔ Link")).center());
    assert!(!h.shell.link_enabled(), "the ticked item switches it off");

    let nodes = open_menu(&mut h);
    h.click(bounds(&menu_item(&nodes, "Configure Link…")).center());
    assert_eq!(h.shell.settings_page(), SettingsPage::Link);
}

// Spec: link R17, R20. Show Link Toggle hides the toggle and its arrow and
// shows them again, and is kept across a relaunch, in the real preference
// files; hiding it leaves Link as it is.
#[test]
fn show_link_toggle_hides_and_shows_the_toggle_and_is_kept_across_a_relaunch() {
    let dir = preferences_dir("show");
    let main = |dir: &Path| {
        let mut h = Harness::manual_in(Window::Main, dir);
        h.settle();
        h
    };
    let choose = |dir: &Path, key: Key| {
        let mut h = link_page(Harness::manual_in(Window::Settings, dir));
        let nodes = Harness::nodes(&h.settle());
        assert!(labelled(&nodes, "Show Link Toggle"));
        h.accesskit(Id::new(("chooser", "show-link-toggle")), Action::Focus);
        h.key(key, Modifiers::NONE);
        h.settle();
        h.shell.link_toggle_shown()
    };

    let mut h = main(&dir);
    let tap = bounds(&bar_node(&mut h, "Tap Tempo").unwrap()).left();
    assert!(bar_node(&mut h, "Link").is_some(), "shown at first");
    assert!(bar_node(&mut h, "Link Options").is_some());
    h.shell
        .run("link.set_enabled", json!({ "on": true }))
        .unwrap();
    drop(h);

    assert!(!choose(&dir, Key::ArrowDown), "Hide");
    let mut h = main(&dir);
    assert!(h.shell.link_enabled(), "hiding it leaves Link on");
    assert!(bar_node(&mut h, "Link").is_none(), "gone after a relaunch");
    assert!(bar_node(&mut h, "Link Options").is_none());
    let hidden = bounds(&bar_node(&mut h, "Tap Tempo").unwrap()).left();
    assert!(hidden < tap, "the tempo group is back in place");
    h.shell
        .run("link.set_enabled", json!({ "on": false }))
        .unwrap();
    drop(h);

    assert!(choose(&dir, Key::ArrowUp), "Show");
    let mut h = main(&dir);
    assert!(bar_node(&mut h, "Link").is_some(), "shown again");
    assert_eq!(bounds(&bar_node(&mut h, "Tap Tempo").unwrap()).left(), tap);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: link R20, R21. Start Stop Sync is a switch on the Link page and an
// item in Playback ▸ Options, and each ticks with the other.
#[test]
fn start_stop_sync_from_the_page_and_the_playback_menu_ticks_in_both() {
    let mut h = link_page(Harness::manual(Window::Settings));
    let menus = menu_bar(MenuPlatform::current());
    let playback = menus.iter().find(|m| m.title == "Playback").unwrap();
    let item = playback
        .walk()
        .find(|item| item.row == "M-PLAY-006d")
        .expect("Options lists Link Start Stop Sync");
    let state = |h: &Harness| item_state(item, &h.shell, &h.keymap, Platform::current());
    let switch =
        |h: &mut Harness| Harness::node(&Harness::nodes(&h.settle()), "Start Stop Sync").clone();

    let at_first = state(&h);
    assert!(at_first.enabled);
    assert_eq!(at_first.checked, Some(false), "off in a new install");
    assert_eq!(switch(&mut h).toggled, Some(false));

    click(&mut h, "Start Stop Sync");
    assert!(h.shell.link_start_stop_sync());
    assert_eq!(state(&h).checked, Some(true), "the menu ticks");
    assert_eq!(switch(&mut h).toggled, Some(true));

    let invocation = state(&h).invocation.unwrap();
    assert_eq!(invocation.id, "link.set_start_stop_sync");
    h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert!(!h.shell.link_start_stop_sync());
    assert_eq!(state(&h).checked, Some(false));
    assert_eq!(switch(&mut h).toggled, Some(false), "the page follows");
}

// Spec: link R20. While no audio engine runs the page shows only its
// message, and Link stays as it is set.
#[test]
fn the_page_shows_only_its_message_while_no_audio_engine_runs() {
    let mut h = Harness::new(Window::Settings);
    h.shell.audio_mut().stop();
    let mut h = link_page(h);
    h.shell
        .run("link.set_enabled", json!({ "on": true }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(labelled(
        &nodes,
        "Link needs the audio engine running. Switch it on in the Audio settings."
    ));
    assert!(!labelled(&nodes, "Show Link Toggle"));
    assert!(!labelled(&nodes, "Start Stop Sync"));
    assert!(h.shell.link_enabled(), "Link stays as it is set");

    let mut h = link_page(Harness::manual(Window::Settings));
    let nodes = Harness::nodes(&h.settle());
    assert!(labelled(&nodes, "Show Link Toggle"));
    assert!(labelled(&nodes, "Start Stop Sync"));
}

fn quantum(h: &mut Harness) -> Option<Node> {
    Harness::nodes(&h.settle())
        .into_iter()
        .find(|node| node.label == "Link Quantum")
}

// Spec: link R22, R23, R25. With a peer and the transport stopped, the
// progress bar fills across the Arrangement Position fields with the
// session's phase, a progress indicator named Link Quantum; it is gone with
// no peer, and while the song plays.
#[test]
fn the_progress_bar_fills_with_the_phase_while_stopped_with_a_peer() {
    let mut h = main_window();
    h.shell
        .run("link.set_enabled", json!({ "on": true }))
        .unwrap();
    assert!(quantum(&mut h).is_none(), "no bar with no peer");

    let peer = peer();
    wait_for(&mut h, "a peer", |h| h.shell.link_peers() == 1);
    wait_for(&mut h, "the bar", |h| h.shell.link_progress().is_some());
    let bar = quantum(&mut h).expect("a bar with a peer while stopped");
    assert_eq!(bar.role, Role::ProgressIndicator);
    let share = bar.numeric.expect("its value is the share gone by");
    assert!((0.0..=1.0).contains(&share));
    let now = h.shell.link_progress().unwrap();
    let apart = (share - now).abs();
    assert!(
        apart < 0.2 || apart > 0.8,
        "{share} then {now}: the session's phase"
    );
    let nodes = Harness::nodes(&h.settle());
    let fields = bounds(Harness::node(&nodes, "Arrangement Position Bars")).union(bounds(
        Harness::node(&nodes, "Arrangement Position Sixteenths"),
    ));
    assert_eq!(bounds(&bar), fields, "across the three fields");
    assert!(
        labelled(&nodes, "Arrangement Position Bars"),
        "the fields stay"
    );

    peer.enable(false, 120.0);
    wait_for(&mut h, "no peer", |h| h.shell.link_peers() == 0);
    assert!(quantum(&mut h).is_none(), "gone with the peer");

    // Playing, with the peer back: no bar until the song stops.
    h.shell.run("transport.play", json!({})).unwrap();
    h.wait("playback", |h| h.engine.playing);
    let peer = self::peer();
    wait_for(&mut h, "a peer", |h| h.shell.link_peers() == 1);
    assert!(quantum(&mut h).is_none(), "no bar while the song plays");
    h.shell.run("transport.stop", json!({})).unwrap();
    wait_for(&mut h, "the bar", |h| h.shell.link_progress().is_some());
    assert!(quantum(&mut h).is_some(), "back once it stops");
    peer.enable(false, 120.0);
    h.shell.audio_mut().stop();
}

// Spec: link R25. The window keeps drawing each frame while the bar shows.
#[test]
fn the_window_asks_for_a_frame_while_the_bar_shows() {
    let mut h = main_window();
    h.shell
        .run("link.set_enabled", json!({ "on": true }))
        .unwrap();
    let peer = peer();
    wait_for(&mut h, "the bar", |h| h.shell.link_progress().is_some());
    let output = h.frame(vec![]);
    let wants = output
        .viewport_output
        .values()
        .any(|v| v.repaint_delay == Duration::ZERO);
    assert!(wants, "an immediate repaint is asked for");
    peer.enable(false, 120.0);
}
