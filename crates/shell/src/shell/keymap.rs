//! The keymap in use, and the person's own shortcuts behind it (the
//! custom-shortcuts spec, R1 to R17).
//!
//! The keymap the shell matches keys against is Live 12's, with the
//! person's own shortcuts layered over it: `keymap.toml`, beside the
//! preferences, holds the binds and unbinds, and the file is written at
//! once whenever it changes (R11).

use super::{Shell, ShellError};
use crate::keymap::{
    Bind, BindArgs, Chord, Keymap, KeymapResult, KeymapState, Platform, Unbind, UserKeymap,
};
use bl_commands::Context;

impl Shell {
    /// The keymap in use: Live 12's shortcuts with the person's own layered
    /// over them (R3).
    #[must_use]
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// The number of times the keymap changed, so a host knows to rebuild
    /// the keymap it matches keys against.
    #[must_use]
    pub fn keymap_revision(&self) -> u64 {
        self.keymap_revision
    }

    /// The person's own shortcuts, as `keymap.toml` holds them.
    #[must_use]
    pub fn user_keymap(&self) -> &UserKeymap {
        &self.user_keymap
    }

    /// The keymap in use, as `keymap.state` reports it (R14).
    #[must_use]
    pub fn keymap_state(&self) -> KeymapState {
        let mut state = self.keymap.state();
        state.skipped = self.user_keymap.skipped();
        state
    }

    /// Reads the person's own keymap file and layers it over Live's. The
    /// entries the file holds but can't use are skipped, and the Status
    /// Bar says how many (R6).
    pub(super) fn load_user_keymap(&mut self) {
        let default = Keymap::live12();
        let mut user = match self.config_dir.as_deref() {
            Some(dir) => match crate::settings::load_keymap(dir) {
                Ok(user) => user,
                Err(error) => {
                    self.set_message(error, true);
                    UserKeymap::default()
                }
            },
            None => UserKeymap::default(),
        };
        user.validate(&default, &self.registry);
        let skipped = user.skipped().len();
        self.keymap = default.layered(&user, &self.registry);
        self.user_keymap = user;
        self.keymap_revision += 1;
        if skipped > 0 {
            self.set_message(
                format!("{skipped} of your shortcuts couldn't be used"),
                false,
            );
        }
    }

    /// Layers the person's own shortcuts over Live's again, and writes the
    /// file (R11).
    pub(super) fn rebuild_keymap(&mut self) {
        self.keymap = Keymap::live12().layered(&self.user_keymap, &self.registry);
        self.keymap_revision += 1;
        self.save_user_keymap();
    }

    /// Writes the person's own keymap file.
    pub(super) fn save_user_keymap(&mut self) {
        let Some(dir) = self.config_dir.clone() else {
            return;
        };
        if let Err(error) = crate::settings::save_keymap(&dir, &self.user_keymap) {
            self.set_message(error, true);
        }
    }

    /// What the keymap commands report: how many entries the person's file
    /// holds, and how many of them can't be used.
    pub(super) fn keymap_result(&self) -> KeymapResult {
        KeymapResult {
            entries: self.user_keymap.binds.len() + self.user_keymap.unbinds.len(),
            skipped: self.user_keymap.skipped().len(),
        }
    }

    /// Gives a command other keys (R15).
    pub(super) fn bind_keys(&mut self, id: &str, args: &BindArgs) -> Result<(), ShellError> {
        let BindArgs {
            command,
            args: fixed,
            keys,
            platform,
            context,
            replace,
        } = args;
        let command = command.as_str();
        let fixed = fixed.clone();
        let keys = keys.as_slice();
        let platform = *platform;
        let context = context.as_deref();
        let replace = *replace;
        if !self.registry.contains(command) {
            return Err(ShellError::UnknownCommand(command.to_owned()));
        }
        if keys.is_empty() {
            return Err(invalid(id, "the binding has no keys"));
        }
        let mut chords: Vec<Chord> = Vec::new();
        for binding in keys {
            let Some(parsed) = crate::keymap::parse_binding(binding, platform) else {
                return Err(invalid(id, &format!("`{binding}` isn't a key")));
            };
            chords.extend(parsed);
        }
        let context = match context {
            Some(context) => context.to_owned(),
            None => self.keymap.context_of(command),
        };
        if Context::from_id(&context).is_none() {
            return Err(invalid(id, &format!("`{context}` isn't a context")));
        }
        if crate::keymap::refuses_context(&context) {
            return Err(invalid(
                id,
                &format!("`{context}` takes no shortcut of your own"),
            ));
        }
        if self.keymap.refuses_keys(&chords, &context, platform) {
            return Err(invalid(id, "those keys are Live's own"));
        }
        if let Some((other, other_context)) =
            self.keymap.conflict(&chords, &context, command, platform)
        {
            if !replace {
                return Err(invalid(
                    id,
                    &format!("the keys already run `{other}` in {other_context}"),
                ));
            }
            self.take_keys_away(&other, &other_context, &chords, platform);
        }
        // The person's own binding for this command in this context: the
        // keys replace the ones it had on this platform.
        let mut bind = Bind {
            command: command.to_owned(),
            args: fixed,
            mac: Vec::new(),
            win: Vec::new(),
            context: Some(context.clone()),
            skipped: None,
        };
        match platform {
            Platform::Mac => bind.mac = keys.to_vec(),
            Platform::Other => bind.win = keys.to_vec(),
        }
        match self.user_keymap.bind_index_for(command, &context) {
            Some(index) => {
                let existing = &mut self.user_keymap.binds[index];
                match platform {
                    Platform::Mac => existing.mac = bind.mac,
                    Platform::Other => existing.win = bind.win,
                }
                if !existing.args.is_empty() {
                    existing.args = bind.args;
                }
            }
            None => self.user_keymap.binds.push(bind),
        }
        self.rebuild_keymap();
        Ok(())
    }

    /// Takes a shortcut's keys away (R15): the person's own binding for
    /// it, or the default shortcut behind it.
    pub(super) fn unbind_keys(
        &mut self,
        id: &str,
        shortcut: Option<&str>,
        command: Option<&str>,
        keys: &[String],
        platform: Option<Platform>,
    ) -> Result<(), ShellError> {
        if let Some(shortcut) = shortcut {
            if self.keymap.entry_for(shortcut).is_none() {
                return Err(invalid(id, &format!("no shortcut is called `{shortcut}`")));
            }
            self.user_keymap.unbinds.push(Unbind {
                shortcut: shortcut.to_owned(),
                platform,
                skipped: None,
            });
            self.rebuild_keymap();
            return Ok(());
        }
        let Some(command) = command else {
            return Err(invalid(id, "name a shortcut or a command"));
        };
        if !self.registry.contains(command) {
            return Err(ShellError::UnknownCommand(command.to_owned()));
        }
        let platform = platform.unwrap_or(Platform::current());
        let chords: Vec<Chord> = keys
            .iter()
            .filter_map(|binding| crate::keymap::parse_binding(binding, platform))
            .flatten()
            .collect();
        let Some(context) = self.context_of(command, &chords, platform) else {
            return Err(invalid(id, "the command has no keys to take away"));
        };
        self.take_keys_away(command, &context, &chords, platform);
        self.rebuild_keymap();
        Ok(())
    }

    /// The context a command's keys run in.
    pub(super) fn context_of(
        &self,
        command: &str,
        chords: &[Chord],
        platform: Platform,
    ) -> Option<String> {
        self.keymap
            .entries()
            .iter()
            .find(|entry| {
                entry.commands.iter().any(|c| c.id == command)
                    && (chords.is_empty()
                        || entry
                            .chords(platform)
                            .iter()
                            .any(|chord| chords.iter().any(|key| key.same(chord))))
            })
            .map(|entry| entry.context.clone())
    }

    /// Takes the keys away from the command they run, so another command
    /// can have them: the person's own binding loses them, or a default
    /// shortcut is unbound.
    pub(super) fn take_keys_away(
        &mut self,
        command: &str,
        context: &str,
        chords: &[Chord],
        platform: Platform,
    ) {
        if let Some(index) = self.user_keymap.bind_index_for(command, context) {
            let bind = &mut self.user_keymap.binds[index];
            match platform {
                Platform::Mac => bind.mac.clear(),
                Platform::Other => bind.win.clear(),
            }
            if bind.mac.is_empty() && bind.win.is_empty() {
                self.user_keymap.binds.remove(index);
            }
            return;
        }
        if let Some(shortcut) = self.keymap.shortcut_for(command, chords, platform) {
            let shortcut = shortcut.to_owned();
            self.user_keymap.unbinds.push(Unbind {
                shortcut,
                platform: Some(platform),
                skipped: None,
            });
        }
    }

    /// Gives a command Live's keys back, or every command's (R15).
    pub(super) fn reset_keys(&mut self, command: Option<&str>) {
        match command {
            Some(command) => {
                let shortcuts: Vec<String> = self
                    .keymap
                    .entries()
                    .iter()
                    .filter(|entry| entry.commands.iter().any(|c| c.id == command))
                    .map(|entry| entry.id.clone())
                    .collect();
                self.user_keymap
                    .binds
                    .retain(|bind| bind.command != command);
                self.user_keymap
                    .unbinds
                    .retain(|unbind| !shortcuts.contains(&unbind.shortcut));
            }
            None => {
                self.user_keymap.binds.clear();
                self.user_keymap.unbinds.clear();
            }
        }
        self.rebuild_keymap();
    }
}

/// An error about a command's arguments.
fn invalid(id: &str, message: &str) -> ShellError {
    ShellError::InvalidArgs {
        command: id.to_owned(),
        message: message.to_owned(),
    }
}
