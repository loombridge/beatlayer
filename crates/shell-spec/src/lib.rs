//! The app shell's commands and menu bar as data (WP-1.7): what `bl-shell`
//! runs and the app draws, without the audio host, files, or windows.
//!
//! - [`commands`]: the shell's commands in the command registry: files
//!   (new, open, save, quit), windows and views, keyboard focus, the
//!   Settings window, and the audio device, with their argument and result
//!   types.
//! - [`menus`]: the menu bar in Live's order, each item tied to the command
//!   it runs.
//! - [`keymap`]: Live 12's shortcuts as data, and the person's own
//!   shortcuts layered over them (IMP-17).
//! - [`audio`] and [`settings`]: the data those commands take and return.
//! - [`agent`]: the agent server's settings and commands (WP-1.15).
//! - [`arrangement`]: the Arrangement View's view commands (WP-1.8a): zoom,
//!   scroll, Follow, the editing grid, track heights, and the selection
//!   the zoom commands work from.
//! - [`editing`]: the Arrangement's editing commands (WP-1.8b): selecting,
//!   moving, copying, resizing, splitting, deleting, and duplicating clips,
//!   and the clipboard; [`context_menus`], the clip and time-selection
//!   menus that reach them.
//! - [`automation`]: the Arrangement's automation editing commands
//!   (WP-6.25): moving the breakpoints of an envelope, the clipboard of
//!   envelope shapes, and the edit commands that act on them.
//! - [`groove`]: the grooves' commands that need the shell (WP-6.19): loading
//!   and saving groove files, extracting, committing, a groove made into a
//!   clip, and the pool query.
//! - [`locator`]: the Arrangement's locators (WP-6.24): adding, deleting,
//!   moving, renaming and describing them, looping to the next one, the
//!   song start time, and the jumps between them.
//! - [`notes`]: the Clip View's note-editing commands (WP-2.3): selecting
//!   the MIDI Note Editor's notes, drawing, deleting, moving and resizing
//!   them.
//! - [`browser`]: the browser's commands (WP-1.10): places, browsing,
//!   search, preview, and adding files to the Arrangement.
//! - [`mixer_view`]: the Mixer view's view commands (WP-1.9): showing and
//!   hiding the mixer and its sections in both views, scrolling it, and
//!   moving the focus to it.
//! - [`devices`]: the Device View's commands (WP-2.4): what it shows,
//!   selecting and folding devices, adding them, the device clipboard,
//!   presets, and hot-swapping; [`context_menus`] holds the device title
//!   bar's menu.
//! - [`multisampler`]: the multisampler's commands that need the shell
//!   (WP-5.13): adding layers from files, Snap, Normalize, and its views.
//! - [`pads`]: a drum rack's pad view and the commands on pads (WP-5.6).
//! - [`tracks`]: Duplicate Track Without Clips (WP-2.19b, IMP-12).
//! - [`session_launch`]: the global launch quantisation and its enabled
//!   switch (WP-3.2), which the Control Bar's Quantization Menu shows.
//! - [`transport`]: what the Control Bar's transport section shows
//!   (`transport.state`), and the IDs of the transport's commands, which
//!   `bl-engine` and `bl-model` register (WP-1.18).
//! - [`export`]: exporting audio (`export.audio`, WP-1.12).
//! - [`file_manager`]: the File Manager's commands (WP-6.63): showing it,
//!   what it lists, replacing a file, and the set's backups and copies.
//! - [`bounce`]: bouncing tracks and pasting bounced audio (`track.bounce_*`,
//!   WP-4.15).
//! - [`midi_files`]: importing and exporting Standard MIDI Files
//!   (`midi.import_file`, `midi.export_clip`, WP-2.21).
//! - [`midi_io`]: MIDI ports and the computer MIDI keyboard (`midi.ports`,
//!   `midi.set_port`, `midi.computer_keyboard_*`, WP-2.2).
//! - [`modulation`]: what a track's clips modulate (`modulation.state`,
//!   WP-5.9).
//! - [`perception`]: how agents hear and see the set (`perception.*`,
//!   WP-1.16): audio previews, analysis, and view captures.
//! - [`warping`]: a clip's transients (`clip.transients`, WP-4.3).
//! - [`sample_editor`]: the Sample Editor's selections and view (WP-4.10).
//! - [`audio_editing`]: Crop, Consolidate, and Consolidate Time to New Scene
//!   (`clip.crop`, `arrangement.consolidate*`, WP-4.13).
//! - [`audio_panel`]: the Audio Utilities panel's own commands
//!   (`clip.edit_sample`, `clip.replace_sample`, WP-4.10b).
//!
//! `cargo xtask` checks the commands, lists them in the command reference, and
//! builds the keymap from this crate, so none of its tasks links an audio
//! backend (which needs ALSA and JACK headers on Linux). `bl-shell`
//! re-exports everything here.

pub mod agent;
pub mod arrangement;
pub mod assistant;
pub mod audio;
pub mod audio_editing;
pub mod audio_panel;
pub mod automation;
pub mod autosampler;
pub mod bounce;
pub mod browser;
pub mod capture_midi;
pub mod commands;
pub mod context_menus;
pub mod dawproject;
pub mod devices;
pub mod editing;
pub mod export;
pub mod file_manager;
pub mod freeze;
pub mod groove;
pub mod keymap;
pub mod locator;
pub mod menus;
pub mod midi_files;
pub mod midi_io;
pub mod mixer_view;
pub mod modulation;
pub mod multisampler;
pub mod notes;
pub mod pads;
pub mod perception;
pub mod plugins;
pub mod sample_editor;
pub mod scales;
pub mod session_launch;
pub mod session_record;
pub mod session_view;
pub mod settings;
pub mod slice;
pub mod stems;
pub mod tracks;
pub mod transport;
pub mod warping;

use bl_commands::{RegisterError, Registry};

/// Adds the shell's commands, the Arrangement View's (its view and its
/// editing commands), the transport query, the Mixer view's, the browser's,
/// the export command, and the perception commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    commands::register(registry)?;
    arrangement::register(registry)?;
    editing::register(registry)?;
    automation::register(registry)?;
    groove::register(registry)?;
    locator::register(registry)?;
    notes::register(registry)?;
    transport::register(registry)?;
    mixer_view::register(registry)?;
    browser::register(registry)?;
    export::register(registry)?;
    file_manager::register(registry)?;
    midi_files::register(registry)?;
    dawproject::register(registry)?;
    midi_io::register(registry)?;
    devices::register(registry)?;
    multisampler::register(registry)?;
    autosampler::register(registry)?;
    tracks::register(registry)?;
    session_view::register(registry)?;
    session_launch::register(registry)?;
    session_record::register(registry)?;
    capture_midi::register(registry)?;
    plugins::register(registry)?;
    settings::register(registry)?;
    modulation::register(registry)?;
    perception::register(registry)?;
    stems::register(registry)?;
    sample_editor::register(registry)?;
    audio_editing::register(registry)?;
    audio_panel::register(registry)?;
    freeze::register(registry)?;
    bounce::register(registry)?;
    slice::register(registry)?;
    warping::register(registry)?;
    scales::register(registry)
}
