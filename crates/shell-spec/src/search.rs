//! The search field's commands (WP-6.42, IMP-16 and IMP-18): opening and
//! closing it, its text and scope, moving through what it finds, Show Only
//! Matches, and the two reads that give an agent what a person would see.
//! The behaviour is in the command palette spec, R1 to R16.
//!
//! The field is view state, so every command is in the read class: it
//! changes nothing in the set and is no undo step. Selecting a track from
//! the Tracks scope is a selection, as clicking its title bar is.
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them
//! by ID.

use bl_commands::Context::{Global, TextField};
use bl_commands::{Binding, CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::commands::SwitchArgs;

/// `search.open`: open the search field.
pub const OPEN: &str = "search.open";
/// `search.close`: close the search field.
pub const CLOSE: &str = "search.close";
/// `search.set_text`: type into the search field.
pub const SET_TEXT: &str = "search.set_text";
/// `search.set_scope`: switch the search field's scope.
pub const SET_SCOPE: &str = "search.set_scope";
/// `search.next`: Down in the Commands scope, Return in the Tracks scope.
pub const NEXT: &str = "search.next";
/// `search.previous`: Up in the Commands scope, Shift-Return in the Tracks
/// scope.
pub const PREVIOUS: &str = "search.previous";
/// `search.set_only_matches`: switch Show Only Matches.
pub const SET_ONLY_MATCHES: &str = "search.set_only_matches";
/// `search.state`: what the search field shows.
pub const STATE: &str = "search.state";
/// `search.commands`: the rows the Commands scope lists for a text.
pub const COMMANDS: &str = "search.commands";
/// `search.tracks`: the tracks the Tracks scope lists for a text.
pub const TRACKS: &str = "search.tracks";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    OPEN,
    CLOSE,
    SET_TEXT,
    SET_SCOPE,
    NEXT,
    PREVIOUS,
    SET_ONLY_MATCHES,
    STATE,
    COMMANDS,
    TRACKS,
];

/// Whether `id` is one of the search field's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What the search field looks for (R2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SearchScope {
    /// The commands a menu item, a context-menu item or a shortcut runs.
    #[default]
    Commands,
    /// The tracks whose name, or a device's name, matches.
    Tracks,
}

/// Arguments of `search.open` and `search.set_scope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopeArgs {
    /// `commands` or `tracks`.
    pub scope: SearchScope,
}

/// Arguments of `search.set_text`, `search.commands` and `search.tracks`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextArgs {
    /// What was typed.
    pub text: String,
}

/// A row the Commands scope lists (R4 and R5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandRow {
    /// The command the row runs, by its ID.
    pub command: String,
    /// The arguments the row fixes.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub args: Map<String, Value>,
    /// The item's label as the menu shows it, or the shortcut's action.
    pub label: String,
    /// The menu or context the row belongs to, such as `Edit` or
    /// `View ▸ Mixer Controls`.
    pub menu: String,
    /// The keys the keymap binds to it now, as the shortcut reference writes
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Whether it can run now (R7).
    pub available: bool,
}

/// A track the Tracks scope lists (R9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackRow {
    /// The track.
    pub track: TrackId,
    /// Its name.
    pub name: String,
    /// The name of a device on it that matched, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Whether the track's own name matched.
    pub by_name: bool,
}

/// What `search.commands` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandRows {
    /// The rows, in the order the list shows them.
    pub rows: Vec<CommandRow>,
}

/// What `search.tracks` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackRows {
    /// The matching tracks, in display order.
    pub rows: Vec<TrackRow>,
}

/// What the search field shows (R15).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchState {
    /// Whether the field is open.
    pub open: bool,
    /// Its scope.
    pub scope: SearchScope,
    /// The text in it.
    pub text: String,
    /// The index of the highlighted row in `commands`, or of the selected
    /// track in `tracks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<usize>,
    /// Whether Show Only Matches is on.
    pub only_matches: bool,
    /// The rows of the Commands scope, empty in the other.
    pub commands: Vec<CommandRow>,
    /// The rows of the Tracks scope, empty in the other.
    pub tracks: Vec<TrackRow>,
}

/// Adds the search field's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(OPEN, "Search Commands")
            .description(
                "Opens the search field over the main view with the text empty, on the Commands \
                 scope (`commands`: the commands a menu item, a context-menu item or a shortcut \
                 runs) or the Tracks scope (`tracks`: tracks found by their name or by the name \
                 of a device on them). Help > Search Commands... and Cmd-Shift-P (Ctrl-Shift-P) \
                 open the first; Navigate > Find Track and Cmd-Shift-O (Ctrl-Shift-O) the \
                 second. With the field open it switches the scope and keeps the text. The \
                 field is view state: it isn't saved in the set and it isn't an undo step.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .binding(Binding::new("S-MENU-007").arg("scope", "commands"))
            .binding(Binding::new("S-FOCUS-015").arg("scope", "tracks"))
            .args::<ScopeArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(CLOSE, "Close Search")
            .description(
                "Closes the search field, removes the track outlines and shows every track \
                 again. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .args::<NoArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_TEXT, "Type in Search")
            .description(
                "Sets the text of the open search field, as typing does. The Commands scope \
                 then lists the rows `search.commands` gives for it, with the first highlighted; \
                 the Tracks scope outlines the tracks `search.tracks` gives. Changes nothing in \
                 the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .args::<TextArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SCOPE, "Set Search Scope")
            .description(
                "Switches the open search field to the Commands or the Tracks scope and keeps \
                 its text. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .args::<ScopeArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(NEXT, "Search Next")
            .description(
                "In the Tracks scope, selects the next matching track after the selected one in \
                 display order (the first when none is selected, wrapping from the last), as \
                 clicking its title bar would, and scrolls the view in front to it, unfolding \
                 any group that holds it: Return in the field. In the Commands scope, moves the \
                 highlight down a row, wrapping. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .args::<NoArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(PREVIOUS, "Search Previous")
            .description(
                "The reverse of `search.next`: in the Tracks scope, selects the previous \
                 matching track (Shift-Return in the field); in the Commands scope, moves the \
                 highlight up a row, wrapping. Changes nothing in the set.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .args::<NoArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(SET_ONLY_MATCHES, "Show Only Matches")
            .description(
                "Switches Show Only Matches in the Tracks scope. While it's on and the text \
                 matches a track, the Arrangement's lanes, the Session View's columns and both \
                 mixers show only the matching tracks, the groups that hold them unfolded, and \
                 Main. The other tracks play, record and take edits as before. It's view state \
                 that ends with the search: it is off each time the field opens.",
            )
            .permission(Permission::Read)
            .contexts([Global, TextField])
            .args::<SwitchArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(STATE, "Search State")
            .description(
                "What the search field shows: whether it is open, its scope, its text, the \
                 highlighted row, whether Show Only Matches is on, and the rows it lists, each \
                 with its command ID and the arguments it fixes, or its track ID.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<SearchState>(),
    )?;
    registry.register(
        CommandSpec::new(COMMANDS, "Find Commands")
            .description(
                "The rows the Commands scope lists for `text`, in the order it lists them: every \
                 command a menu item, a context-menu item or a shortcut runs, once, with the \
                 arguments that item fixes, its label, the menu or context it belongs to, the \
                 keys bound to it now and whether it can run now. A word matches when it starts \
                 a word of the label or the menu, or appears in the command's description; \
                 rows that can't run now come last. Works with the field closed, and runs \
                 nothing: run a row by running its `command` with its `args`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<TextArgs>()
            .result::<CommandRows>(),
    )?;
    registry.register(
        CommandSpec::new(TRACKS, "Find Tracks")
            .description(
                "The tracks the Tracks scope lists for `text`, in display order: those with the \
                 text, ignoring case, in their name or in the name or own name of a device on \
                 them at any depth, plug-ins included. Each row says whether the track's name \
                 matched and which device did. Works with the field closed, and changes nothing.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<TextArgs>()
            .result::<TrackRows>(),
    )?;
    Ok(())
}
