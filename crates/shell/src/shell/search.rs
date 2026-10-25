//! The search field in the shell (WP-6.42, IMP-16 and IMP-18): the rows the
//! Commands scope lists, built from the menus, the context menus and the
//! keymap; the tracks the Tracks scope finds; and the commands that open and
//! drive the field. The behaviour is in the command palette spec, R1 to R16.
//!
//! The field is view state: nothing here is saved with the set or is an undo
//! step, except the command a row runs.

use std::collections::HashSet;

use bl_shell_spec::context_menus::{
    self, ADAPTIVE_GRID_ROWS, CHAIN_COLOUR_ROW, FIXED_GRID_ROWS, TrackMenu,
};
use bl_shell_spec::keymap::{Platform, Status, Trigger};
use bl_shell_spec::menus::{Action, MenuItem, MenuPlatform, menu_bar};
use bl_shell_spec::search::{
    self as ids, CommandRow, CommandRows, ScopeArgs, SearchState, TextArgs, TrackRow, TrackRows,
};
use serde_json::{Map, Value};

use super::*;
use crate::search::{Candidate, Field, find_tracks, keep_set, rank_commands, step};

/// How a context menu is named in a row's menu column.
const CONTEXT_MENUS: [&str; 8] = [
    "Clip context menu",
    "Audio clip context menu",
    "Time selection context menu",
    "Device context menu",
    "Rack context menu",
    "Chain context menu",
    "Track context menu",
    "Note context menu",
];

impl Shell {
    /// The search field's state, to draw it.
    #[must_use]
    pub fn search(&self) -> &Field {
        &self.search
    }

    /// Whether the search field is open.
    #[must_use]
    pub fn search_open(&self) -> bool {
        self.search.open
    }

    /// Whether the search field outlines `track` (R10): it is open on the
    /// Tracks scope, and the track matches.
    #[must_use]
    pub fn search_outlines(&self, track: TrackId) -> bool {
        self.search.outlines() && self.search.tracks.iter().any(|row| row.track == track)
    }

    /// The return tracks the mixers and the Session View show, in order:
    /// all of them, or while Show Only Matches is on the matching ones
    /// (R12).
    #[must_use]
    pub fn shown_returns(&self) -> Vec<TrackId> {
        self.editor
            .document()
            .returns()
            .filter(|id| self.arrangement.search_keeps(*id))
            .collect()
    }

    /// The row the list draws highlighted: the Commands scope's, or the
    /// Tracks scope's selected track (R8, R11).
    #[must_use]
    pub fn search_highlight(&self) -> Option<usize> {
        let field = &self.search;
        if field.scope == ids::SearchScope::Commands {
            return (!field.commands.is_empty()).then_some(field.highlight);
        }
        let selected = self.arrangement.selected_tracks().last().copied();
        field
            .tracks
            .iter()
            .position(|row| Some(row.track) == selected)
    }

    /// What the search field shows, as `search.state` reports it (R15).
    #[must_use]
    pub fn search_state(&self) -> SearchState {
        let field = &self.search;
        let commands = field.scope == ids::SearchScope::Commands;
        SearchState {
            open: field.open,
            scope: field.scope,
            text: field.text.clone(),
            highlight: self.search_highlight(),
            only_matches: field.only_matches,
            commands: if commands {
                field.commands.clone()
            } else {
                Vec::new()
            },
            tracks: if commands {
                Vec::new()
            } else {
                field.tracks.clone()
            },
        }
    }

    /// The rows the Commands scope lists for `text`, in order (R4 to R7),
    /// as `search.commands` returns them.
    #[must_use]
    pub fn search_commands(&self, text: &str) -> Vec<CommandRow> {
        rank_commands(self.command_candidates(), text)
    }

    /// Every row the Commands scope can list now, before a text picks among
    /// them and in no order (R4).
    #[doc(hidden)]
    #[must_use]
    pub fn search_all_commands(&self) -> Vec<CommandRow> {
        self.command_candidates()
            .into_iter()
            .map(|candidate| candidate.row)
            .collect()
    }

    /// The tracks the Tracks scope finds for `text` (R9), as
    /// `search.tracks` returns them.
    #[must_use]
    pub fn search_tracks(&self, text: &str) -> Vec<TrackRow> {
        find_tracks(self.editor.document(), text)
    }

    /// Chooses row `index` of the list: in the Commands scope it runs the
    /// row as its menu item or shortcut would and closes the field (R8); a
    /// row that can't run now runs nothing and the Status Bar says so (R7).
    /// In the Tracks scope it selects the row's track as Return does (R11).
    pub fn search_run_row(&mut self, index: usize) {
        if self.search.scope == ids::SearchScope::Tracks {
            if let Some(track) = self.search.tracks.get(index).map(|row| row.track) {
                // A selection can't fail for a track the set has.
                let _ = self.select_found_track(track);
                self.refresh_search();
            }
            return;
        }
        let Some(row) = self.search.commands.get(index).cloned() else {
            return;
        };
        if !row.available {
            self.set_message(format!("{} isn't available now", row.label), false);
            return;
        }
        self.search = Field::default();
        self.refresh_search();
        // Errors show in the Status Bar; `invoke` has put them there.
        let _ = self.invoke(&row.command, Value::Object(row.args));
    }

    /// Brings the rows, the matches and the hidden tracks up to date with
    /// the field's text and the set (R10, R12). Run after every command
    /// while the field is open, and once more when it closes.
    pub(super) fn refresh_search(&mut self) {
        let open = self.search.open;
        self.search.tracks = if open && self.search.scope == ids::SearchScope::Tracks {
            find_tracks(self.editor.document(), &self.search.text)
        } else {
            Vec::new()
        };
        self.search.commands = if open && self.search.scope == ids::SearchScope::Commands {
            self.search_commands(&self.search.text)
        } else {
            Vec::new()
        };
        self.search.highlight = self
            .search
            .highlight
            .min(self.search.commands.len().saturating_sub(1));
        let keep = (self.search.only_matches && !self.search.tracks.is_empty())
            .then(|| keep_set(self.editor.document(), &self.search.tracks));
        // Only a change in what is hidden moves the lanes and the columns.
        if self.arrangement.search_keep() != keep.as_ref() {
            self.arrangement.set_search_keep(keep);
            self.arrangement
                .forget_missing_tracks(self.editor.document());
            self.mixer
                .clamp_scroll(self.editor.document(), &self.arrangement);
        }
        self.touch();
    }

    /// Runs one of the search field's commands.
    pub(super) fn run_search(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::COMMANDS => {
                let TextArgs { text } = Self::args(id, args)?;
                return Ok(to_value(&CommandRows {
                    rows: self.search_commands(&text),
                }));
            }
            ids::TRACKS => {
                let TextArgs { text } = Self::args(id, args)?;
                return Ok(to_value(&TrackRows {
                    rows: self.search_tracks(&text),
                }));
            }
            ids::STATE => {
                Self::args::<NoArgs>(id, args)?;
                return Ok(to_value(&self.search_state()));
            }
            ids::OPEN => {
                let ScopeArgs { scope } = Self::args(id, args)?;
                if self.search.open {
                    self.switch_search_scope(scope);
                } else {
                    self.search = Field {
                        open: true,
                        scope,
                        ..Field::default()
                    };
                }
            }
            ids::CLOSE => {
                Self::args::<NoArgs>(id, args)?;
                self.search = Field::default();
            }
            _ => {
                if !self.search.open {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "the search field isn't open".to_owned(),
                    });
                }
                match id {
                    ids::SET_TEXT => {
                        let TextArgs { text } = Self::args(id, args)?;
                        self.search.text = text;
                        self.search.highlight = 0;
                    }
                    ids::SET_SCOPE => {
                        let ScopeArgs { scope } = Self::args(id, args)?;
                        self.switch_search_scope(scope);
                    }
                    ids::SET_ONLY_MATCHES => {
                        let SwitchArgs { on } = Self::args(id, args)?;
                        if self.search.scope != ids::SearchScope::Tracks {
                            return Err(ShellError::Unavailable {
                                command: id.to_owned(),
                                reason: "Show Only Matches belongs to the Tracks scope".to_owned(),
                            });
                        }
                        self.search.only_matches = on;
                    }
                    ids::NEXT | ids::PREVIOUS => {
                        Self::args::<NoArgs>(id, args)?;
                        self.step_search(id == ids::PREVIOUS)?;
                    }
                    other => return Err(ShellError::UnknownCommand(other.to_owned())),
                }
            }
        }
        self.refresh_search();
        Ok(to_value(&self.search_state()))
    }

    /// Switches the scope, keeping the text; Show Only Matches ends with
    /// the scope it belongs to (R12).
    fn switch_search_scope(&mut self, scope: ids::SearchScope) {
        if self.search.scope != scope {
            self.search.scope = scope;
            self.search.only_matches = false;
            self.search.highlight = 0;
        }
    }

    /// `search.next` and `search.previous`: Down and Up through the rows of
    /// the Commands scope, Return and Shift-Return through the matches of
    /// the Tracks scope (R8, R11).
    fn step_search(&mut self, backwards: bool) -> Result<(), ShellError> {
        if self.search.scope == ids::SearchScope::Commands {
            let rows = self.search.commands.len();
            if rows > 0 {
                let at = self.search.highlight;
                self.search.highlight = if backwards {
                    (at + rows - 1) % rows
                } else {
                    (at + 1) % rows
                };
            }
            return Ok(());
        }
        let selected = self.arrangement.selected_tracks().last().copied();
        let doc = self.editor.document();
        match step(doc, &self.search.tracks, selected, backwards) {
            Some(track) => self.select_found_track(track),
            None => Ok(()),
        }
    }

    /// Selects a track the Tracks scope found as clicking its title bar
    /// does, which scrolls the lanes to it, after opening any folded group
    /// that holds it (R11).
    fn select_found_track(&mut self, track: TrackId) -> Result<(), ShellError> {
        let mut parent = self.editor.document().parent(track);
        while let Some(group) = parent {
            self.arrangement.unfold(group);
            parent = self.editor.document().parent(group);
        }
        self.run_command(
            view_ids::SELECT_TRACKS,
            json!({ "tracks": [track], "title_bars": true }),
        )
        .map(|_| ())
    }

    /// Every row the Commands scope can list now, unordered and unmatched:
    /// the menu bar's items, then the context menus', then the shortcuts
    /// that only a key runs, each (command, arguments) once.
    fn command_candidates(&self) -> Vec<Candidate> {
        let platform = Platform::current();
        let contexts = self.active_contexts();
        let table = self.key_table(platform);
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut out = Vec::new();
        let mut add = |shell: &Self,
                       id: &str,
                       args: Map<String, Value>,
                       label: String,
                       menu: &str,
                       contexts: &[Context]| {
            let Some(spec) = shell.registry.get(id) else {
                return;
            };
            if !seen.insert((id.to_owned(), Value::Object(args.clone()).to_string())) {
                return;
            }
            let available = shell.can_invoke_in(id, &Value::Object(args.clone()), contexts);
            out.push(Candidate {
                row: CommandRow {
                    command: id.to_owned(),
                    keys: keys_of(&table, id, &args),
                    args,
                    label,
                    menu: menu.to_owned(),
                    available,
                },
                description: spec.description.to_string(),
            });
        };
        for menu in menu_bar(MenuPlatform::current()) {
            self.add_menu_items(&mut add, &menu.items, menu.title, &contexts, &[]);
        }
        let track_contexts = self.track_menu_contexts();
        let menus = [
            context_menus::arrangement_clip(),
            context_menus::arrangement_audio_clip(),
            context_menus::arrangement_time(),
            context_menus::device(),
            context_menus::rack(),
            context_menus::chain(),
            context_menus::track(TrackMenu {
                group: true,
                several: true,
                arrangement: true,
                time_selection: true,
                devices_off: false,
            }),
            context_menus::note(),
        ];
        // Rows that need a value the menu supplies by its layout.
        let valued: Vec<&str> = ADAPTIVE_GRID_ROWS
            .iter()
            .chain(&FIXED_GRID_ROWS)
            .chain(&[CHAIN_COLOUR_ROW])
            .copied()
            .collect();
        for (items, name) in menus.iter().zip(CONTEXT_MENUS) {
            let contexts = if name == "Track context menu" {
                &track_contexts
            } else {
                &contexts
            };
            self.add_menu_items(&mut add, items, name.to_owned(), contexts, &valued);
        }
        for entry in self.keymap.entries() {
            if entry.status != Status::Bound
                || entry.kind != "key"
                || entry.trigger == Trigger::Held
                || entry.bindings(platform).is_empty()
            {
                continue;
            }
            let command = contexts
                .iter()
                .find_map(|context| {
                    entry
                        .commands
                        .iter()
                        .find(|command| command.contexts.contains(context))
                })
                .or(entry.commands.first());
            if let Some(command) = command {
                let menu = humanise(&entry.context);
                add(
                    self,
                    &command.id,
                    command.args.clone(),
                    entry.action.clone(),
                    &menu,
                    &contexts,
                );
            }
        }
        out
    }

    /// Adds the rows of `items` and of their submenus, skipping the rows in
    /// `skip`.
    fn add_menu_items(
        &self,
        add: &mut impl FnMut(&Self, &str, Map<String, Value>, String, &str, &[Context]),
        items: &[MenuItem],
        path: impl Into<String>,
        contexts: &[Context],
        skip: &[&str],
    ) {
        let path = path.into();
        for item in items.iter().filter(|item| item.is_shown()) {
            if skip.contains(&item.row) {
                continue;
            }
            let label = |id: &str| {
                self.dynamic_label(id)
                    .unwrap_or_else(|| item.label.to_owned())
            };
            match item.action {
                Action::Command(id) => add(self, id, Map::new(), label(id), &path, contexts),
                Action::Choice(id, args) => {
                    if let Ok(Value::Object(args)) = serde_json::from_str(args) {
                        add(self, id, args, label(id), &path, contexts);
                    }
                }
                Action::Shortcut(shortcuts) => {
                    let keymap = &self.keymap;
                    let command = shortcuts
                        .iter()
                        .find_map(|shortcut| {
                            keymap.command_for_running(shortcut, contexts, |command| {
                                self.can_invoke_in(
                                    &command.id,
                                    &Value::Object(command.args.clone()),
                                    contexts,
                                )
                            })
                        })
                        .or_else(|| {
                            shortcuts.iter().find_map(|shortcut| {
                                let entry = keymap.entry_for(shortcut)?;
                                (entry.status == Status::Bound)
                                    .then(|| entry.commands.first())
                                    .flatten()
                            })
                        });
                    if let Some(command) = command {
                        add(
                            self,
                            &command.id,
                            command.args.clone(),
                            label(&command.id),
                            &path,
                            contexts,
                        );
                    }
                }
                Action::Submenu => {
                    let path = format!("{path} ▸ {}", item.label);
                    self.add_menu_items(add, &item.items, path, contexts, skip);
                }
                Action::Pending(_) | Action::System(_) | Action::Omitted(_) | Action::Separator => {
                }
            }
        }
    }

    /// The keys the keymap binds on `platform`, with the commands and the
    /// arguments they run.
    fn key_table(&self, platform: Platform) -> Vec<(&str, &Map<String, Value>, &[String])> {
        self.keymap
            .entries()
            .iter()
            .filter(|entry| entry.status == Status::Bound && entry.kind == "key")
            .flat_map(|entry| {
                entry.commands.iter().map(move |command| {
                    (command.id.as_str(), &command.args, entry.bindings(platform))
                })
            })
            .collect()
    }
}

/// The keys bound to a command with these arguments.
fn keys_of(
    table: &[(&str, &Map<String, Value>, &[String])],
    id: &str,
    args: &Map<String, Value>,
) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (_, _, bindings) in table
        .iter()
        .filter(|(command, fixed, _)| *command == id && *fixed == args)
    {
        for binding in *bindings {
            if !keys.contains(binding) {
                keys.push(binding.clone());
            }
        }
    }
    keys
}

/// A context's ID as a menu column reads it: `track-headers` is `Track
/// Headers`.
fn humanise(context: &str) -> String {
    context
        .split('-')
        .map(|word| {
            let mut letters = word.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
