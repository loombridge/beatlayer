//! Where each command's effect reads back (agent readback).
//!
//! The rule: anything an agent can set, it can read back. Every command that
//! isn't `read` names the place its effect shows up, in [`READBACK`]: a read
//! command and the field in its result, or the query tool's path for an
//! object and the field in its full result. An action with nothing to read
//! back says so, with a reason. [`problems`] checks the table against the
//! registry, and [`markdown`] renders a namespace's rows for the command reference.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::Value;

use crate::{Permission, Registry};

/// An object the query tool reads, by the path that reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    /// `song`: the song's overview and, under `saved`, its settings and scenes.
    Song,
    /// `track`.
    Track,
    /// `clip`.
    Clip,
    /// `device`.
    Device,
    /// `scene`.
    Scene,
}

impl Object {
    /// The query path that reaches the object.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Track => "track",
            Self::Clip => "clip",
            Self::Device => "device",
            Self::Scene => "scene",
        }
    }

    /// The fields of the object's full result that sit beside `saved` rather
    /// than in it. A test in `bl-agent` holds these against the real result.
    #[must_use]
    pub const fn listed(self) -> &'static [&'static str] {
        match self {
            Self::Song => &[
                "set.name",
                "set.path",
                "set.unsaved_changes",
                "tempo.bpm_at_start",
                "tempo.segments",
                "time_signature_at_start",
                "scale",
                "tuning",
                "counts.tracks",
                "counts.clips",
                "counts.devices",
                "undo.next_undo",
                "undo.next_redo",
                "undo.steps",
                "undo.redo_steps",
                "undo.current",
                "scheduled",
                "checkpoints",
                "tracks",
            ],
            Self::Device => &["compare", "cv", "matrix", "wavetables"],
            Self::Track => &["linked"],
            _ => &[],
        }
    }
}

/// Where a command's effect reads back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readback {
    /// A read command, and a field in its result: dotted names, with `[]`
    /// after a list (`agents.clients[].autonomous`).
    Command {
        /// The read command's ID.
        command: &'static str,
        /// The field in its result.
        field: &'static str,
    },
    /// The query tool, and a field of the object's full result. A field not
    /// listed beside `saved` is a field of the object as the set saves it.
    Query {
        /// The object.
        object: Object,
        /// The field.
        field: &'static str,
    },
    /// Nothing to read back, and why.
    Nothing {
        /// One line: why there's nothing to read.
        reason: &'static str,
    },
}

/// Reads back from `command`'s result.
const fn read(command: &'static str, field: &'static str) -> Readback {
    Readback::Command { command, field }
}

/// Reads back from the query tool.
const fn query(object: Object, field: &'static str) -> Readback {
    Readback::Query { object, field }
}

/// Has nothing to read back.
const fn nothing(reason: &'static str) -> Readback {
    Readback::Nothing { reason }
}

/// Why the Browser's commands read back through nothing a read command gives.
const BROWSER: &str = "the Browser's state is read with `browser.state`, which is `external` \
                       because it lists files outside the project";
/// Why a command that writes a file has nothing to read.
const FILE: &str = "writes a file outside the set; the set doesn't change";

mod arrangement;
mod browser;
mod clip;
mod device;
mod session;
mod settings;
mod song;
mod track;
mod transport;

/// One row of the table: a command and where its effect reads back.
type Row = (&'static str, Readback);

/// The areas' tables, in the order [`READBACK`] lists them.
const TABLES: [&[Row]; 9] = [
    arrangement::ROWS,
    browser::ROWS,
    clip::ROWS,
    device::ROWS,
    session::ROWS,
    settings::ROWS,
    song::ROWS,
    track::ROWS,
    transport::ROWS,
];

/// The number of rows in all the tables.
const LEN: usize = {
    let (mut len, mut table) = (0, 0);
    while table < TABLES.len() {
        len += TABLES[table].len();
        table += 1;
    }
    len
};

/// The tables joined, in order.
static JOINED: [Row; LEN] = {
    let mut rows = [TABLES[0][0]; LEN];
    let (mut next, mut table) = (0, 0);
    while table < TABLES.len() {
        let mut row = 0;
        while row < TABLES[table].len() {
            rows[next] = TABLES[table][row];
            next += 1;
            row += 1;
        }
        table += 1;
    }
    rows
};

/// Every command that isn't `read`, and where its effect reads back: one
/// table per area in `readback/<area>.rs`, joined here in a fixed order.
pub const READBACK: &[(&str, Readback)] = &JOINED;

/// Whether `field` is in the schema `root`: dotted names through its
/// properties, `[]` after a list, following references and the branches of
/// `oneOf`, `anyOf` and `allOf`.
#[must_use]
pub fn has_field(root: &Value, field: &str) -> bool {
    let mut here = vec![root];
    for segment in field.split('.') {
        let (name, is_list) = segment
            .strip_suffix("[]")
            .map_or((segment, false), |name| (name, true));
        let mut next: Vec<&Value> = here
            .iter()
            .flat_map(|schema| branches(root, schema))
            .filter_map(|branch| branch.get("properties")?.get(name))
            .collect();
        if is_list {
            next = next
                .iter()
                .flat_map(|schema| branches(root, schema))
                .filter_map(|branch| branch.get("items"))
                .collect();
        }
        if next.is_empty() {
            return false;
        }
        here = next;
    }
    true
}

/// `schema` and every schema it stands for: what it references, and the
/// branches of its combinators.
fn branches<'a>(root: &'a Value, schema: &'a Value) -> Vec<&'a Value> {
    fn walk<'a>(root: &'a Value, schema: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
        if depth > 16 {
            return;
        }
        out.push(schema);
        if let Some(target) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| root.pointer(reference.strip_prefix('#')?))
        {
            walk(root, target, depth + 1, out);
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            for branch in schema
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(root, branch, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, schema, 0, &mut out);
    out
}

/// What is wrong with `table` as the readback of `registry`'s commands, one
/// line each: a command that changes something and has no entry, an entry
/// for a command that isn't registered, is `read`, or has two, a readback
/// through a command that isn't registered or isn't `read`, a field its
/// result schema lacks, a query field its object's saved form lacks (`saved`
/// gives the schema of the form: see [`Object::path`]), and an exemption
/// without a reason.
#[must_use]
pub fn problems(
    registry: &Registry,
    table: &[(&str, Readback)],
    saved: impl Fn(Object) -> Option<Value>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for (id, readback) in table {
        if !seen.insert(*id) {
            problems.push(format!("`{id}` has two readbacks"));
        }
        match registry.get(id) {
            None => problems.push(format!(
                "a readback is declared for `{id}`, which isn't registered"
            )),
            Some(spec) if spec.permission == Some(Permission::Read) => problems.push(format!(
                "a readback is declared for `{id}`, which is a read command and changes nothing"
            )),
            Some(_) => {}
        }
        match *readback {
            Readback::Command { command, field } => match registry.get(command) {
                None => problems.push(format!(
                    "`{id}` reads back from `{command}`, which isn't registered"
                )),
                Some(spec) if spec.permission != Some(Permission::Read) => problems.push(format!(
                    "`{id}` reads back from `{command}`, which isn't a read command"
                )),
                Some(spec) => {
                    let found = spec
                        .result
                        .as_ref()
                        .is_some_and(|schema| has_field(schema.as_value(), field));
                    if !found {
                        problems.push(format!(
                            "`{id}` reads back from `{field}` in `{command}`'s result, which has no such field"
                        ));
                    }
                }
            },
            Readback::Query { object, field } => {
                let found = object.listed().contains(&field)
                    || saved(object).is_some_and(|schema| has_field(&schema, field));
                if !found {
                    problems.push(format!(
                        "`{id}` reads back from `{field}` of a `{}` query, which has no such field",
                        object.path()
                    ));
                }
            }
            Readback::Nothing { reason } => {
                if reason.trim().is_empty() {
                    problems.push(format!(
                        "`{id}` has nothing to read back and gives no reason"
                    ));
                }
            }
        }
    }
    for spec in registry.iter() {
        if spec.permission != Some(Permission::Read) && !seen.contains(spec.id.as_ref()) {
            problems.push(format!(
                "`{}` changes something and names no place it reads back; add it to the readback table",
                spec.id
            ));
        }
    }
    problems
}

/// The reference's "Readback" section as the index explains it.
pub const INTRO: &str = "## Readback\n\n\
     Anything an agent can set, it can read back. Each command that isn't `read` names where \
     its effect shows: a read command and the field in its result, or the query tool's path \
     for an object and the field in its full result. A query field is one the set saves for \
     the object, or one of the song's overview. `cargo xtask commands` fails when a command \
     that changes something names none, or names a field that isn't there. Each namespace's \
     page has the rows for its commands.\n";

/// The rows of one namespace (the ID's part before its first dot): where
/// each of its commands that isn't `read` reads back, then those with
/// nothing to read back and why. Empty when it has no such command.
#[must_use]
pub fn markdown(registry: &Registry, table: &[(&str, Readback)], namespace: &str) -> String {
    let anchor = |id: &str| format!("[`{id}`](#{})", id.replace('.', ""));
    let mut reads = String::new();
    let mut nothing = String::new();
    for spec in registry.iter().filter(|spec| spec.area() == namespace) {
        let Some((_, readback)) = table.iter().find(|(id, _)| *id == spec.id) else {
            continue;
        };
        let id = anchor(&spec.id);
        match *readback {
            Readback::Command { command, field } => {
                let _ = writeln!(reads, "| {id} | `{command}` | `{field}` |");
            }
            Readback::Query { object, field } => {
                let _ = writeln!(
                    reads,
                    "| {id} | the query tool's `{}` | `{field}` |",
                    object.path()
                );
            }
            Readback::Nothing { reason } => {
                let _ = writeln!(nothing, "| {id} | {reason} |");
            }
        }
    }
    let mut out = String::new();
    if !reads.is_empty() {
        let _ = write!(
            out,
            "| Command | Read it with | Field |\n|---|---|---|\n{reads}\n"
        );
    }
    if !nothing.is_empty() {
        let _ = write!(
            out,
            "### Nothing to read back\n\n| Command | Why |\n|---|---|\n{nothing}\n"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Object::Track;
    use super::*;
    use crate::{CommandSpec, NoArgs};

    /// A registry of a setter, a read command whose result has a field
    /// `loop_on` and a list `clients` of objects with a `name`, and an action.
    fn registry() -> Registry {
        let state = crate::schemars::json_schema!({
            "type": "object",
            "properties": {
                "loop_on": { "type": "boolean" },
                "clients": { "type": "array", "items": { "$ref": "#/$defs/Client" } }
            },
            "$defs": { "Client": { "type": "object", "properties": { "name": { "type": "string" } } } }
        });
        let command = |id: &'static str, permission| {
            CommandSpec::new(id, id)
                .description("A test command.")
                .permission(permission)
                .context(crate::Context::Global)
                .args::<NoArgs>()
        };
        let mut registry = Registry::new();
        for spec in [
            command("test.state", Permission::Read).result_schema(state),
            command("test.set_loop", Permission::Edit).result::<()>(),
            command("test.quit", Permission::Destructive).result::<()>(),
        ] {
            registry.register(spec).unwrap();
        }
        registry
    }

    fn saved(object: Object) -> Option<Value> {
        (object == Object::Track).then(|| {
            json!({ "properties": { "routing": { "$ref": "#/$defs/Routing" } },
                    "$defs": { "Routing": { "properties": { "output": {} } } } })
        })
    }

    fn check(table: &[(&str, Readback)]) -> Vec<String> {
        problems(&registry(), table, saved)
    }

    #[test]
    fn a_complete_table_has_no_problems() {
        let table = [
            ("test.set_loop", read("test.state", "loop_on")),
            ("test.quit", nothing("ends the app")),
        ];
        assert_eq!(check(&table), Vec::<String>::new());
        let by_list = [
            ("test.set_loop", read("test.state", "clients[].name")),
            ("test.quit", query(Track, "routing.output")),
        ];
        assert_eq!(check(&by_list), Vec::<String>::new());
    }

    #[test]
    fn a_setter_with_no_declaration_fails() {
        let table = [("test.quit", nothing("ends the app"))];
        let found = check(&table);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("`test.set_loop` changes something"));
    }

    #[test]
    fn a_declaration_naming_a_missing_or_non_read_command_fails() {
        let table = [
            ("test.set_loop", read("test.gone", "loop_on")),
            ("test.quit", read("test.set_loop", "loop_on")),
            ("test.ghost", nothing("not registered")),
            ("test.state", nothing("reads")),
        ];
        let found = check(&table).join("\n");
        assert!(
            found.contains("`test.gone`, which isn't registered"),
            "{found}"
        );
        assert!(
            found.contains("`test.set_loop`, which isn't a read command"),
            "{found}"
        );
        assert!(
            found.contains("declared for `test.ghost`, which isn't registered"),
            "{found}"
        );
        assert!(
            found.contains("declared for `test.state`, which is a read command"),
            "{found}"
        );
    }

    #[test]
    fn a_declaration_naming_a_field_the_schema_lacks_fails() {
        let table = [
            ("test.set_loop", read("test.state", "loop_off")),
            ("test.quit", query(Track, "routing.input")),
        ];
        let found = check(&table).join("\n");
        assert!(
            found.contains("`loop_off` in `test.state`'s result"),
            "{found}"
        );
        assert!(
            found.contains("`routing.input` of a `track` query"),
            "{found}"
        );
    }

    #[test]
    fn an_exemption_needs_a_reason() {
        let table = [
            ("test.set_loop", read("test.state", "loop_on")),
            ("test.quit", nothing(" ")),
        ];
        assert!(check(&table)[0].contains("gives no reason"));
    }

    #[test]
    fn the_reference_lists_readbacks_and_exemptions() {
        let table = [
            ("test.set_loop", read("test.state", "loop_on")),
            ("test.quit", nothing("ends the app")),
        ];
        let text = markdown(&registry(), &table, "test");
        assert!(text.contains("| [`test.set_loop`](#testset_loop) | `test.state` | `loop_on` |"));
        assert!(text.contains("| [`test.quit`](#testquit) | ends the app |"));
    }

    #[test]
    fn the_table_names_each_command_once() {
        let mut ids: Vec<&str> = READBACK.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }

    #[test]
    fn discarding_a_recovery_reads_back_from_the_list_of_recoverable_sets() {
        let row = READBACK
            .iter()
            .find(|(id, _)| *id == "set.discard_recovery");
        assert_eq!(
            row.map(|(_, readback)| *readback),
            Some(read("set.list_recoverable", "sets"))
        );
    }
}
