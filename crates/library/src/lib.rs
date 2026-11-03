//! Beatlayer's library as data (spec: core-library R1 to R4, R11, R12): the
//! manifest under `library/`, the licence checks, the packages built from it
//! and the credits of their items. `xtask` builds the packages and
//! `bl-shell` reads the installed ones, so the format has one definition.

mod build;
mod manifest;
mod package;

pub use build::{Built, build};
pub use manifest::{
    ALLOWED_LICENCES, Attribution, ItemDef, Kind, LABELS, Manifest, PackageDef, Source, sha256_hex,
};
pub use package::{
    Item, PACKAGE_FILE, Package, Reference, credits_text, licence_link, reference_of,
};
