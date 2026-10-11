//! The agent guide: Beatlayer's concepts, IDs, and good practice, for the
//! agent. It's `crates/agent/guide.md`, served as the MCP resource
//! [`URI`], and the server's `instructions` point to it.

use serde_json::{Value, json};

/// The guide's text: `crates/agent/guide.md`.
pub const TEXT: &str = include_str!("../guide.md");

/// Where the guide is served.
pub const URI: &str = "beatlayer://guide";

/// What `initialize` tells the agent, which clients show to the model.
pub const INSTRUCTIONS: &str = "Beatlayer is a digital audio workstation with the workflow of \
    Ableton Live 12. Read the guide resource beatlayer://guide first: it explains tracks, \
    clips, devices, IDs, ticks, and permissions. Find commands with list_commands and \
    describe_command, read the project with query, and change it with run, which makes each \
    call one undo step the person can undo. Use checkpoint before experiments and schedule \
    for anything that must land on a beat.";

/// The guide's entry in `resources/list`.
#[must_use]
pub fn listing() -> Value {
    json!({
        "uri": URI,
        "name": "guide",
        "title": "Beatlayer agent guide",
        "description": "Beatlayer's concepts, IDs, time, permissions, and good practice for \
                        agents. Read it before your first edit.",
        "mimeType": "text/markdown",
        "size": TEXT.len(),
    })
}

/// The resource at `uri`, as `resources/read` returns it, if it's the
/// guide.
#[must_use]
pub fn read(uri: &str) -> Option<Value> {
    (uri == URI).then(|| json!({ "uri": URI, "mimeType": "text/markdown", "text": TEXT }))
}
