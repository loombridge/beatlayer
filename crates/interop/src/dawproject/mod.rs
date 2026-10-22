//! DAWproject import (WP-6.36; the DAWproject spec, R1 to R43): reading a
//! `.dawproject` file and opening what it holds as a set.
//!
//! The format is an open file format for moving a song between music
//! applications: a ZIP archive holding the project as XML, optional
//! metadata, the plug-ins' saved states, and the audio files. The reader
//! takes the file as far as it can be taken and says what it left out; the
//! importer turns what it read into a set and a report of everything it
//! couldn't carry.
//!
//! Nothing here panics on a damaged or foreign file.

pub mod container;
pub mod import;
pub mod model;
pub mod read;
pub mod report;

pub use container::{Container, Error, read_container};
pub use import::{Context, Extracted, Imported, import};
pub use model::Project;
pub use read::ReadReport;
pub use report::{Entry, Report};
