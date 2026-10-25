//! Tunings in the browser (WP-6.10; the tunings spec, R25 to R27): Beatlayer's
//! own, shipped inside the app, and the text the Info View shows for one.

use std::path::Path;

use bl_interop::scala;
use bl_model::Tuning;

/// What a bundled tuning's path starts with; the rest is its name.
pub const BUNDLED_PREFIX: &str = "tunings:/";

macro_rules! tuning {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../../tunings/", $name, ".ascl")),
        )
    };
}

/// Beatlayer's own tunings, in the order the Tunings label lists them: the
/// `.ascl` files `tests/fixtures/gen/gen_bundled_tunings.py` writes (R31).
pub const BUNDLED: &[(&str, &str)] = &[
    tuning!("5 Equal Steps"),
    tuning!("7 Equal Steps"),
    tuning!("10 Equal Steps"),
    tuning!("12 Equal Steps"),
    tuning!("15 Equal Steps"),
    tuning!("16 Equal Steps"),
    tuning!("17 Equal Steps"),
    tuning!("19 Equal Steps"),
    tuning!("22 Equal Steps"),
    tuning!("24 Equal Steps"),
    tuning!("31 Equal Steps"),
    tuning!("41 Equal Steps"),
    tuning!("53 Equal Steps"),
    tuning!("Bohlen-Pierce"),
    tuning!("Harmonics 8 to 16"),
    tuning!("Just Intonation 5-Limit"),
    tuning!("Just Intonation 7-Limit"),
    tuning!("Kirnberger III"),
    tuning!("Pythagorean"),
    tuning!("Quarter-Comma Meantone"),
    tuning!("Vallotti"),
    tuning!("Werckmeister III"),
    tuning!("Young"),
];

/// The path of the bundled tuning called `name`.
#[must_use]
pub fn bundled_path(name: &str) -> String {
    format!("{BUNDLED_PREFIX}{name}")
}

/// Reads the tuning at `path`: a bundled tuning's path, or a `.scl` or `.ascl`
/// file.
///
/// # Errors
///
/// [`scala::ScalaError`] if the file can't be read or breaks the tunings
/// spec's R3.
pub fn read(path: &str) -> Result<Tuning, scala::ScalaError> {
    match path.strip_prefix(BUNDLED_PREFIX) {
        Some(name) => {
            let (_, text) = BUNDLED
                .iter()
                .find(|(own, _)| *own == name)
                .ok_or_else(|| {
                    scala::ScalaError::Unreadable(format!("no bundled tuning is called {name}"))
                })?;
            scala::parse(text.as_bytes(), name)
        }
        None => scala::read(Path::new(path)),
    }
}

/// What the Info View shows for a tuning (R26): its name, and its
/// description, the notes it holds to the pseudo-octave and the
/// pseudo-octave's size in cents, or the error a file R3 refuses gives.
#[must_use]
pub fn info(path: &str) -> (String, String) {
    let name = path.strip_prefix(BUNDLED_PREFIX).map_or_else(
        || {
            Path::new(path)
                .file_stem()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        },
        str::to_owned,
    );
    (name, info_body(read(path)))
}

/// The Info View's lines for a tuning that was read, or the error.
#[must_use]
pub fn info_body(read: Result<Tuning, scala::ScalaError>) -> String {
    match read {
        Ok(tuning) => body(&tuning),
        Err(error) => error.to_string(),
    }
}

/// The Info View's lines for a tuning.
#[must_use]
pub fn body(tuning: &Tuning) -> String {
    let mut lines = Vec::new();
    if !tuning.description.is_empty() {
        lines.push(tuning.description.clone());
    }
    lines.push(format!("{} notes to the pseudo-octave", tuning.degrees()));
    lines.push(format!(
        "Pseudo-octave: {:.2} cents",
        tuning.pseudo_octave()
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: tunings R25, R31.
    #[test]
    fn every_file_in_the_tunings_folder_is_bundled_and_reads() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tunings");
        let mut files: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let mut bundled: Vec<String> = BUNDLED.iter().map(|(n, _)| format!("{n}.ascl")).collect();
        bundled.sort();
        assert_eq!(files, bundled);
        for (name, _) in BUNDLED {
            let tuning = read(&bundled_path(name)).unwrap();
            assert_eq!(tuning.name, *name);
        }
    }

    // Spec: tunings R26.
    #[test]
    fn the_info_names_the_notes_and_the_pseudo_octave() {
        let (name, body) = info(&bundled_path("19 Equal Steps"));
        assert_eq!(name, "19 Equal Steps");
        assert_eq!(
            body,
            "19 equal steps to the octave\n19 notes to the pseudo-octave\nPseudo-octave: 1200.00 cents"
        );
        let (_, body) = info("/nowhere/Missing.scl");
        assert!(body.starts_with("the tuning file can't be read"), "{body}");
    }
}
