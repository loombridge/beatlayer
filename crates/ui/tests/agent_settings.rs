//! The agent server's section of the Settings window (WP-1.15), headless:
//! on the Tempo & MIDI page (L-SET-014, laid out as L-SET-003), off until
//! the person switches it on, with the address, the token, and each
//! allowed agent's autonomous switch and own token (WP-1.15c), every
//! control reachable by a screen reader and each change a registered
//! command.

mod common;

use std::time::Instant;

use bl_shell::agent::AgentServerStatus;
use bl_shell::settings::SettingsPage;
use common::{Harness, Window};
use egui::accesskit::{Action, ActionRequest, Role, TreeId};
use egui::{Event, Id};
use serde_json::json;

fn open_tempo_midi() -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "tempo-midi" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    assert_eq!(h.shell.settings_page(), SettingsPage::TempoMidi);
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
    let mut h = open_tempo_midi();
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
    let mut h = open_tempo_midi();
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
    let mut h = open_tempo_midi();
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

/// The UI's CPU time per frame for this view, against a scene like
/// ADR-001's benchmark: 200 tracks and 5,000 clips in the open set, the
/// agent server on, and 20 allowed agents. Run it in release:
/// `cargo test --release -p bl-ui --test agent_settings -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run on demand in release"]
fn frame_time_of_the_agents_section() {
    let mut h = open_tempo_midi();
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
        "Settings, Tempo & MIDI, Agents section: median {median:.3} ms, p99 {p99:.3} ms of CPU \
         per frame over {} frames (layout, AccessKit tree, and tessellation; no GPU)",
        times.len()
    );
}
