//! What the plug-in reports back, read from the state the host saves.
//!
//! A file of its own, with nothing but the standard library, so the host's
//! tests can take it in with `#[path]` instead of linking this crate beside
//! `bl-test-vst3`: both export the VST3 entry points, and a test binary that
//! links the two has two `GetPluginFactory`s.

/// A component state that makes the plug-in ask the host for the transport:
/// play, locate, a cycle range, the cycle on, and stop.
pub const REQUEST: &[u8] = b"REQUEST";

/// What the plug-in reported, read back from the component state the host
/// saved (see the crate documentation).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// How many times the host broke one of ARA's rules.
    pub violations: u32,
    /// What its editor view was told is selected, as (regions, sequences).
    pub selections: Vec<(usize, usize)>,
    /// Its audio modifications.
    pub modifications: Vec<ModificationReport>,
    /// How many audio sources it holds.
    pub sources: usize,
    /// The archive ID it was restored from, if it was.
    pub archive_id: String,
}

/// One audio modification, as the plug-in reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModificationReport {
    /// Its persistent ID: `clip-` and the clip's number.
    pub id: String,
    /// How many times the host updated its properties.
    pub updates: u32,
    /// What its archive said `updates` was, if an archive was restored.
    pub restored: Option<u32>,
    /// Whether it is active: not deactivated for the undo history.
    pub active: bool,
}

/// Reads the plug-in's report from a state the host saved
/// (`Vst3Host::save_state`'s bytes): the component's part of it.
#[must_use]
pub fn report_of(state: &[u8]) -> Report {
    // The host's state: a magic, then the component's and the controller's
    // parts, each with its length.
    let length = state
        .get(4..8)
        .and_then(|bytes| bytes.try_into().ok())
        .map_or(0, u32::from_le_bytes) as usize;
    let text = String::from_utf8_lossy(state.get(8..8 + length).unwrap_or_default()).into_owned();
    let field = |name: &str| -> &str {
        text.split(';')
            .find_map(|part| part.strip_prefix(name))
            .unwrap_or_default()
    };
    let list = |value: &str| -> Vec<String> {
        value
            .split(',')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    Report {
        violations: field("v=").parse().unwrap_or(0),
        selections: list(field("s="))
            .iter()
            .filter_map(|entry| {
                let (regions, sequences) = entry.split_once(':')?;
                Some((regions.parse().ok()?, sequences.parse().ok()?))
            })
            .collect(),
        modifications: list(field("m="))
            .iter()
            .filter_map(|entry| {
                let mut parts = entry.split(':');
                let id = parts.next()?.to_owned();
                let updates = parts.next()?.parse().ok()?;
                let restored: i64 = parts.next()?.parse().ok()?;
                let active = parts.next()? == "1";
                Some(ModificationReport {
                    id,
                    updates,
                    restored: u32::try_from(restored).ok(),
                    active,
                })
            })
            .collect(),
        sources: field("sources=").parse().unwrap_or(0),
        archive_id: field("a=").to_owned(),
    }
}
