//! Perception (WP-1.16): hearing and seeing the set.
//!
//! - `preview` renders a range of the set, the whole mix or one track, and
//!   returns it as audio the client can play to a model: an `audio`
//!   content block, or an embedded `resource` for clients of protocol
//!   2024-11-05, which has no audio blocks. It's also a resource,
//!   `beatlayer://preview/<id>.wav`.
//! - `analyze` measures a preview or a range as numbers with units.
//! - `capture_view` captures one of the app's views as a PNG image. The
//!   app's window draws it, so the call waits while the host captures the
//!   next frame ([`crate::Agent::take_capture_requests`]); headless there's
//!   no window, and it fails, saying so.
//!
//! Each runs its `perception.*` command on the shell, so all three are in
//! the read class: they change nothing and never ask the person.

use bl_commands::Permission;
use bl_shell::perception_commands::{
    self as ids, AnalyzeArgs, CAPTURE_SIZE, CaptureArgs, PreviewArgs,
};
use serde_json::Value;

use lspk_agent::Media;

use super::{Ctx, ToolOutcome, args, done, failed, step, tool_definition};
use crate::server::{Capture, Deferred, Render, RenderKind};

pub(crate) const PREVIEW: &str = "preview";
pub(crate) const ANALYZE: &str = "analyze";
pub(crate) const CAPTURE: &str = "capture_view";

pub(crate) fn preview_definition() -> Value {
    tool_definition::<PreviewArgs>(
        PREVIEW,
        "Preview audio",
        "Renders a short audio preview of the set and returns it as a WAV file you can listen \
         to, with what was rendered as JSON. The range is `start` (ticks, or bar.beat.sixteenth \
         such as \"9.1.1\") and `length` (ticks) if given, otherwise the Arrangement's time \
         selection, otherwise the whole set; it plays straight through, without the loop, and \
         is cut to its first 30 seconds (the result says when). `track` previews one track as \
         it sounds soloed; left out, the whole mix. The render is offline and doesn't stop or \
         disturb playback. 16-bit stereo at 48 kHz is about 190 KB a second: pass a lower \
         `sample_rate`, such as 22050, for a smaller file. The result's `id` names the preview \
         for `analyze`, and its `uri` reads it again as a resource; the last 4 are kept.",
        true,
    )
}

pub(crate) fn analyze_definition() -> Value {
    tool_definition::<AnalyzeArgs>(
        ANALYZE,
        "Analyze audio",
        "Measures audio and returns numbers with units, and a short text summary: integrated, \
         momentary and short-term loudness in LUFS, loudness range in LU, and true peak in \
         dBTP (ITU-R BS.1770-4, EBU R 128); sample peak, RMS, and crest factor in dB; the \
         level in each octave band from 31.5 Hz to 16 kHz; the tempo in BPM; the key; and \
         when transients start, in seconds and as bar.beat.sixteenth. Give `preview` (an ID \
         from the preview tool) to measure what you heard, or a range (`start`, `length`, \
         `track`, as for preview) to render one for the analysis, up to ten minutes. \
         `measure` picks what to measure; left out, everything. Tempo and key are estimates \
         from the audio, not the set's settings: a tempo may read as half or double, and a key \
         as its relative major or minor (see `runner_up`).",
        true,
    )
}

pub(crate) fn capture_definition() -> Value {
    tool_definition::<CaptureArgs>(
        CAPTURE,
        "Capture a view",
        "Captures one of Beatlayer's views as a PNG image, as the person sees it on screen: \
         the whole `window`, the `arrangement` or `session` view, the `mixer`, the `browser`, \
         the `detail` area (Clip or Device View), or the `settings` window. The view must be \
         showing; if it isn't, the error says which command shows it. The image is scaled down \
         to fit `max_size` pixels on its longer side (1568 unless you say). Only the running \
         app can capture: headless (`bl-cli mcp`) there's no window, and the call fails. \
         Changes nothing.",
        true,
    )
}

pub(crate) fn preview(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let parsed: PreviewArgs = step!(args(raw));
    step!(ctx.permit(&[Permission::Read], &[ids::PREVIEW], String::new));
    if ctx.view_capture && !bl_shell::plugins::has_clap(ctx.shell.editor().document()) {
        return ToolOutcome::Defer(Deferred::render(Render {
            kind: RenderKind::Preview(parsed),
            document: ctx.shell.editor().document().clone(),
            media: ctx.shell.media_store(),
            selection: ctx.shell.time_selection(),
            engine_rate: ctx.shell.engine_rate(),
            playing: ctx.shell.is_playing(),
        }));
    }
    let value = serde_json::to_value(&parsed).unwrap_or_default();
    let result = match ctx.shell.run(ids::PREVIEW, value) {
        Ok(result) => result,
        Err(error) => return failed(error.to_string()),
    };
    let Some(preview) = result["id"]
        .as_u64()
        .and_then(|id| ctx.shell.previews().get(id))
    else {
        return failed("the preview was made but isn't kept");
    };
    match preview.wav() {
        Ok(bytes) => ToolOutcome::WithMedia(
            result,
            vec![Media {
                mime_type: "audio/wav",
                bytes,
                uri: Some(ids::preview_uri(preview.id)),
            }],
        ),
        Err(message) => failed(message),
    }
}

pub(crate) fn analyze(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let parsed: AnalyzeArgs = step!(args(raw));
    step!(ctx.permit(&[Permission::Read], &[ids::ANALYZE], String::new));
    if parsed.preview.is_none()
        && ctx.view_capture
        && !bl_shell::plugins::has_clap(ctx.shell.editor().document())
    {
        return ToolOutcome::Defer(Deferred::render(Render {
            kind: RenderKind::Analyze(parsed),
            document: ctx.shell.editor().document().clone(),
            media: ctx.shell.media_store(),
            selection: ctx.shell.time_selection(),
            engine_rate: ctx.shell.engine_rate(),
            playing: ctx.shell.is_playing(),
        }));
    }
    let value = serde_json::to_value(&parsed).unwrap_or_default();
    match ctx.shell.run(ids::ANALYZE, value) {
        Ok(result) => done(result),
        Err(error) => failed(error.to_string()),
    }
}

pub(crate) fn capture(ctx: &mut Ctx<'_>, raw: Value) -> ToolOutcome {
    let parsed: CaptureArgs = step!(args(raw));
    step!(ctx.permit(&[Permission::Read], &[ids::CAPTURE_VIEW], String::new));
    let (default, smallest, largest) = CAPTURE_SIZE;
    let max_size = parsed.max_size.unwrap_or(default);
    if !(smallest..=largest).contains(&max_size) {
        return failed(format!(
            "a `max_size` of {max_size} pixels; give {smallest} to {largest}"
        ));
    }
    ToolOutcome::Defer(Deferred::Capture(Capture {
        view: parsed.view,
        max_size,
    }))
}
