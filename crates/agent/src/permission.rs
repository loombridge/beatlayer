//! Who may run what: a command's permission class (D17) against where the
//! agent API runs and what the person allowed.
//!
//! Read and edit commands run straight away. Destructive and external ones
//! ask the person first, unless the person turned on autonomous mode for
//! that agent. Headless, there's no person to ask, so they're refused
//! unless `bl-cli mcp` was started with `--autonomous`. Commands that change
//! what agents may do (the agent server's own) are refused whatever the
//! agent's mode.

use bl_commands::Permission;
use bl_shell::Shell;

/// Where the agent API runs, which decides who answers when a command asks
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `bl-cli mcp`: no window and no person to ask.
    Headless {
        /// Whether `--autonomous` was given, so destructive and external
        /// commands run without asking.
        autonomous: bool,
    },
    /// The running app: the person answers, and autonomous mode is set per
    /// agent in the Settings window.
    App,
}

/// What to do with a command an agent asked to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run it.
    Run,
    /// Ask the person first.
    Ask,
    /// Refuse it, saying why.
    Refuse(String),
}

/// Decides whether an agent may run commands whose most far-reaching
/// class is `class`. `commands` are their IDs, for the messages.
pub(crate) fn decide(
    mode: Mode,
    shell: &Shell,
    client: &str,
    class: Permission,
    commands: &[&str],
) -> Decision {
    if let Some(id) = commands.iter().find(|id| bl_shell::agent::handles(id)) {
        return Decision::Refuse(format!(
            "`{id}` changes what agents may do, so only a person can run it, in Beatlayer's \
             Settings window (Tempo & MIDI, Agents)."
        ));
    }
    if !class.asks_first() {
        return Decision::Run;
    }
    match mode {
        Mode::Headless { autonomous: true } => Decision::Run,
        Mode::Headless { autonomous: false } => {
            let named = commands
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ");
            Decision::Refuse(format!(
                "{named} {} {class}, and `bl-cli mcp` has no person to ask. Start it with \
                 `--autonomous` to let agents run destructive and external commands without \
                 asking.",
                if commands.len() == 1 { "is" } else { "include" }
            ))
        }
        Mode::App if shell.agent_settings().is_autonomous(client) => Decision::Run,
        Mode::App => Decision::Ask,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn read_and_edit_run_and_the_rest_ask_or_refuse() {
        let mut shell = Shell::headless();
        let headless = Mode::Headless { autonomous: false };
        let free = Mode::Headless { autonomous: true };
        for class in [Permission::Read, Permission::Edit] {
            assert_eq!(
                decide(headless, &shell, "a", class, &["x.y"]),
                Decision::Run
            );
            assert_eq!(
                decide(Mode::App, &shell, "a", class, &["x.y"]),
                Decision::Run
            );
        }
        for class in [Permission::Destructive, Permission::External] {
            let Decision::Refuse(message) = decide(headless, &shell, "a", class, &["track.delete"])
            else {
                panic!("headless refuses {class}");
            };
            assert!(message.contains("--autonomous"), "{message}");
            assert!(message.contains("`track.delete` is"), "{message}");
            assert_eq!(decide(free, &shell, "a", class, &["x.y"]), Decision::Run);
            assert_eq!(
                decide(Mode::App, &shell, "a", class, &["x.y"]),
                Decision::Ask
            );
        }
        shell
            .run("agent.approve_client", json!({ "client": "a" }))
            .unwrap();
        shell
            .run("agent.set_autonomous", json!({ "client": "a", "on": true }))
            .unwrap();
        assert_eq!(
            decide(Mode::App, &shell, "a", Permission::Destructive, &["x.y"]),
            Decision::Run
        );
        assert_eq!(
            decide(Mode::App, &shell, "b", Permission::Destructive, &["x.y"]),
            Decision::Ask
        );
    }

    #[test]
    fn agents_never_change_their_own_permissions() {
        let shell = Shell::headless();
        for mode in [Mode::App, Mode::Headless { autonomous: true }] {
            let decision = decide(
                mode,
                &shell,
                "a",
                Permission::External,
                &["agent.set_autonomous"],
            );
            assert!(matches!(decision, Decision::Refuse(m) if m.contains("only a person")));
        }
    }
}
