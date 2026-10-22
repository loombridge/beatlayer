//! The themes Beatlayer can draw: the bundled one and the theme files in
//! the User Library's `Themes` folder (spec: themes R2, R9, R10).

use std::collections::BTreeMap;
use std::path::Path;

use bl_shell::Shell;
use bl_shell::settings::{BUNDLED_THEME, ThemeLook};

use crate::theme::Theme;

/// A theme: the one or two files that share a name.
struct Entry {
    light: Option<Theme>,
    dark: Option<Theme>,
}

/// The themes found, by name.
pub struct ThemeLibrary {
    bundled: Entry,
    users: BTreeMap<String, Entry>,
}

impl ThemeLibrary {
    /// Only the bundled theme.
    #[must_use]
    pub fn bundled() -> Self {
        Self {
            bundled: Entry {
                light: Some(Theme::bundled(false)),
                dark: Some(Theme::bundled(true)),
            },
            users: BTreeMap::new(),
        }
    }

    /// The bundled theme and the `.toml` files in `folder`, with a message
    /// for each file that isn't listed (R9): a file that isn't a valid
    /// theme, one that takes the bundled theme's name, and a second light
    /// or dark file of a name.
    #[must_use]
    pub fn load(folder: Option<&Path>) -> (Self, Vec<String>) {
        let mut library = Self::bundled();
        let mut problems = Vec::new();
        let mut files: Vec<_> = folder
            .and_then(|folder| std::fs::read_dir(folder).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for path in files {
            let file = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let theme = std::fs::read_to_string(&path)
                .map_err(|e| format!("can't be read: {e}"))
                .and_then(|text| Theme::parse(&text));
            let theme = match theme {
                Ok(theme) if theme.name == BUNDLED_THEME => {
                    Err(format!("the name {BUNDLED_THEME} is the bundled theme's"))
                }
                other => other,
            };
            let theme = theme.and_then(|theme| {
                let entry = library.users.entry(theme.name.clone()).or_insert(Entry {
                    light: None,
                    dark: None,
                });
                let slot = if theme.dark {
                    &mut entry.dark
                } else {
                    &mut entry.light
                };
                if slot.is_some() {
                    return Err(format!(
                        "{} already has a {} file",
                        theme.name,
                        if theme.dark { "dark" } else { "light" }
                    ));
                }
                *slot = Some(theme);
                Ok(())
            });
            if let Err(why) = theme {
                problems.push(format!("Theme file {file} isn't used: {why}"));
            }
        }
        (library, problems)
    }

    /// Reads the User Library's themes again and tells the shell which it
    /// offers; the Status Bar names each broken file, or says the chosen
    /// theme is missing and Beatlayer is drawn (R9, R10).
    pub fn refresh(&mut self, shell: &mut Shell) {
        let (library, problems) = Self::load(shell.themes_folder().as_deref());
        *self = library;
        shell.set_themes(self.names());
        let chosen = &shell.look().theme;
        if !problems.is_empty() {
            shell.set_message(problems.join(" "), true);
        } else if !self.has(chosen) {
            let message = format!("The theme {chosen} isn't there, so Beatlayer is drawn.");
            shell.set_message(message, true);
        }
    }

    /// The names the Theme chooser offers: Beatlayer, then the rest by name.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        std::iter::once(BUNDLED_THEME.to_owned())
            .chain(self.users.keys().cloned())
            .collect()
    }

    fn has(&self, name: &str) -> bool {
        name == BUNDLED_THEME || self.users.contains_key(name)
    }

    /// The colours to draw for the settings, `dark` being the appearance
    /// drawn: the chosen theme's file for it (or its only file), as the
    /// bundled theme if the chosen theme isn't found.
    #[must_use]
    pub fn resolve(&self, look: &ThemeLook, dark: bool) -> Theme {
        let entry = self.users.get(&look.theme).unwrap_or(&self.bundled);
        let (wanted, other) = if dark {
            (&entry.dark, &entry.light)
        } else {
            (&entry.light, &entry.dark)
        };
        wanted
            .as_ref()
            .or(other.as_ref())
            .expect("a theme has a file")
            .styled(look)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: &str = include_str!("../themes/dark.toml");

    fn library(tag: &str) -> (Shell, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("bl-themes-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Themes")).unwrap();
        let mut shell = Shell::headless();
        shell.set_user_library(Some(dir.clone()));
        (shell, dir)
    }

    fn user_theme(name: &str, panel: &str) -> String {
        DARK.replace("name = \"Beatlayer\"", &format!("name = \"{name}\""))
            .replace("panel = \"#22262c\"", &format!("panel = \"{panel}\""))
    }

    // Spec: themes R2, R9.
    #[test]
    fn a_good_theme_file_is_listed_and_drawn() {
        let (mut shell, dir) = library("good");
        std::fs::write(
            dir.join("Themes/ember.toml"),
            user_theme("Ember", "#301010"),
        )
        .unwrap();
        let mut themes = ThemeLibrary::bundled();
        themes.refresh(&mut shell);
        assert_eq!(shell.themes(), ["Beatlayer", "Ember"]);
        assert!(shell.message().is_none());
        shell
            .run(
                "settings.set_theme",
                serde_json::json!({ "theme": "Ember" }),
            )
            .unwrap();
        let drawn = themes.resolve(shell.look(), true);
        assert_eq!(
            drawn.colors.panel.0,
            egui::Color32::from_rgb(0x30, 0x10, 0x10)
        );
        // It has only a dark file, so it's drawn whatever the Appearance is.
        assert_eq!(themes.resolve(shell.look(), false).name, "Ember");
        std::fs::remove_file(dir.join("Themes/ember.toml")).unwrap();
        std::fs::remove_dir(dir.join("Themes")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    // Spec: themes R9.
    #[test]
    fn a_broken_theme_file_is_refused_with_its_message() {
        let (mut shell, dir) = library("broken");
        std::fs::write(dir.join("Themes/bad.toml"), user_theme("Bad", "#30101")).unwrap();
        std::fs::write(
            dir.join("Themes/typo.toml"),
            user_theme("Typo", "#301010").replace("panel =", "panle ="),
        )
        .unwrap();
        std::fs::write(
            dir.join("Themes/steal.toml"),
            user_theme("Beatlayer", "#301010"),
        )
        .unwrap();
        let mut themes = ThemeLibrary::bundled();
        themes.refresh(&mut shell);
        assert_eq!(shell.themes(), ["Beatlayer"]);
        let message = shell.message().unwrap();
        assert!(message.error);
        for part in [
            "bad.toml",
            "#30101",
            "typo.toml",
            "panle",
            "steal.toml",
            "bundled theme's",
        ] {
            assert!(message.text.contains(part), "{part} in {}", message.text);
        }
        for file in ["bad", "typo", "steal"] {
            std::fs::remove_file(dir.join(format!("Themes/{file}.toml"))).unwrap();
        }
        std::fs::remove_dir(dir.join("Themes")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    // Spec: themes R9. A theme is a light file and a dark file that share a name.
    #[test]
    fn a_theme_is_a_light_file_and_a_dark_file() {
        let (mut shell, dir) = library("pair");
        let light =
            include_str!("../themes/light.toml").replace("name = \"Beatlayer\"", "name = \"Pair\"");
        std::fs::write(
            dir.join("Themes/pair-dark.toml"),
            user_theme("Pair", "#301010"),
        )
        .unwrap();
        std::fs::write(dir.join("Themes/pair-light.toml"), light).unwrap();
        std::fs::write(
            dir.join("Themes/pair-dark2.toml"),
            user_theme("Pair", "#301011"),
        )
        .unwrap();
        let mut themes = ThemeLibrary::bundled();
        themes.refresh(&mut shell);
        assert_eq!(shell.themes(), ["Beatlayer", "Pair"]);
        assert!(shell.message().unwrap().text.contains("pair-dark2.toml"));
        shell
            .run("settings.set_theme", serde_json::json!({ "theme": "Pair" }))
            .unwrap();
        assert!(!themes.resolve(shell.look(), false).dark);
        assert!(themes.resolve(shell.look(), true).dark);
        for file in ["pair-dark", "pair-light", "pair-dark2"] {
            std::fs::remove_file(dir.join(format!("Themes/{file}.toml"))).unwrap();
        }
        std::fs::remove_dir(dir.join("Themes")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    // Spec: themes R10.
    #[test]
    fn a_missing_chosen_theme_draws_beatlayer_and_comes_back_with_its_file() {
        let (mut shell, dir) = library("missing");
        let file = dir.join("Themes/ember.toml");
        std::fs::write(&file, user_theme("Ember", "#301010")).unwrap();
        let mut themes = ThemeLibrary::bundled();
        themes.refresh(&mut shell);
        shell
            .run(
                "settings.set_theme",
                serde_json::json!({ "theme": "Ember" }),
            )
            .unwrap();
        std::fs::remove_file(&file).unwrap();
        themes.refresh(&mut shell);
        assert_eq!(shell.look().theme, "Ember", "the choice is kept");
        assert_eq!(themes.resolve(shell.look(), true).name, "Beatlayer");
        assert!(shell.message().unwrap().text.contains("Ember"));
        std::fs::write(&file, user_theme("Ember", "#301010")).unwrap();
        themes.refresh(&mut shell);
        assert_eq!(themes.resolve(shell.look(), true).name, "Ember");
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(dir.join("Themes")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
