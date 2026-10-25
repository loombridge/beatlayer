//! The colours of what the shell makes new (the themes spec, R19, R20, R23).

use bl_model::{ClipId, Command, EditError, Editor, TrackId};
use bl_shell_spec::settings::ThemeLook;

/// Colours the tracks and clips a command just made, inside its undo step:
/// each track as Auto-Assign Track Colors, Default Track Color and Reduced
/// Automatic Colors say, then each clip as Clip Color says, so a clip that
/// takes its track's colour takes the new track's. A new track already has
/// swatch `id mod 14` and a new clip its track's, so with the default
/// settings this changes nothing.
///
/// # Errors
///
/// [`EditError`] if the document refuses a colour.
pub fn colour_new(
    editor: &mut Editor,
    look: &ThemeLook,
    tracks: &[TrackId],
    clips: &[ClipId],
) -> Result<(), EditError> {
    for &track in tracks {
        let colour = look.new_track_colour(track);
        if editor
            .document()
            .track(track)
            .is_some_and(|held| held.colour() != colour)
        {
            editor.apply(Command::SetTrackColour {
                tracks: vec![track],
                colour,
            })?;
        }
    }
    for &clip in clips {
        let doc = editor.document();
        let colour = look.new_clip_colour(clip).or_else(|| {
            doc.track(doc.clip_track(clip)?)
                .map(bl_model::Track::colour)
        });
        if let Some(colour) = colour
            && doc
                .clip(clip)
                .is_some_and(|held| held.colour != Some(colour))
        {
            editor.apply(Command::SetClipColour {
                clips: vec![clip],
                colour,
            })?;
        }
    }
    Ok(())
}
