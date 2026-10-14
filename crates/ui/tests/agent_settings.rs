//! The agent server's settings of the Settings window (WP-1.15), headless:
//! on the Agents page (L-SET-020, laid out as L-SET-003), off until
//! the person switches it on, with the address, the token, and each
//! allowed agent's autonomous switch and own token (WP-1.15c), every
//! control reachable by a screen reader and each change a registered
//! command. The address and the tokens can be selected and copied, each
//! with a Copy button beside it.

mod common;

use std::time::Instant;

use bl_shell::agent::AgentServerStatus;
use bl_shell::settings::SettingsPage;
use common::{Harness, Window};
use egui::accesskit::{Action, ActionRequest, Role, TreeId};
use egui::{Event, Id, OutputCommand};
use serde_json::json;

fn open_agents() -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "agents" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    assert_eq!(h.shell.settings_page(), SettingsPage::Agents);
    h
}

/// Clicks the node labelled `label`, as a screen reader does.
fn press(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let id = Harness::node(&nodes, label).id;
    h.frame(vec![Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: id,
        data: None,
    })]);
    h.settle();
}

#[test]
fn the_agent_server_is_off_until_switched_on_and_then_shows_where_to_connect() {
    let mut h = open_agents();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let switch = Harness::node(&nodes, "Agent Server");
    assert_eq!(switch.role, Role::Button);
    assert_eq!(Harness::node(&nodes, "Status").value, "Off");
    assert_eq!(
        Harness::node(&nodes, "Address").value,
        "http://127.0.0.1:37420/mcp"
    );
    assert!(
        !nodes.iter().any(|n| n.label == "Token"),
        "no token before the server is first switched on"
    );

    h.accesskit(Id::new(("switch", "agent-server")), Action::Click);
    h.settle();
    assert!(h.shell.agent_settings().server);
    let token = h.shell.agent_settings().token.clone();
    assert_eq!(token.expose().len(), 64);
    assert!(h.events.iter().any(|e| matches!(
        e,
        bl_ui::main_window::UiEvent::Invoke(i) if i.id == "agent.set_server"
    )));

    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Status").value, "Starting");
    let field = Harness::node(&nodes, "Token");
    assert_eq!(field.value, token, "the token can be read and copied");

    // The app reports the running server.
    h.shell.set_agent_status(AgentServerStatus {
        listening: Some("127.0.0.1:37420".to_owned()),
        error: None,
        sessions: 1,
    });
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(
        Harness::node(&nodes, "Status").value,
        "Listening, 1 agent session"
    );
    h.shell.set_agent_status(AgentServerStatus {
        listening: None,
        error: Some("port 37420 is in use".to_owned()),
        sessions: 0,
    });
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(
        Harness::node(&nodes, "Status").value,
        "Couldn't start: port 37420 is in use"
    );

    press(&mut h, "New Token");
    assert_ne!(h.shell.agent_settings().token, token);
}

#[test]
fn each_allowed_agent_has_an_autonomous_switch_and_can_be_forgotten() {
    let mut h = open_agents();
    h.shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Autonomous: claude-code").role,
        Role::Button
    );
    assert!(!h.shell.agent_settings().is_autonomous("claude-code"));

    h.accesskit(
        Id::new(("switch", "agent-autonomous-claude-code")),
        Action::Click,
    );
    h.settle();
    assert!(h.shell.agent_settings().is_autonomous("claude-code"));

    press(&mut h, "Forget claude-code");
    assert!(h.shell.agent_settings().client("claude-code").is_none());
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(!nodes.iter().any(|n| n.label == "Autonomous: claude-code"));
}

/// Every piece of text the frame painted.
fn painted(output: &egui::FullOutput) -> String {
    fn walk(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(text) => {
                out.push_str(text.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

#[test]
fn each_allowed_agent_s_token_shows_in_its_row_and_new_token_replaces_it() {
    // Spec: agent-permissions R12, R13.
    let mut h = open_agents();
    h.shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    for name in ["claude-code", "goose"] {
        h.shell
            .run("agent.approve_client", json!({ "client": name }))
            .unwrap();
    }
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let server = h.shell.agent_settings().token.clone();
    let own = |h: &Harness, name: &str| {
        h.shell
            .agent_settings()
            .client(name)
            .unwrap()
            .token
            .expose()
            .to_owned()
    };
    let token = own(&h, "claude-code");
    let field = Harness::node(&nodes, "Token: claude-code");
    assert_eq!(
        field.value, token,
        "the agent's token can be read and copied"
    );
    assert_ne!(field.value, server.expose());
    assert_eq!(
        Harness::node(&nodes, "Token: goose").value,
        own(&h, "goose")
    );
    assert_eq!(
        Harness::node(&nodes, "New Token: claude-code").role,
        Role::Button
    );
    // It's laid out under the agent's name, after its switch.
    let top = |label: &str| Harness::node(&nodes, label).bounds.unwrap().y0;
    assert!(top("Autonomous: claude-code") < top("Token: claude-code"));
    assert!(top("Token: claude-code") < top("Autonomous: goose"));

    // The note says who is asked.
    let text = painted(&output);
    assert!(
        text.contains("An agent you allow gets its own token and isn't asked again."),
        "{text}"
    );
    assert!(
        text.contains("Clients using the server's token are asked each time."),
        "{text}"
    );
    assert!(!text.contains("Each agent is asked about once"), "{text}");

    // New Token runs `agent.new_client_token` for that agent only.
    let goose = own(&h, "goose");
    h.events.clear();
    press(&mut h, "New Token: claude-code");
    assert!(h.events.iter().any(|e| matches!(
        e,
        bl_ui::main_window::UiEvent::Invoke(i)
            if i.id == "agent.new_client_token" && i.args == json!({ "client": "claude-code" })
    )));
    let new = own(&h, "claude-code");
    assert_ne!(new, token);
    assert_eq!(new.len(), 64);
    assert_eq!(own(&h, "goose"), goose);
    assert_eq!(h.shell.agent_settings().token, server);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Token: claude-code").value, new);
}

/// Clicks the node labelled `label` and returns what the clipboard got:
/// egui's `CopyText` command, over the click's frame and the next few in
/// case the action is handled a frame later.
fn click(h: &mut Harness, label: &str) -> Vec<String> {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let id = Harness::node(&nodes, label).id;
    let copied = |output: &egui::FullOutput| -> Vec<String> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    let mut out = copied(&h.frame(vec![Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: id,
        data: None,
    })]));
    for _ in 0..3 {
        if !out.is_empty() {
            break;
        }
        out = copied(&h.frame(vec![]));
    }
    out
}

#[test]
fn the_page_list_puts_agents_after_plug_ins_with_none_of_the_settings_on_tempo_midi() {
    // Decided 14 October 2026: the agent server's settings move to
    // a page of their own, straight after Plug-Ins.
    let mut h = open_agents();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let list = nodes
        .iter()
        .find(|n| n.role == Role::TabList)
        .expect("a tab list");
    let mut tabs: Vec<&str> = Vec::new();
    let mut stack = vec![list.id];
    while let Some(id) = stack.pop() {
        let node = nodes.iter().find(|n| n.id == id).unwrap();
        if node.role == Role::Tab {
            tabs.push(node.label.as_str());
        }
        stack.extend(node.children.iter().rev());
    }
    let plug_ins = tabs.iter().position(|t| *t == "Plug-Ins").unwrap();
    assert_eq!(
        tabs[plug_ins + 1],
        "Agents",
        "Agents follows Plug-Ins: {tabs:?}"
    );

    // The section's controls are on the Agents page.
    h.shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    h.shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    for label in [
        "Agent Server",
        "Status",
        "Address",
        "Token",
        "Autonomous: claude-code",
        "Token: claude-code",
    ] {
        let _ = Harness::node(&nodes, label);
    }
    // The section's two headings are painted text, not controls.
    let text = painted(&h.frame(vec![]));
    assert!(text.contains("Agents"), "{text}");
    assert!(text.contains("Allowed Agents"), "{text}");

    // None of it is left on Tempo & MIDI.
    h.shell
        .invoke("app.settings", json!({ "page": "tempo-midi" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    for label in [
        "Agent Server",
        "Status",
        "Address",
        "Token",
        "Autonomous: claude-code",
        "Token: claude-code",
    ] {
        assert!(
            !nodes.iter().any(|n| n.label == label),
            "{label} is still on Tempo & MIDI"
        );
    }
    let text = painted(&h.frame(vec![]));
    assert!(!text.contains("Allowed Agents"), "{text}");
}

#[test]
fn each_copy_button_puts_its_value_on_the_clipboard_and_the_values_are_selectable() {
    let mut h = open_agents();
    h.shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    h.shell
        .run("agent.approve_client", json!({ "client": "claude-code" }))
        .unwrap();
    let server = h.shell.agent_settings().token.clone();
    let own = h
        .shell
        .agent_settings()
        .client("claude-code")
        .unwrap()
        .token
        .clone();
    let url = h.shell.agent_settings().url();

    // The values sit in selectable read-only fields, not plain labels.
    let nodes = Harness::nodes(&h.settle());
    for (label, value) in [
        ("Address", url.as_str()),
        ("Token", server.expose()),
        ("Token: claude-code", own.expose()),
    ] {
        let node = Harness::node(&nodes, label);
        assert_eq!(node.role, Role::TextInput, "{label} is selectable");
        assert_eq!(node.value, value, "{label}");
    }

    // Each Copy button puts exactly its value on the clipboard.
    assert_eq!(click(&mut h, "Copy Address"), [url]);
    assert_eq!(click(&mut h, "Copy Token"), [server.expose().to_owned()]);
    assert_eq!(
        click(&mut h, "Copy Token: claude-code"),
        [own.expose().to_owned()]
    );
}

/// The UI's CPU time per frame for this view, against a scene like
/// ADR-001's benchmark: 200 tracks and 5,000 clips in the open set, the
/// agent server on, and 20 allowed agents. Run it in release:
/// `cargo test --release -p bl-ui --test agent_settings -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run on demand in release"]
fn frame_time_of_the_agents_page() {
    let mut h = open_agents();
    h.shell.edit(|editor| {
        for t in 0..200usize {
            let track: bl_model::TrackId = editor.new_id();
            editor
                .apply(bl_model::Command::CreateTrack {
                    id: track,
                    kind: bl_model::TrackKind::Midi,
                    name: format!("Track {t}"),
                    parent: None,
                    index: t,
                })
                .unwrap();
            for c in 0..25i64 {
                let clip = bl_model::Clip::new(
                    editor.new_id(),
                    bl_core::time::Ticks::WHOLE * c,
                    bl_core::time::Ticks::WHOLE,
                    bl_model::ClipData::Midi(bl_model::MidiData::default()),
                );
                editor
                    .apply(bl_model::Command::CreateClip { track, clip })
                    .unwrap();
            }
        }
    });
    assert_eq!(h.shell.editor().document().clip_count(), 5_000);
    h.shell
        .run("agent.set_server", json!({ "on": true }))
        .unwrap();
    for n in 0..20 {
        h.shell
            .run(
                "agent.approve_client",
                json!({ "client": format!("agent-{n}") }),
            )
            .unwrap();
    }
    h.settle();
    let mut times = Vec::new();
    for _ in 0..600 {
        let start = Instant::now();
        let output = h.frame(vec![]);
        let primitives = h.ctx.tessellate(output.shapes, output.pixels_per_point);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(primitives);
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    eprintln!(
        "Settings, Agents page: median {median:.3} ms, p99 {p99:.3} ms of CPU \
         per frame over {} frames (layout, AccessKit tree, and tessellation; no GPU)",
        times.len()
    );
}
