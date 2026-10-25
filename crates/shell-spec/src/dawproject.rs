//! DAWproject import and export (WP-6.36, WP-6.37): the
//! `set.import_dawproject` and `set.export_dawproject` commands and the
//! `set.import_report` read command, their arguments, and their results.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID. The import reads a file outside the project and the export writes
//! one, so they are in the external class and an agent asks the person
//! first. The behaviour they follow is in the DAWproject spec (WP-6.36,
//! WP-6.37).

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `set.import_dawproject`: open a DAWproject file as a new set.
pub const IMPORT_DAWPROJECT: &str = "set.import_dawproject";

/// `set.import_report`: what the last DAWproject import left out or changed.
pub const IMPORT_REPORT: &str = "set.import_report";

/// `set.export_dawproject`: write the open set as a DAWproject file.
pub const EXPORT_DAWPROJECT: &str = "set.export_dawproject";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[IMPORT_DAWPROJECT, IMPORT_REPORT, EXPORT_DAWPROJECT];

/// Whether `id` is one of the DAWproject commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `set.import_dawproject`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDawProjectArgs {
    /// The DAWproject file to read, absolute or relative to the app's
    /// working directory.
    pub path: String,
}

/// Arguments of `set.export_dawproject`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportDawProjectArgs {
    /// The file to write, absolute or relative to the app's working
    /// directory; `.dawproject` is added when it is missing.
    pub path: String,
    /// Whether to write over a file already at `path`; without it, an
    /// existing file is refused.
    #[serde(default)]
    pub replace: bool,
    /// Whether to put the audio files in the DAWproject file (the default)
    /// or refer to them by their absolute paths.
    #[serde(default = "embed_by_default")]
    pub embed_media: bool,
}

/// The media are embedded unless the person asks otherwise.
const fn embed_by_default() -> bool {
    true
}

/// One thing the import left out or changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportReportEntry {
    /// What it was: a VST2 plug-in, a send's pan, a note's channel.
    pub what: String,
    /// Where it was: the track by name, a clip by name and its place, a
    /// device by name.
    #[serde(rename = "where")]
    pub place: String,
    /// How many there were.
    pub count: usize,
    /// What Beatlayer did: left out, or changed to what.
    pub did: String,
}

/// What the import left out or changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportReport {
    /// The application and version the file says made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    /// Each thing left out or changed, in the file's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ImportReportEntry>,
}

/// What `set.export_dawproject` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportDawProjectResult {
    /// The file that was written, with `.dawproject` where it was added.
    pub path: String,
    /// What the format can't carry, which the file leaves out, in the form
    /// of the import report: each thing, where it was, how many there were
    /// and what the file holds in its place.
    pub report: ImportReport,
}

/// What `set.import_dawproject` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportDawProjectResult {
    /// The new set's name, as the title bar shows it: the file's name
    /// without `.dawproject`.
    pub name: String,
    /// What the import left out or changed.
    pub report: ImportReport,
}

/// Adds the DAWproject commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(IMPORT_DAWPROJECT, "Import DAWproject File")
            .description(
                "Reads the DAWproject file at `path` and opens what it holds as a new set, in \
                 place of the open one, as opening a set does: it is refused while the open set \
                 has unsaved changes. The new set starts empty and holds what the file carries: \
                 its tracks, groups, returns and their mixer and routing, its clips with their \
                 notes, media, warping and loops, its scenes and slots, its tempo and time \
                 signature, its colours, fades and crossfades, its devices and plug-ins with \
                 their saved states, its automation, MIDI controllers and note expressions, \
                 and its markers as locators. It is unsaved, and its name is the file's name without `.dawproject` \
                 until it is first saved. The result's `report` says what the import left out or \
                 changed, and `set.import_report` gives that report again while the set the \
                 import made is open. Reads a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ImportDawProjectArgs>()
            .result::<ImportDawProjectResult>(),
    )?;
    registry.register(
        CommandSpec::new(IMPORT_REPORT, "DAWproject Import Report")
            .description(
                "Reports what the last DAWproject import left out or changed: each thing, where \
                 it was, how many there were, and what Beatlayer did, and the application and \
                 version the file says made it. It gives the report again while the set the \
                 import made is open, and nothing after another set opens. Changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ImportReport>(),
    )?;
    registry.register(
        CommandSpec::new(EXPORT_DAWPROJECT, "Export DAWproject File")
            .description(
                "Writes the open set to the file at `path` as DAWproject 1.0, adding \
                 `.dawproject` when it is missing. It is refused when a file is already there \
                 unless `replace` is true. The plug-ins' states are taken from the running \
                 plug-ins as a save takes them, and the file is written on a worker thread \
                 into a hidden file beside the destination and then renamed into place, so the \
                 destination holds the whole file or nothing new; the set does not change. \
                 The file holds the tracks, groups, returns and their mixer and routing, the \
                 clips with their notes, media, warping, fades and crossfades, the scenes and \
                 slots, the tempo and time signature, the automation, the devices and plug-ins \
                 with their saved states, Beatlayer's own devices and racks with their presets, \
                 and the locators. The audio files are embedded unless `embed_media` is false, \
                 when each is referred to by its absolute path. The result gives the file's \
                 path and a `report` of what the format cannot carry, which the file leaves \
                 out. Writes a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ExportDawProjectArgs>()
            .result::<ExportDawProjectResult>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");

        let import = registry.get(IMPORT_DAWPROJECT).unwrap();
        assert_eq!(import.permission, Some(Permission::External));
        let schema = import.args.as_ref().unwrap();
        assert!(bl_commands::validate(schema, &json!({ "path": "/a/b.dawproject" })).is_ok());
        assert!(bl_commands::validate(schema, &json!({})).is_err());

        let report = registry.get(IMPORT_REPORT).unwrap();
        assert_eq!(report.permission, Some(Permission::Read));

        let export = registry.get(EXPORT_DAWPROJECT).unwrap();
        assert_eq!(export.permission, Some(Permission::External));
        let schema = export.args.as_ref().unwrap();
        assert!(bl_commands::validate(schema, &json!({ "path": "/a/b.dawproject" })).is_ok());
        assert!(
            bl_commands::validate(
                schema,
                &json!({ "path": "/a/b", "replace": true, "embed_media": false })
            )
            .is_ok()
        );
        assert!(bl_commands::validate(schema, &json!({})).is_err());
    }
}
