//! Push 2's MIDI ports by their names on each system
//! (`docs/specs/push-2.md`, R1 and R3).

const PREFIX: &str = "Ableton Push 2";

/// Whether `name` is `prefix` or `prefix` and a number after a space (the
/// interface manual's `nn`, which may be blank).
fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|rest| {
        let rest = rest.trim_start_matches(' ');
        rest.chars().all(|c| c.is_ascii_digit())
    })
}

/// Whether `name` is Push 2's Live port: `Ableton Push 2 Live Port` on
/// macOS, `Ableton Push 2` and an optional number on Windows, and the
/// device's first port, `Ableton Push 2 nn:0`, on Linux (R3).
#[must_use]
pub fn is_live(name: &str) -> bool {
    name == "Ableton Push 2 Live Port"
        || numbered(name, PREFIX)
        || name.strip_suffix(":0").is_some_and(|rest| numbered(rest, PREFIX))
}

/// Whether `name` is Push 2's User port: `Ableton Push 2 User Port`,
/// `MIDIIN2 (Ableton Push 2)` and `MIDIOUT2 (Ableton Push 2)` with an
/// optional number, or the device's second port, `:1` (R3).
#[must_use]
pub fn is_user(name: &str) -> bool {
    name == "Ableton Push 2 User Port"
        || numbered(name, "MIDIIN2 (Ableton Push 2)")
        || numbered(name, "MIDIOUT2 (Ableton Push 2)")
        || name.strip_suffix(":1").is_some_and(|rest| numbered(rest, PREFIX))
}

/// The Live ports among `names`, once each, in the order given.
#[must_use]
pub fn live_ports<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for name in names {
        if is_live(name) && !found.iter().any(|f| f == name) {
            found.push(name.to_owned());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: push-2 R3. The names the interface manual gives on each system.
    #[test]
    fn the_live_and_user_ports_are_recognised_by_their_names() {
        for live in [
            "Ableton Push 2 Live Port",
            "Ableton Push 2",
            "Ableton Push 2 2",
            "Ableton Push 2 20:0",
            "Ableton Push 2:0",
        ] {
            assert!(is_live(live), "{live}");
            assert!(!is_user(live), "{live}");
        }
        for user in [
            "Ableton Push 2 User Port",
            "MIDIIN2 (Ableton Push 2)",
            "MIDIOUT2 (Ableton Push 2) 3",
            "Ableton Push 2 20:1",
        ] {
            assert!(is_user(user), "{user}");
            assert!(!is_live(user), "{user}");
        }
        for other in ["IAC Driver Bus 1", "Ableton Push 3", "Ableton Push 2 Live Portx", "Ableton Push 2 x"] {
            assert!(!is_live(other) && !is_user(other), "{other}");
        }
    }
}
