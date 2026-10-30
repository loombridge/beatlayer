//! The Control Surfaces table through the agent API (WP-7.5; the control
//! surfaces spec, R7, R11, R13 and R14): `control_surface.set`, `list`,
//! `dump` and `surfaces` by ID, read back, and agents' calls waiting for a
//! surface's gesture as they wait for the person's.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, Credential, Outcome};
use bl_control::testing::{HELLO, Journal, SETUP};
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_shell::Shell;
use serde_json::{Value, json};

fn shell_with_ports() -> (Shell, TestBackend) {
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Pads");
    test.add_port(Direction::Output, "Pads Out");
    let mut shell = Shell::headless();
    shell.use_midi_backend(Box::new(test.clone()));
    (shell, test)
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let reply = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap();
    assert!(!reply.is_error, "{command}: {}", reply.text);
    reply.ok().unwrap()["results"][0]["result"].clone()
}

// Spec: control-surfaces R11, R13, R14. An agent sets a row, lists it back,
// dumps its setup and lists the surfaces the chooser offers.
#[test]
fn an_agent_sets_lists_dumps_and_reads_the_surfaces_back() {
    let (shell, test) = shell_with_ports();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();

    let surfaces = run(&mut client, "control_surface.surfaces", json!({}));
    assert_eq!(surfaces["surfaces"][0]["id"], "test");
    assert_eq!(
        surfaces["surfaces"][0]["ring"],
        json!({ "tracks": 8, "scenes": 2 })
    );

    let listed = run(&mut client, "control_surface.list", json!({}));
    assert_eq!(listed["rows"].as_array().unwrap().len(), 6);
    assert_eq!(listed["rows"][0]["state"], "empty");

    let set = run(
        &mut client,
        "control_surface.set",
        json!({ "row": 1, "surface": "test", "input": "Pads", "output": "Pads Out" }),
    );
    let listed = run(&mut client, "control_surface.list", json!({}));
    assert_eq!(
        listed["rows"][0], set,
        "`list`'s rows read back what `set` did"
    );
    assert_eq!(listed["rows"][0]["state"], "running");
    assert_eq!(listed["rows"][0]["can_dump"], true);
    assert_eq!(listed["rows"][0]["surface_state"]["mode"], "Test");
    assert_eq!(test.sent("Pads Out"), [HELLO]);

    let dumped = run(&mut client, "control_surface.dump", json!({ "row": 1 }));
    assert_eq!(dumped, json!({ "sent": 1 }));
    assert_eq!(test.sent("Pads Out").last().unwrap(), SETUP);

    // A port another row has is refused, naming the row.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "control_surface.set",
                                   "args": { "row": 2, "input": "Pads" } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(refused.text.contains("row 1"), "{}", refused.text);
}

/// The app's agent API on `shell`, as the permissions tests have it: calls
/// go to `Agent::handle`, and calls that wait are resumed.
struct Host {
    agent: Agent,
    shell: Shell,
    session: String,
    next: u64,
}

impl Host {
    fn new(shell: Shell) -> Self {
        let mut agent = Agent::for_app();
        let session = agent.open_session_as(None, Credential::Server);
        let mut host = Self {
            agent,
            shell,
            session,
            next: 0,
        };
        let Outcome::Ask(question) = host.send(
            "initialize",
            json!({ "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "editor" } }),
        ) else {
            panic!("the agent asks to connect")
        };
        host.agent
            .resolve(&mut host.shell, question.id, true)
            .unwrap();
        host
    }

    fn send(&mut self, method: &str, params: Value) -> Outcome {
        self.next += 1;
        let message = json!({ "jsonrpc": "2.0", "id": self.next, "method": method,
                              "params": params });
        self.agent.handle(&mut self.shell, &self.session, message)
    }
}

// Spec: control-surfaces R7. A surface's gesture is the person's own: while
// a knob is held an agent's call waits, and it lands after the release.
#[test]
fn an_agents_call_waits_while_a_surface_gesture_lasts() {
    let (shell, test) = shell_with_ports();
    let mut host = Host::new(shell);
    host.shell
        .invoke("track.create", json!({ "kind": "audio", "name": "T" }))
        .unwrap();
    let track = host.shell.editor().document().top_level().last().unwrap();
    host.shell
        .run(
            "control_surface.set",
            json!({ "row": 1, "surface": "test", "input": "Pads", "output": "Pads Out" }),
        )
        .unwrap();
    let journal = host
        .shell
        .midi()
        .surfaces()
        .probe(0)
        .unwrap()
        .downcast::<Journal>()
        .map(|journal| (*journal).clone())
        .unwrap();
    journal.knob("mixer.set_volume", json!({ "track": track, "volume": "$" }));

    let now = TestClock::new().now();
    assert!(test.send("Pads", now, &[0xB0, 7, 64]));
    host.shell.midi().surfaces().settle();
    host.shell.poll();
    assert!(host.shell.gesture_under_way());

    let rename = json!({ "name": "run", "arguments": { "commands": [
        { "command": "track.rename", "args": { "track": track, "name": "Keys" } }] } });
    let Outcome::Wait(ticket) = host.send("tools/call", rename) else {
        panic!("the call waits while the knob is held")
    };
    assert!(host.agent.resume(&mut host.shell).is_none());

    assert!(test.send("Pads", now, &[0x80, 7, 0]));
    host.shell.midi().surfaces().settle();
    host.shell.poll();
    assert!(!host.shell.gesture_under_way());
    let (done, outcome) = host.agent.resume(&mut host.shell).expect("it runs now");
    assert_eq!(done, ticket);
    assert!(matches!(outcome, Outcome::Reply(Some(_))), "{outcome:?}");
    // The knob's step came first, the agent's own after it.
    let history: Vec<_> = host.shell.editor().undo_history().collect();
    assert_eq!(history[0], "editor: Rename Track");
    assert_eq!(history[1], "Set Track Volume");
    assert_eq!(
        host.shell.editor().document().track(track).unwrap().name(),
        "Keys"
    );
}
