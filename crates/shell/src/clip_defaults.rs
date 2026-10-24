//! A file's saved default clip settings (WP-4.9, the warping spec R42): a
//! small file of Beatlayer's own beside the sample, so the settings travel
//! with it. Written only by Save Default Clip; read when a clip is made from
//! the file. Beatlayer never reads or writes Live's `.asd` files.

use std::fs;
use std::path::{Path, PathBuf};

use bl_model::{ClipDefaults, ContentHash};
use serde::{Deserialize, Serialize};

/// The format version of the file. Version 1 left Loop Forward, then Beats'
/// default Transient Loop Mode, out of the settings.
const VERSION: u32 = 2;

/// What is added to the sample's file name to name its defaults file.
const SUFFIX: &str = ".beatlayer-clip";

/// The defaults file as it is written: the settings with the content they
/// were saved for, so a changed sample is never shown its old settings.
#[derive(Serialize, Deserialize)]
struct DefaultsFile {
    version: u32,
    media: ContentHash,
    defaults: ClipDefaults,
}

/// Where `sample`'s defaults are kept: beside it, as `<file name>.beatlayer-clip`.
#[must_use]
pub fn path_for(sample: &Path) -> PathBuf {
    let mut name = sample.file_name().unwrap_or_default().to_owned();
    name.push(SUFFIX);
    sample.with_file_name(name)
}

/// The defaults saved beside `sample`, if there is a file that is whole, of
/// this format version and made for the content `media`.
#[must_use]
pub fn load(sample: &Path, media: ContentHash) -> Option<ClipDefaults> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(path_for(sample)).ok()?).ok()?;
    if value.get("version")? == 1 {
        value
            .pointer_mut("/defaults/warp_settings")?
            .as_object_mut()?
            .entry("beats")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()?
            .entry("transient_loop_mode")
            .or_insert_with(|| serde_json::json!("loop_forward"));
    }
    let file: DefaultsFile = serde_json::from_value(value).ok()?;
    (matches!(file.version, 1 | VERSION) && file.media == media).then_some(file.defaults)
}

/// Saves `defaults` beside `sample`, replacing any saved before, and returns
/// the file's path.
///
/// # Errors
///
/// Why the file couldn't be written, naming it. The folder is never swapped
/// for another place.
pub fn save(sample: &Path, media: ContentHash, defaults: ClipDefaults) -> Result<PathBuf, String> {
    let path = path_for(sample);
    let json = serde_json::to_vec_pretty(&DefaultsFile {
        version: VERSION,
        media,
        defaults,
    })
    .map_err(|error| error.to_string())?;
    // A temporary file renamed over the old one, so a failed write leaves the
    // defaults saved before.
    let temporary = path.with_extension("beatlayer-clip.tmp");
    fs::write(&temporary, json)
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!("couldn't write {}: {error}", path.display())
        })?;
    Ok(path)
}
