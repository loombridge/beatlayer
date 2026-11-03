//! A built package's own manifest, and the credits of its items (spec:
//! core-library R3, R11, R12).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::manifest::{Attribution, Kind};

/// The package manifest's name in a package: the main file the Packages
/// manager installs, listing every item with its file's SHA-256.
pub const PACKAGE_FILE: &str = "library-package.json";

/// An item of a built package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Its ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: Kind,
    /// The content label it lists under in the browser.
    pub label: String,
    /// Its file, relative to the package's folder.
    pub file: String,
    /// Its file's SHA-256, in hex.
    pub sha256: String,
    /// Its file's size in bytes.
    pub size: u64,
    /// Its licence.
    pub licence: String,
    /// Its credit.
    pub attribution: Attribution,
    /// Its tags, each `Group|Tag` or `Group|Tag|Subtag`, with its Source tag.
    pub tags: Vec<String>,
}

/// A built package, as `library-package.json` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    /// Its ID in the manifest.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Its items, in ID order.
    pub items: Vec<Item>,
}

impl Package {
    /// Reads the package manifest in an installed package's `dir`.
    ///
    /// # Errors
    ///
    /// A message if it isn't there or isn't a package manifest.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let file = dir.join(PACKAGE_FILE);
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))
    }
}

/// The address of a licence's text.
#[must_use]
pub fn licence_link(licence: &str) -> &'static str {
    match licence {
        "CC0-1.0" => "https://creativecommons.org/publicdomain/zero/1.0/",
        "CC-BY-4.0" => "https://creativecommons.org/licenses/by/4.0/",
        _ => "",
    }
}

impl Item {
    /// Its credit as one line: title, creator, licence and its link, the
    /// source's link, and the changes made (R11).
    #[must_use]
    pub fn credit(&self) -> String {
        let a = &self.attribution;
        let mut credit = format!("\u{201c}{}\u{201d} by {}", a.title, a.creator);
        credit.push_str(&format!(
            ", {} ({})",
            self.licence.replace('-', " "),
            licence_link(&self.licence)
        ));
        if !a.link.is_empty() {
            credit.push_str(&format!(", {}", a.link));
        }
        if !a.changes.is_empty() {
            credit.push_str(&format!(". Changes: {}", a.changes));
        }
        credit
    }
}

/// The text Export Credits writes (R12): each package's name, then its
/// items' credits, one a line.
#[must_use]
pub fn credits_text<'a>(packages: impl IntoIterator<Item = (&'a str, &'a [Item])>) -> String {
    let mut text = String::new();
    for (name, items) in packages {
        if items.is_empty() {
            continue;
        }
        text.push_str(&format!("{name}\n"));
        for item in items {
            text.push_str(&format!("  {}\n", item.credit()));
        }
        text.push('\n');
    }
    text
}

/// Which library item a file in the library's folder is (R14): a set refers
/// to the items it uses by the place their package installs them, which
/// names the package, its version and the item, beside the file's content
/// hash its media table holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// The package's ID in the Packages manager, such as `library-core`.
    pub package: String,
    /// Its version.
    pub version: String,
    /// The item's ID.
    pub item: String,
}

/// The item a path names, if it's laid out as an installed item is:
/// `library-<part>-<version>/<item>/<file>` at its end.
#[must_use]
pub fn reference_of(path: &Path) -> Option<Reference> {
    let mut parts = path
        .components()
        .rev()
        .filter_map(|c| c.as_os_str().to_str());
    let (_file, item, folder) = (parts.next()?, parts.next()?, parts.next()?);
    let (package, version) = folder.rsplit_once('-')?;
    package.starts_with("library-").then(|| Reference {
        package: package.to_owned(),
        version: version.to_owned(),
        item: item.to_owned(),
    })
}
