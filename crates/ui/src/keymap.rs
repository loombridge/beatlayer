//! The keymap the UI matches keys against: the keymap as data (Live 12's
//! shortcuts, and the person's own layered over them) with its keys
//! resolved to the keys the UI reports.
//!
//! The data lives in the shell's [`bl_shell::keymap`], so the shell and an
//! agent can read and change the keymap without a UI toolkit; this module
//! is the thin layer that draws it and matches keys against it.

use bl_commands::Context;
use bl_shell::keymap as data;
use egui::{Key, Modifiers};

pub use bl_shell::keymap::{
    Bind, BindArgs, Hit, KeyCommand, KeymapState, LATCH_HOLD_SECONDS, Mods, Origin, Platform,
    PlatformKeys, ResetArgs, ShortcutState, SkippedEntry, Status, Trigger, Unbind, UnbindArgs,
    UserKeymap,
};

/// The generated keymap file, embedded so the app can't start without it.
pub use bl_shell::keymap::LIVE12;

/// A key chord with the key the UI reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    /// The key.
    pub key: Key,
    /// The modifiers held with it.
    pub mods: Mods,
    /// The chord's place in a range such as `F1..F8`, counting from 1.
    pub index: Option<u8>,
}

impl Chord {
    /// Whether a key press matches this chord. `Plus` also matches the `=`
    /// key that carries it on most keyboards.
    #[must_use]
    pub fn matches(&self, key: Key, mods: Mods) -> bool {
        let key_matches = self.key == key || (self.key == Key::Plus && key == Key::Equals);
        key_matches && self.mods == mods
    }
}

/// One shortcut in the keymap, with its keys as the UI reports them.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    inner: data::Entry,
    mac_chords: Vec<Chord>,
    win_chords: Vec<Chord>,
}

impl std::ops::Deref for Entry {
    type Target = data::Entry;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Entry {
    /// The key chords for a platform (empty for gestures).
    #[must_use]
    pub fn chords(&self, platform: Platform) -> &[Chord] {
        match platform {
            Platform::Mac => &self.mac_chords,
            Platform::Other => &self.win_chords,
        }
    }
}

/// The keymap the UI matches keys against.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    inner: data::Keymap,
    entries: Vec<Entry>,
}

impl Keymap {
    /// The default keymap, `keymaps/live12.toml`.
    #[must_use]
    pub fn live12() -> Self {
        Self::from_data(data::Keymap::live12())
    }

    /// Reads a keymap file.
    ///
    /// # Errors
    ///
    /// A message if it isn't a valid keymap.
    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(Self::from_data(data::Keymap::parse(text)?))
    }

    /// The keymap as data, with the person's own shortcuts layered over
    /// it.
    #[must_use]
    pub fn from_data(keymap: data::Keymap) -> Self {
        let entries = keymap
            .entries()
            .iter()
            .map(|entry| Entry {
                inner: entry.clone(),
                mac_chords: chords_of(entry.chords(Platform::Mac)),
                win_chords: chords_of(entry.chords(Platform::Other)),
            })
            .collect();
        Self {
            inner: keymap,
            entries,
        }
    }

    /// Every entry, in the reference's order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry for a shortcut ID.
    #[must_use]
    pub fn entry_for(&self, shortcut: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == shortcut)
    }

    /// The command a key press runs, given the contexts that apply,
    /// narrowest first: the first context in which a bound shortcut with
    /// these keys runs a command decides.
    #[must_use]
    pub fn resolve(
        &self,
        platform: Platform,
        key: Key,
        mods: Mods,
        contexts: &[Context],
    ) -> Option<Hit<'_>> {
        self.inner.resolve(platform, key.name(), mods, contexts)
    }

    /// [`Keymap::resolve`], where a context's first shortcut that `runs`
    /// decides, and the first one at all if none does.
    #[must_use]
    pub fn resolve_with(
        &self,
        platform: Platform,
        key: Key,
        mods: Mods,
        contexts: &[Context],
        runs: impl Fn(&Hit<'_>) -> bool,
    ) -> Option<Hit<'_>> {
        self.inner
            .resolve_with(platform, key.name(), mods, contexts, runs)
    }

    /// The command a shortcut runs in the first of `contexts` where it
    /// runs one, as a menu item reached through its shortcut needs.
    #[must_use]
    pub fn command_for(&self, shortcut: &str, contexts: &[Context]) -> Option<&KeyCommand> {
        self.inner.command_for(shortcut, contexts)
    }

    /// The first key chord that runs a command, for showing beside it in a
    /// menu.
    #[must_use]
    pub fn chord_for_command(&self, command: &str, platform: Platform) -> Option<&Chord> {
        self.entries.iter().find_map(|entry| {
            entry
                .commands
                .iter()
                .any(|c| c.id == command)
                .then(|| entry.chords(platform).first())
                .flatten()
        })
    }

    /// A shortcut's first key chord, for showing beside a menu item.
    #[must_use]
    pub fn chord_for_shortcut(&self, shortcut: &str, platform: Platform) -> Option<&Chord> {
        self.entry_for(shortcut)?.chords(platform).first()
    }
}

/// The keys the UI reports, by the name the keymap holds them under.
fn egui_keys() -> &'static std::collections::HashMap<&'static str, Key> {
    static KEYS: std::sync::OnceLock<std::collections::HashMap<&'static str, Key>> =
        std::sync::OnceLock::new();
    KEYS.get_or_init(|| Key::ALL.iter().map(|key| (key.name(), *key)).collect())
}

/// The named chords as the keys the UI reports, the ones it doesn't report
/// left out.
fn chords_of(named: &[data::Chord]) -> Vec<Chord> {
    let keys = egui_keys();
    named
        .iter()
        .filter_map(|chord| {
            Some(Chord {
                key: *keys.get(chord.key.as_str())?,
                mods: chord.mods,
                index: chord.index,
            })
        })
        .collect()
}

/// Parses one binding as shortcuts.toml writes it (`Cmd+Shift+S`, `F1..F8`)
/// into chords, or `None` if it isn't a key chord this UI can match (a
/// mouse gesture, or a key egui doesn't report, such as Menu).
#[must_use]
pub fn parse_binding(binding: &str, platform: Platform) -> Option<Vec<Chord>> {
    Some(chords_of(&data::parse_binding(binding, platform)?))
}

/// The keymap in use, read the first time a view shows a key from it, and
/// read again when the person's own shortcuts change (R10).
#[must_use]
pub fn keymap_in_use<'a>(
    keymap: &'a mut Option<Keymap>,
    revision: &mut u64,
    shell: &bl_shell::Shell,
) -> &'a Keymap {
    if *revision != shell.keymap_revision() {
        *keymap = Some(Keymap::from_data(shell.keymap().clone()));
        *revision = shell.keymap_revision();
    }
    keymap.as_ref().expect("the keymap")
}

/// The modifiers egui reports, as a platform names them.
#[must_use]
pub fn mods_from_egui(modifiers: Modifiers, platform: Platform) -> Mods {
    Mods {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        cmd: platform == Platform::Mac && modifiers.mac_cmd,
    }
}

/// The same modifiers as egui reports them.
#[must_use]
pub fn mods_to_egui(mods: Mods, platform: Platform) -> Modifiers {
    let mac_cmd = platform == Platform::Mac && mods.cmd;
    Modifiers {
        alt: mods.alt,
        ctrl: mods.ctrl,
        shift: mods.shift,
        mac_cmd,
        command: if platform == Platform::Mac {
            mac_cmd
        } else {
            mods.ctrl
        },
    }
}

/// How a key press reads as a binding, as the shortcut reference writes
/// keys: `Cmd+Shift+S`.
#[must_use]
pub fn binding_text(key: Key, mods: Mods, platform: Platform) -> String {
    let mut parts = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.cmd {
        parts.push("Cmd");
    }
    if mods.alt {
        parts.push(if platform == Platform::Mac {
            "Option"
        } else {
            "Alt"
        });
    }
    if mods.shift {
        parts.push("Shift");
    }
    parts.push(data::spelling(key.name()));
    parts.join("+")
}

/// Whether a key is a modifier, and so isn't the key a binding names.
#[must_use]
pub fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// How a chord reads in a menu drawn by Beatlayer: `Ctrl+Shift+S`, or on
/// macOS with the system's symbols, `⇧⌘S`.
#[must_use]
pub fn chord_text(chord: &Chord, platform: Platform) -> String {
    let key = match chord.key {
        Key::Plus => "+".to_owned(),
        Key::Minus => "-".to_owned(),
        Key::Comma => ",".to_owned(),
        Key::Period => ".".to_owned(),
        Key::Slash => "/".to_owned(),
        other => other.symbol_or_name().to_owned(),
    };
    match platform {
        Platform::Mac => {
            let mut text = String::new();
            for (on, symbol) in [
                (chord.mods.ctrl, "⌃"),
                (chord.mods.alt, "⌥"),
                (chord.mods.shift, "⇧"),
                (chord.mods.cmd, "⌘"),
            ] {
                if on {
                    text.push_str(symbol);
                }
            }
            text + &key
        }
        Platform::Other => {
            let mut parts = Vec::new();
            for (on, name) in [
                (chord.mods.ctrl, "Ctrl"),
                (chord.mods.alt, "Alt"),
                (chord.mods.shift, "Shift"),
            ] {
                if on {
                    parts.push(name.to_owned());
                }
            }
            parts.push(key);
            parts.join("+")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_parse_per_platform() {
        let save = parse_binding("Cmd+Shift+S", Platform::Mac).unwrap();
        assert_eq!(save[0].key, Key::S);
        assert!(save[0].mods.cmd && save[0].mods.shift && !save[0].mods.ctrl);
        assert!(parse_binding("Cmd+S", Platform::Other).is_none());
        let alt = parse_binding("Ctrl+Alt+B", Platform::Other).unwrap();
        assert!(alt[0].mods.ctrl && alt[0].mods.alt);
        let keys = parse_binding("F1..F8", Platform::Mac).unwrap();
        assert_eq!(keys.len(), 8);
        assert_eq!((keys[7].key, keys[7].index), (Key::F8, Some(8)));
        assert_eq!(parse_binding("0..9", Platform::Other).unwrap().len(), 10);
        assert_eq!(
            parse_binding("Fn+Left", Platform::Mac).unwrap()[0].key,
            Key::Home
        );
        assert!(parse_binding("Menu", Platform::Other).is_none());
        assert!(parse_binding("Shift+click", Platform::Other).is_none());
    }

    #[test]
    fn plus_matches_the_equals_key() {
        let chord = &parse_binding("Cmd+Plus", Platform::Mac).unwrap()[0];
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        assert!(chord.matches(Key::Equals, cmd));
        assert!(chord.matches(Key::Plus, cmd));
        assert!(!chord.matches(Key::Minus, cmd));
    }

    #[test]
    fn chords_read_as_each_platform_writes_them() {
        let mac = &parse_binding("Cmd+Shift+S", Platform::Mac).unwrap()[0];
        assert_eq!(chord_text(mac, Platform::Mac), "⇧⌘S");
        let win = &parse_binding("Ctrl+Alt+B", Platform::Other).unwrap()[0];
        assert_eq!(chord_text(win, Platform::Other), "Ctrl+Alt+B");
        let comma = &parse_binding("Cmd+Comma", Platform::Mac).unwrap()[0];
        assert_eq!(chord_text(comma, Platform::Mac), "⌘,");
    }

    #[test]
    fn modifiers_round_trip_through_egui() {
        for platform in [Platform::Mac, Platform::Other] {
            let mods = Mods {
                ctrl: true,
                alt: false,
                shift: true,
                cmd: platform == Platform::Mac,
            };
            assert_eq!(mods_from_egui(mods_to_egui(mods, platform), platform), mods);
        }
    }

    #[test]
    fn every_key_the_keymap_names_is_one_egui_reports() {
        // The shell names the keys the way the UI does, so every name in
        // the keymap resolves to a key the UI reports.
        let keymap = Keymap::live12();
        for entry in keymap.entries() {
            for platform in [Platform::Mac, Platform::Other] {
                assert_eq!(
                    entry.chords(platform).len(),
                    entry.inner.chords(platform).len(),
                    "{}: {platform:?}",
                    entry.id
                );
            }
        }
    }
}
