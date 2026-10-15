//! Audio Unit hosting (WP-2.8; the AU hosting spec): a plug-in format of
//! macOS.
//!
//! An Audio Unit has no file and no ID of its own: the system finds it by
//! its *component description* — a type, a subtype, and a manufacturer,
//! four characters each — which [`description_id`] writes as
//! `type/subtype/manufacturer` (such as `aufx/dely/appl`, Apple's AUDelay)
//! for the device's `plugin_id`, and [`parse_description_id`] reads back.
//!
//! The implementation is macOS only ([the `mac` module](mac)); on Windows
//! and Linux this module compiles to [`stub`], which answers for it: its
//! commands say Audio Units aren't available, and a set that carries an AU
//! device from a Mac passes its input through, as an unhosted device does.

use std::fmt;

use bl_model::DeviceId;

#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
#[path = "stub.rs"]
mod imp;

pub use crate::clap::PluginStatus;
pub use imp::{AuFactory, AuHost, AuType};

/// Why an Audio Unit host call failed. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuError {
    /// This platform doesn't host Audio Units: they're macOS only.
    Unavailable,
    /// No installed Audio Unit has that description, it isn't a device
    /// Beatlayer hosts, or it won't start, and why.
    Load(String),
    /// The device has no running Audio Unit: it isn't an AU device, or its
    /// unit couldn't be loaded.
    NoInstance(DeviceId),
    /// The unit refused, and what it refused.
    Refused(String),
    /// It was called off the host's own thread.
    WrongThread,
}

impl fmt::Display for AuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("Audio Units are only available on macOS"),
            Self::Load(why) => write!(f, "couldn't load the Audio Unit: {why}"),
            Self::NoInstance(device) => write!(f, "{device} has no running Audio Unit"),
            Self::Refused(what) => write!(f, "the Audio Unit refused {what}"),
            Self::WrongThread => f.write_str("the host's calls run on the thread that made it"),
        }
    }
}

impl std::error::Error for AuError {}

/// The four characters a component description's field holds as text: all
/// four, even a trailing space (Apple's DLS synth's subtype is `dls `), so
/// reading an id back gives the description it was made from.
///
/// Used by the macOS host's role mapping and by the tests' `id_of`: on
/// Windows and Linux a library build has no caller.
#[cfg(any(target_os = "macos", test))]
fn field(code: u32) -> String {
    let bytes = code.to_be_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A component description field from its four characters: `text` must be
/// four characters, printable ASCII and without `/` (which joins the
/// fields of a `plugin_id`).
fn field_of(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    if bytes.len() != 4
        || bytes
            .iter()
            .any(|b| !(0x20..0x7f).contains(b) || *b == b'/')
    {
        return None;
    }
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// The `plugin_id` of an Audio Unit whose description's fields are `kind`,
/// `subtype`, and `manufacturer`: `type/subtype/manufacturer`, such as
/// `aufx/dely/appl`. `None` if a field isn't four characters.
#[must_use]
pub fn description_id(kind: &str, subtype: &str, manufacturer: &str) -> Option<String> {
    field_of(kind)?;
    field_of(subtype)?;
    field_of(manufacturer)?;
    Some(format!("{kind}/{subtype}/{manufacturer}"))
}

/// The three fields a `plugin_id` names, if it's one [`description_id`]
/// made.
#[must_use]
pub fn parse_description_id(id: &str) -> Option<(&str, &str, &str)> {
    let mut fields = id.split('/');
    let (kind, subtype, manufacturer) = (fields.next()?, fields.next()?, fields.next()?);
    if fields.next().is_some() {
        return None;
    }
    field_of(kind)?;
    field_of(subtype)?;
    field_of(manufacturer)?;
    Some((kind, subtype, manufacturer))
}

/// The three four-character codes a `plugin_id` names, if it names them.
///
/// Used by the macOS host's component lookup: a library build on Windows
/// and Linux has no caller.
#[cfg(target_os = "macos")]
pub(crate) fn description_fields(id: &str) -> Option<(u32, u32, u32)> {
    let (kind, subtype, manufacturer) = parse_description_id(id)?;
    Some((field_of(kind)?, field_of(subtype)?, field_of(manufacturer)?))
}

/// Writes three codes as a `plugin_id`: the tests' own spelling of what
/// [`description_id`] makes of the three fields.
#[cfg(test)]
pub(crate) fn id_of(kind: u32, subtype: u32, manufacturer: u32) -> String {
    format!("{}/{}/{}", field(kind), field(subtype), field(manufacturer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_component_description_writes_and_reads_as_its_id() {
        let id = description_id("aufx", "dely", "appl").expect("four characters each");
        assert_eq!(id, "aufx/dely/appl");
        assert_eq!(parse_description_id(&id), Some(("aufx", "dely", "appl")));
        // A subtype with a space (Apple's DLS synth) keeps it.
        let id = description_id("aumu", "dls ", "appl").expect("four characters each");
        assert_eq!(id, "aumu/dls /appl");
        assert_eq!(parse_description_id(&id), Some(("aumu", "dls ", "appl")));
        assert_eq!(id_of(0x6175_6d75, 0x646c_7320, 0x6170_706c), id);
    }

    #[test]
    fn a_field_that_isnt_four_characters_is_refused() {
        assert!(description_id("aufx", "dely", "apple").is_none());
        assert!(description_id("auf", "dely", "appl").is_none());
        assert!(description_id("aufx", "de/y", "appl").is_none());
        assert!(description_id("aufx", "dely", "app\t").is_none());
        assert!(description_id("aufx", "dely", "ap\u{7f}l").is_none());
        assert!(parse_description_id("aufx/dely").is_none());
        assert!(parse_description_id("aufx/dely/appl/appl").is_none());
    }
}
