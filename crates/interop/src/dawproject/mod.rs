//! DAWproject import and export (WP-6.36, WP-6.37; the DAWproject spec, R1 to
//! R82): reading a `.dawproject` file and opening what it holds as a set, and
//! writing a set as a file.
//!
//! The format is an open file format for moving a song between music
//! applications: a ZIP archive holding the project as XML, optional
//! metadata, the plug-ins' saved states, and the audio files. The reader
//! takes the file as far as it can be taken and says what it left out; the
//! importer turns what it read into a set and a report of everything it
//! couldn't carry. The exporter turns a document into the same model, writes
//! it as XML in the order the schema gives, and says what the format can't
//! carry.
//!
//! Nothing here panics on a damaged or foreign file.

pub mod container;
pub mod export;
pub mod generic;
pub mod import;
pub mod model;
pub mod plugins;
pub mod read;
pub mod report;
pub mod write;

pub use container::{
    Container, Error, Limits, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, folded, read_container,
    read_container_from, read_container_strict, read_container_with_limits,
};
pub use export::{ExportContext, Exported, MediaFiles, export, export_to};
pub use import::{Context, DefaultColours, Extracted, Imported, NewColours, import};
pub use model::Project;
pub use plugins::{Installed, NoPlugins, Plugins};
pub use read::ReadReport;
pub use report::{Entry, Report};
