//! The built-in assistant's panel (WP-2.23, part 3; D24), headless: the
//! Control Bar's toggle opens and closes the pane at the main window's right
//! edge, dragging its left edge resizes it within limits, the Float and Dock
//! button moves it to a window of its own and back, a scripted turn's
//! messages and tool calls are drawn in order, Stop cancels, and every
//! control has an accessible name.

use crate::common;
use std::collections::VecDeque;
use std::sync::Mutex;

use bl_assistant::message::{Event, Reply, Request, Stop};
use bl_assistant::provider::{Error, Provider};
use bl_assistant::tools::ToolCall;
use bl_shell::assistant::{Decision, PromptRequest, Prompter, run_turn};
use bl_ui::layout::Element;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event as Input, Key, Modifiers, PointerButton, Pos2};
use serde_json::json;

/// A provider answering with a fixed script.
struct Script(Mutex<VecDeque<Reply>>);

impl Provider for Script {
    fn stream(&self, _request: &Request, on_event: &mut dyn FnMut(Event)) -> Result<Reply, Error> {
        let reply = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted reply");
        if !reply.text.is_empty() {
            on_event(Event::Text(reply.text.clone()));
        }
        for call in &reply.tool_calls {
            on_event(Event::ToolCall(call.clone()));
        }
        Ok(reply)
    }
}

struct Quiet;

impl Prompter for Quiet {
    fn ask(&mut self, request: &PromptRequest) -> Decision {
        panic!("no prompt was expected: {request:?}")
    }
}

fn open(h: &mut Harness) {
    h.shell.invoke("view.toggle_assistant", json!({})).unwrap();
    h.settle();
}

fn drawn_assistant(h: &Harness) -> Option<egui::Rect> {
    h.main.drawn_layout().expect("a drawn layout").assistant
}

/// Presses the node labelled `label`, as a screen reader does.
fn press(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let id = Harness::node(&nodes, label).id;
    h.frame(vec![Input::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id,
            data: None,
        },
    )]);
    h.settle();
}

#[test]
fn the_control_bars_toggle_opens_and_closes_the_pane_at_the_right_edge() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    assert_eq!(drawn_assistant(&h), None, "a new session starts closed");
    let layout = h.main.drawn_layout().unwrap();
    let toggle = layout.rect(Element::AssistantToggle);
    assert_eq!(
        toggle.right(),
        1590.0,
        "the last control on the Control Bar"
    );
    let arrangement = layout.rect(Element::ArrangementView);
    assert!(
        toggle.left() > arrangement.right(),
        "right of the view selector"
    );
    let closed_main = layout.main_view;

    h.click(toggle.center());
    h.settle();
    assert!(h.shell.assistant_panel_state().open);
    let layout = h.main.drawn_layout().unwrap();
    let pane = layout.assistant.expect("the pane shows");
    // Full height beside everything else, at the window's right edge.
    assert_eq!(pane.right(), 1591.0);
    assert_eq!(pane.width(), 320.0);
    assert_eq!(pane.top(), layout.main_view.top());
    assert!(pane.bottom() >= layout.detail.bottom());
    assert!(layout.main_view.right() < pane.left());
    assert!(layout.detail.right() < pane.left());
    assert_eq!(layout.status_bar.right(), pane.left());
    assert_eq!(layout.main_view.left(), closed_main.left());

    // The toggle reads as on to a screen reader.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let node = Harness::node(&nodes, "Show/Hide Assistant");
    assert_eq!(node.toggled, Some(true));

    h.click(toggle.center());
    h.settle();
    assert_eq!(drawn_assistant(&h), None);
    assert_eq!(h.main.drawn_layout().unwrap().main_view, closed_main);
}

/// Drags from `from` to `to` along the pane's middle, in a few frames.
fn drag(h: &mut Harness, y: f32, from: f32, to: f32) {
    let start = Pos2::new(from, y);
    let end = Pos2::new(to, y);
    h.frame(vec![Input::PointerMoved(start)]);
    h.frame(vec![Input::PointerButton {
        pos: start,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Input::PointerMoved(end)]);
    h.frame(vec![Input::PointerMoved(end)]);
    h.frame(vec![Input::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

#[test]
fn dragging_the_panes_left_edge_resizes_it_within_limits() {
    let mut h = Harness::new(Window::Main);
    open(&mut h);
    let pane = drawn_assistant(&h).unwrap();
    let y = pane.center().y;

    // 100 points left makes it 100 wider.
    drag(&mut h, y, pane.left(), pane.left() - 100.0);
    assert_eq!(h.shell.assistant_panel_state().width, 420.0);
    assert_eq!(drawn_assistant(&h).unwrap().width(), 420.0);

    // Far left stops at the widest.
    let pane = drawn_assistant(&h).unwrap();
    drag(&mut h, y, pane.left(), 400.0);
    assert_eq!(h.shell.assistant_panel_state().width, 640.0);

    // And far right stops at the narrowest.
    let pane = drawn_assistant(&h).unwrap();
    drag(&mut h, y, pane.left(), 1590.0);
    assert_eq!(h.shell.assistant_panel_state().width, 240.0);

    // The edge is a splitter a screen reader finds, with its limits.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let splitter = Harness::node(&nodes, "Assistant Width");
    assert_eq!(splitter.role, Role::Splitter);
    assert_eq!(splitter.numeric, Some(240.0));
}

#[test]
fn float_and_dock_move_the_panel_between_the_pane_and_its_own_window() {
    let mut h = Harness::new(Window::Main);
    open(&mut h);
    press(&mut h, "Float Assistant");
    assert!(h.shell.assistant_panel_state().floating);
    assert_eq!(drawn_assistant(&h), None, "the pane makes room");
    assert!(h.shell.assistant_window_open());

    // The window draws the panel, with Dock in the header's place of Float.
    let mut window = Harness::new(Window::Assistant);
    window.shell = std::mem::replace(&mut h.shell, bl_shell::Shell::headless());
    window.settle();
    let nodes = Harness::nodes(&window.frame(vec![]));
    assert!(nodes.iter().any(|n| n.label == "Dock Assistant"));
    assert!(!nodes.iter().any(|n| n.label == "Float Assistant"));
    assert!(
        !nodes.iter().any(|n| n.label == "Assistant Width"),
        "a window has no edge to drag"
    );
    press(&mut window, "Dock Assistant");
    assert!(!window.shell.assistant_panel_state().floating);
    assert!(window.shell.assistant_docked_width().is_some());
}

#[test]
fn a_scripted_turns_messages_and_calls_are_drawn_in_order_and_every_control_is_named() {
    let mut h = Harness::new(Window::Main);
    open(&mut h);
    let track: bl_model::TrackId = h.shell.new_id();
    let provider = Script(Mutex::new(VecDeque::from([
        Reply {
            text: "Adding a track.".to_owned(),
            tool_calls: vec![ToolCall {
                id: "c1".to_owned(),
                name: "track_create".to_owned(),
                arguments: json!({ "id": track, "kind": "audio", "name": "New", "index": 0 }),
            }],
            stop: Stop::End,
        },
        Reply {
            text: "All done.".to_owned(),
            tool_calls: vec![],
            stop: Stop::End,
        },
    ])));
    h.shell
        .invoke("assistant.send", json!({ "text": "Please add a track" }))
        .unwrap();
    let text = h.shell.take_assistant_prompt().unwrap();
    run_turn(&mut h.shell, &provider, &mut Quiet, &text).unwrap();
    h.settle();

    let pane = drawn_assistant(&h).unwrap();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let position = |text: &str| {
        nodes
            .iter()
            .find(|n| n.label.contains(text) || n.value.contains(text))
            .unwrap_or_else(|| panic!("{text:?} isn't drawn"))
            .bounds
            .expect("bounds")
            .y0
    };
    let order = [
        position("Please add a track"),
        position("Adding a track."),
        position("track.create"),
        position("done:"),
        position("All done."),
    ];
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    assert!(pane.contains(Pos2::new(pane.center().x, order[0] as f32)));

    // The provider and model in use, and each control, by name.
    assert!(
        nodes
            .iter()
            .any(|n| n.label.starts_with("Provider and model: ")),
        "the provider and model are shown"
    );
    for label in [
        "Assistant",
        "Float Assistant",
        "Close Assistant",
        "Assistant Message",
        "Send Message",
        "Assistant Width",
    ] {
        assert!(nodes.iter().any(|n| n.label == label), "{label}");
    }
}

#[test]
fn sending_from_the_prompt_field_runs_the_command_and_stop_cancels() {
    let mut h = Harness::new(Window::Main);
    open(&mut h);
    // Type a message and press Enter in the field.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let field = Harness::node(&nodes, "Assistant Message").bounds.unwrap();
    h.click(Pos2::new(
        ((field.x0 + field.x1) / 2.0) as f32,
        ((field.y0 + field.y1) / 2.0) as f32,
    ));
    h.settle();
    h.frame(vec![Input::Text("Make it louder".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.assistant_rows(),
        vec![bl_shell::assistant::Row::You("Make it louder".to_owned())]
    );
    assert!(h.shell.assistant_running(), "the assistant is at work");

    // While it works the button is Stop; pressing it cancels.
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(nodes.iter().any(|n| n.label == "Stop Assistant"));
    assert!(!nodes.iter().any(|n| n.label == "Send Message"));
    press(&mut h, "Stop Assistant");
    assert!(!h.shell.assistant_running());
    assert_eq!(h.shell.take_assistant_prompt(), None, "nothing runs");
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(nodes.iter().any(|n| n.label == "Send Message"));
}
