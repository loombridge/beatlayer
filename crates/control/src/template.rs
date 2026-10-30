//! Controller templates: Beatlayer's declarative surface files
//! (`docs/specs/control-surfaces.md`, R39 to R48).
//!
//! A template is data, read on the main thread and never run (R48): this
//! module parses a file, checks the rules of R44 and lists the templates
//! found, the bundled one and the User Library's. The surface a template
//! makes is `template_surface.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_model::RelativeEncoding;
use serde::Deserialize;
use toml::Spanned;

use crate::surface::{Kind, RingStep, Role};
use crate::template_surface::TemplateSurface;

/// The only format this build reads (R39).
pub const FORMAT: u32 = 1;

/// The end of a template file's name (R39).
pub const EXTENSION: &str = ".bltemplate.toml";

/// The bundled template, Generic 8 + 8 (R46).
pub const BUNDLED: &str = include_str!("../templates/generic-8-8.bltemplate.toml");

/// The keys a template file uses, for the tests that check the bundled
/// template comments each one (R46).
pub const KEYS: &[&str] = &[
    "format",
    "name",
    "ring_width",
    "ring_height",
    "lock",
    "ports",
    "id",
    "message",
    "channel",
    "number",
    "kind",
    "role",
    "press",
    "release",
    "encoding",
    "accelerated",
    "feedback",
    "off",
    "on",
    "flash",
    "value",
    "bytes",
];

/// The role words a control's `role` starts with, for the same test (R40).
pub const ROLES: &[&str] = &[
    "slot",
    "scene",
    "track-stop",
    "track-mute",
    "track-solo",
    "track-arm",
    "track-select",
    "stop-all-clips",
    "volume",
    "pan",
    "send",
    "main-volume",
    "device",
    "next-bank",
    "previous-bank",
    "next-device",
    "previous-device",
    "ring-left",
    "ring-right",
    "ring-up",
    "ring-down",
    "page-left",
    "page-right",
    "page-up",
    "page-down",
    "play",
    "stop",
    "record",
    "session-record",
    "loop",
    "metronome",
    "tap-tempo",
    "tempo",
    "undo",
    "redo",
    "back-to-arrangement",
];

/// One thing wrong in a template file, and the line it is on (R44).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateError {
    /// The line, from 1.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

/// The message a control sends (R40).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A note, on a channel from 0.
    Note { channel: u8, key: u8 },
    /// A controller.
    Cc { channel: u8, controller: u8 },
    /// A 14-bit controller pair: `controller` and `controller + 32`.
    Cc14 { channel: u8, controller: u8 },
    /// Pitch bend.
    PitchBend { channel: u8 },
}

/// How a control's messages read (R40).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reads {
    /// A button: `press` on press and `release` on release for a
    /// controller, a note-on and a note-off for a note.
    Button { press: u8, release: u8 },
    /// A toggle: each message says whether it is on.
    Toggle,
    /// An absolute control.
    Absolute,
    /// A relative control of one of the four relative types.
    Relative {
        encoding: RelativeEncoding,
        accelerated: bool,
    },
}

/// What a control does (R40): a role of the shell's, or a command the
/// surface runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// A control of the ring, the banks or the mixer: the shell works out
    /// what it reaches (R6).
    Role(Role),
    /// The device after or before the one in focus, run through
    /// `device.select`.
    NextDevice,
    /// See [`Act::NextDevice`].
    PreviousDevice,
    /// `transport.play`.
    Play,
    /// `transport.stop`.
    Stop,
    /// `transport.set_recording`.
    Record,
    /// `session.record`.
    SessionRecord,
    /// `transport.set_loop_enabled`.
    Loop,
    /// `transport.set_metronome_enabled`.
    Metronome,
    /// `transport.tap_tempo`.
    TapTempo,
    /// `song.set_tempo`, from an absolute control.
    Tempo,
    /// `edit.undo`.
    Undo,
    /// `edit.redo`.
    Redo,
    /// `session.back_to_arrangement`.
    BackToArrangement,
}

/// What a control's lamp or fader shows (R41).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// The values for off, on and, where the controller has one, flashing.
    State { off: u8, on: u8, flash: Option<u8> },
    /// The control's value, as a motorised fader or a ring shows it.
    Value,
}

/// One control of a template (R40).
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// The control's own ID.
    pub id: String,
    /// The message it sends.
    pub source: Source,
    /// How its messages read.
    pub kind: Reads,
    /// What it does.
    pub act: Act,
    /// What its lamp or fader shows, if it has one.
    pub feedback: Option<Feedback>,
}

/// A controller template that parsed and breaks no rule (R44).
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Its name.
    pub name: String,
    /// The ring's width in tracks; zero for a controller with no ring.
    pub ring_width: u8,
    /// The ring's height in scenes; zero for no clip launching.
    pub ring_height: u8,
    /// Whether a device can be locked to it (R27).
    pub lock: bool,
    /// Text the names of the ports it is for contain (R39).
    pub ports: Vec<String>,
    /// Its controls with a role.
    pub controls: Vec<Control>,
    /// Sent when the surface starts (R41).
    pub setup: Vec<Vec<u8>>,
    /// Sent when it stops.
    pub goodbye: Vec<Vec<u8>>,
    /// Sent by Dump.
    pub dump: Vec<Vec<u8>>,
}

impl Template {
    /// The template's ID in the chooser and in `control_surface.set` (R47).
    #[must_use]
    pub fn id(&self) -> String {
        format!("template:{}", self.name)
    }

    /// Whether the template names a port whose name is `port` (R45).
    #[must_use]
    pub fn matches(&self, port: &str) -> bool {
        self.ports.iter().any(|text| port.contains(text.as_str()))
    }

    /// The surface the chooser offers for it (R43).
    #[must_use]
    pub fn kind(self: &Arc<Self>) -> Kind {
        let ring = (self.ring_width > 0).then_some(crate::surface::Ring {
            tracks: self.ring_width,
            scenes: self.ring_height,
        });
        let needs_output = !(self.setup.is_empty()
            && self.goodbye.is_empty()
            && self.dump.is_empty()
            && self.controls.iter().all(|c| c.feedback.is_none()));
        let template = Arc::clone(self);
        Kind::new(
            self.id(),
            self.name.clone(),
            needs_output,
            self.lock,
            ring,
            self.dump.clone(),
            move || Box::new(TemplateSurface::new(Arc::clone(&template))),
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(rename = "format")]
    _format: Spanned<u32>,
    surface: SurfaceTable,
    #[serde(default)]
    control: Vec<Spanned<ControlTable>>,
    #[serde(default)]
    setup: Vec<Spanned<BytesTable>>,
    #[serde(default)]
    goodbye: Vec<Spanned<BytesTable>>,
    #[serde(default)]
    dump: Vec<Spanned<BytesTable>>,
}

/// Only the format is read at first, so a file of a later format is refused
/// by its format even where its other tables differ.
#[derive(Deserialize)]
struct Head {
    format: Spanned<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceTable {
    name: String,
    #[serde(default)]
    ring_width: u8,
    #[serde(default)]
    ring_height: u8,
    #[serde(default)]
    lock: bool,
    #[serde(default)]
    ports: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlTable {
    id: String,
    message: Spanned<String>,
    channel: Spanned<u8>,
    number: Option<Spanned<u8>>,
    kind: Spanned<String>,
    role: Option<Spanned<String>>,
    press: Option<u8>,
    release: Option<u8>,
    encoding: Option<Spanned<String>>,
    accelerated: Option<bool>,
    feedback: Option<FeedbackTable>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedbackTable {
    off: Option<u8>,
    on: Option<u8>,
    flash: Option<u8>,
    value: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BytesTable {
    bytes: Spanned<String>,
}

/// The line of byte `at` in `text`, from 1.
fn line_of(text: &str, at: usize) -> usize {
    text.as_bytes()[..at.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// Parses a template file and checks R44's rules.
///
/// # Errors
///
/// Every problem found, each with its line; a file that isn't TOML, or
/// asks for a later format, gives one.
pub fn parse(text: &str) -> Result<Template, Vec<TemplateError>> {
    let at = |offset: usize, message: String| TemplateError {
        line: line_of(text, offset),
        message,
    };
    let head: Head = toml::from_str(text)
        .map_err(|e| vec![at(e.span().map_or(0, |s| s.start), e.message().to_owned())])?;
    if *head.format.get_ref() != FORMAT {
        return Err(vec![at(
            head.format.span().start,
            format!(
                "the file is format {}, and this version of Beatlayer reads format {FORMAT}",
                head.format.get_ref()
            ),
        )]);
    }
    let file: File = toml::from_str(text)
        .map_err(|e| vec![at(e.span().map_or(0, |s| s.start), e.message().to_owned())])?;
    let mut errors = Vec::new();
    let surface = &file.surface;
    if surface.name.trim().is_empty() {
        errors.push(at(0, "the surface has no name".to_owned()));
    }
    if surface.ring_width == 0 && surface.ring_height > 0 {
        errors.push(at(0, "a ring with scenes needs a width".to_owned()));
    }
    let mut controls = Vec::new();
    let mut taken: HashMap<Source, String> = HashMap::new();
    for table in &file.control {
        let start = table.span().start;
        match control_of(table.get_ref(), surface, text) {
            Ok(control) => {
                // The message or messages the control takes; none may be
                // another's.
                let sources: Vec<Source> = match control.source {
                    Source::Cc14 {
                        channel,
                        controller,
                    } => vec![
                        Source::Cc {
                            channel,
                            controller,
                        },
                        Source::Cc {
                            channel,
                            controller: controller + 32,
                        },
                    ],
                    other => vec![other],
                };
                if let Some(other) = sources.iter().find_map(|s| taken.get(s)) {
                    errors.push(at(
                        start,
                        format!("this message is already control \"{other}\"'s"),
                    ));
                } else {
                    for source in sources {
                        taken.insert(source, control.id.clone());
                    }
                    controls.push(control);
                }
            }
            Err(error) => errors.push(error),
        }
    }
    let mut messages = |tables: &[Spanned<BytesTable>]| -> Vec<Vec<u8>> {
        let mut all = Vec::new();
        for table in tables {
            match hex_messages(table.get_ref().bytes.get_ref()) {
                Ok(found) => all.extend(found),
                Err(message) => errors.push(at(table.get_ref().bytes.span().start, message)),
            }
        }
        all
    };
    let setup = messages(&file.setup);
    let goodbye = messages(&file.goodbye);
    let dump = messages(&file.dump);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Template {
        name: surface.name.trim().to_owned(),
        ring_width: surface.ring_width,
        ring_height: surface.ring_height,
        lock: surface.lock,
        ports: surface.ports.clone(),
        controls,
        setup,
        goodbye,
        dump,
    })
}

fn control_of(
    table: &ControlTable,
    surface: &SurfaceTable,
    text: &str,
) -> Result<Control, TemplateError> {
    let err = |span: &std::ops::Range<usize>, message: String| TemplateError {
        line: line_of(text, span.start),
        message,
    };
    let channel = *table.channel.get_ref();
    if !(1..=16).contains(&channel) {
        return Err(err(
            &table.channel.span(),
            format!("the channel is {channel}; channels are 1 to 16"),
        ));
    }
    let channel = channel - 1;
    let number = |what: &str| {
        let number = table.number.as_ref().ok_or_else(|| {
            err(
                &table.message.span(),
                format!("a {what} control needs a `number`"),
            )
        })?;
        if *number.get_ref() > 127 {
            return Err(err(&number.span(), "the number is 0 to 127".to_owned()));
        }
        Ok(*number.get_ref())
    };
    let source = match table.message.get_ref().as_str() {
        "note" => Source::Note {
            channel,
            key: number("note")?,
        },
        "cc" => Source::Cc {
            channel,
            controller: number("cc")?,
        },
        "cc14" => {
            let controller = number("cc14")?;
            if controller > 31 {
                return Err(err(
                    &table.message.span(),
                    "a 14-bit controller's number is 0 to 31; its low half is 32 higher".to_owned(),
                ));
            }
            Source::Cc14 {
                channel,
                controller,
            }
        }
        "pitch-bend" => Source::PitchBend { channel },
        other => {
            return Err(err(
                &table.message.span(),
                format!("the message \"{other}\" is not note, cc, cc14 or pitch-bend"),
            ));
        }
    };
    let kind = match table.kind.get_ref().as_str() {
        "button" => Reads::Button {
            press: table.press.unwrap_or(127),
            release: table.release.unwrap_or(0),
        },
        "toggle" => Reads::Toggle,
        "absolute" => Reads::Absolute,
        "relative" => {
            let encoding = match table
                .encoding
                .as_ref()
                .map(|e| (e.get_ref().as_str(), e.span()))
            {
                Some(("signed-bit", _)) => RelativeEncoding::SignedBit,
                Some(("signed-bit-2", _)) => RelativeEncoding::SignedBit2,
                Some(("bin-offset", _)) => RelativeEncoding::BinOffset,
                Some(("twos-complement", _)) => RelativeEncoding::TwosComplement,
                Some((other, span)) => {
                    return Err(err(
                        &span,
                        format!(
                            "the encoding \"{other}\" is not signed-bit, signed-bit-2, \
                             bin-offset or twos-complement"
                        ),
                    ));
                }
                None => {
                    return Err(err(
                        &table.kind.span(),
                        "a relative control needs an `encoding`".to_owned(),
                    ));
                }
            };
            if !matches!(source, Source::Cc { .. }) {
                return Err(err(
                    &table.kind.span(),
                    "a relative control sends a controller".to_owned(),
                ));
            }
            Reads::Relative {
                encoding,
                accelerated: table.accelerated.unwrap_or(false),
            }
        }
        other => {
            return Err(err(
                &table.kind.span(),
                format!("the kind \"{other}\" is not button, toggle, absolute or relative"),
            ));
        }
    };
    if matches!(source, Source::Cc14 { .. } | Source::PitchBend { .. })
        && !matches!(kind, Reads::Absolute)
    {
        return Err(err(
            &table.kind.span(),
            "a 14-bit controller and pitch bend are absolute controls".to_owned(),
        ));
    }
    let Some(role) = &table.role else {
        return Err(err(
            &table.kind.span(),
            format!("the control \"{}\" has no `role`", table.id),
        ));
    };
    let act = role_of(role.get_ref(), surface).map_err(|m| err(&role.span(), m))?;
    let absolute = matches!(kind, Reads::Absolute | Reads::Relative { .. });
    let shows_value = matches!(
        act,
        Act::Role(Role::Volume(_) | Role::Pan(_) | Role::MainVolume)
    );
    if act == Act::Tempo && !matches!(kind, Reads::Absolute) {
        return Err(err(
            &role.span(),
            "tempo needs an absolute control".to_owned(),
        ));
    }
    let feedback = match &table.feedback {
        None => None,
        Some(f) if f.value == Some(true) => {
            if !shows_value || !matches!(kind, Reads::Absolute) {
                return Err(err(
                    &table.kind.span(),
                    "only an absolute volume, pan or main-volume control shows its value"
                        .to_owned(),
                ));
            }
            Some(Feedback::Value)
        }
        Some(f) => {
            if absolute {
                return Err(err(
                    &table.kind.span(),
                    "an absolute or relative control's feedback is `value = true`".to_owned(),
                ));
            }
            if matches!(source, Source::Cc14 { .. } | Source::PitchBend { .. }) {
                return Err(err(
                    &table.message.span(),
                    "a lamp is on a note or a controller".to_owned(),
                ));
            }
            let (Some(off), Some(on)) = (f.off, f.on) else {
                return Err(err(
                    &table.kind.span(),
                    "feedback needs `off` and `on`".to_owned(),
                ));
            };
            if off > 127 || on > 127 || f.flash.is_some_and(|v| v > 127) {
                return Err(err(
                    &table.kind.span(),
                    "feedback values are 0 to 127".to_owned(),
                ));
            }
            Some(Feedback::State {
                off,
                on,
                flash: f.flash,
            })
        }
    };
    if shows_value && feedback == Some(Feedback::Value) && matches!(source, Source::Note { .. }) {
        return Err(err(
            &table.message.span(),
            "a value is shown on a controller or pitch bend".to_owned(),
        ));
    }
    Ok(Control {
        id: table.id.clone(),
        source,
        kind,
        act,
        feedback,
    })
}

/// The act a `role` string names: a word, then its numbers from 1 (R40).
fn role_of(text: &str, surface: &SurfaceTable) -> Result<Act, String> {
    let mut words = text.split_whitespace();
    let word = words.next().unwrap_or("");
    let numbers: Vec<u8> = words
        .map(|w| {
            w.parse::<u8>()
                .map_err(|_| format!("\"{w}\" is not a number from 1 to 255"))
        })
        .collect::<Result<_, _>>()?;
    let (wants, role): (usize, fn(&[u8]) -> Act) = match word {
        "slot" => (2, |n| {
            Act::Role(Role::Slot {
                column: n[0] - 1,
                row: n[1] - 1,
            })
        }),
        "scene" => (1, |n| Act::Role(Role::SceneLaunch(n[0] - 1))),
        "track-stop" => (1, |n| Act::Role(Role::Stop(n[0] - 1))),
        "track-mute" => (1, |n| Act::Role(Role::Mute(n[0] - 1))),
        "track-solo" => (1, |n| Act::Role(Role::Solo(n[0] - 1))),
        "track-arm" => (1, |n| Act::Role(Role::Arm(n[0] - 1))),
        "track-select" => (1, |n| Act::Role(Role::Select(n[0] - 1))),
        "volume" => (1, |n| Act::Role(Role::Volume(n[0] - 1))),
        "pan" => (1, |n| Act::Role(Role::Pan(n[0] - 1))),
        "send" => (2, |n| {
            Act::Role(Role::Send {
                column: n[1] - 1,
                send: n[0] - 1,
            })
        }),
        "device" => (1, |n| Act::Role(Role::Device(n[0] - 1))),
        "stop-all-clips" => (0, |_| Act::Role(Role::StopAll)),
        "main-volume" => (0, |_| Act::Role(Role::MainVolume)),
        "next-bank" => (0, |_| Act::Role(Role::NextBank)),
        "previous-bank" => (0, |_| Act::Role(Role::PreviousBank)),
        "next-device" => (0, |_| Act::NextDevice),
        "previous-device" => (0, |_| Act::PreviousDevice),
        "ring-left" => (0, |_| Act::Role(Role::Ring(RingStep::Left))),
        "ring-right" => (0, |_| Act::Role(Role::Ring(RingStep::Right))),
        "ring-up" => (0, |_| Act::Role(Role::Ring(RingStep::Up))),
        "ring-down" => (0, |_| Act::Role(Role::Ring(RingStep::Down))),
        "page-left" => (0, |_| Act::Role(Role::Ring(RingStep::PageLeft))),
        "page-right" => (0, |_| Act::Role(Role::Ring(RingStep::PageRight))),
        "page-up" => (0, |_| Act::Role(Role::Ring(RingStep::PageUp))),
        "page-down" => (0, |_| Act::Role(Role::Ring(RingStep::PageDown))),
        "play" => (0, |_| Act::Play),
        "stop" => (0, |_| Act::Stop),
        "record" => (0, |_| Act::Record),
        "session-record" => (0, |_| Act::SessionRecord),
        "loop" => (0, |_| Act::Loop),
        "metronome" => (0, |_| Act::Metronome),
        "tap-tempo" => (0, |_| Act::TapTempo),
        "tempo" => (0, |_| Act::Tempo),
        "undo" => (0, |_| Act::Undo),
        "redo" => (0, |_| Act::Redo),
        "back-to-arrangement" => (0, |_| Act::BackToArrangement),
        other => return Err(format!("\"{other}\" is not a role")),
    };
    if numbers.len() != wants {
        return Err(format!("the role {word} takes {wants} number(s) after it"));
    }
    if numbers.contains(&0) {
        return Err("a role's numbers count from 1".to_owned());
    }
    let (width, height) = (surface.ring_width, surface.ring_height);
    let outside = |what: &str, limit: u8| {
        Err(format!(
            "{what} is outside the ring, which is {width} tracks by {height} scenes (the limit is {limit})"
        ))
    };
    match word {
        "slot" => {
            if numbers[0] > width {
                return outside("the slot's column", width);
            }
            if numbers[1] > height {
                return outside("the slot's row", height);
            }
        }
        "scene" if numbers[0] > height => return outside("the scene's row", height),
        "track-stop" | "track-mute" | "track-solo" | "track-arm" | "track-select" | "volume"
        | "pan"
            if numbers[0] > width =>
        {
            return outside("the track's column", width);
        }
        "send" if numbers[1] > width => return outside("the track's column", width),
        "device" if numbers[0] > 8 => {
            return Err("a device control is 1 to 8".to_owned());
        }
        _ => {}
    }
    Ok(role(&numbers))
}

/// The whole messages in a string of hexadecimal bytes (R41, R44).
fn hex_messages(text: &str) -> Result<Vec<Vec<u8>>, String> {
    let bytes: Vec<u8> = text
        .split_whitespace()
        .map(|w| {
            u8::from_str_radix(w, 16).map_err(|_| format!("\"{w}\" is not a hexadecimal byte"))
        })
        .collect::<Result<_, _>>()?;
    let mut messages = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let status = bytes[at];
        let end = match status {
            0xF0 => bytes[at..]
                .iter()
                .position(|b| *b == 0xF7)
                .map(|n| at + n + 1)
                .ok_or("a system-exclusive message needs its F7")?,
            0x80..=0xBF | 0xE0..=0xEF => at + 3,
            0xC0..=0xDF => at + 2,
            _ => return Err(format!("{status:02X} doesn't start a message")),
        };
        let message = bytes.get(at..end).ok_or("a message is cut short")?.to_vec();
        if message[1..message.len().saturating_sub(usize::from(status == 0xF0))]
            .iter()
            .any(|b| *b > 0x7F)
        {
            return Err("a message's data bytes are 00 to 7F".to_owned());
        }
        messages.push(message);
        at = end;
    }
    Ok(messages)
}

/// Where a template was found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The template's name, if the file parsed far enough to have one.
    pub name: Option<String>,
    /// The file; `None` for the bundled template.
    pub path: Option<PathBuf>,
    /// What is wrong with the file, if anything.
    pub errors: Vec<TemplateError>,
    /// The template, if the file is whole.
    pub template: Option<Arc<Template>>,
    /// Whether the chooser lists it: not a broken one, one replaced by a
    /// User Library template of its name, or a second of one name (R43,
    /// R44).
    pub listed: bool,
}

/// Every template: the bundled one, then the files in `folder` and its
/// subfolders, in path order (R43, R44).
#[must_use]
pub fn load(folder: Option<&Path>) -> Vec<Found> {
    let mut found = vec![found_of(None, BUNDLED)];
    let mut files = Vec::new();
    if let Some(folder) = folder {
        walk(folder, 8, &mut files);
    }
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path);
        found.push(match text {
            Ok(text) => found_of(Some(path), &text),
            Err(e) => Found {
                name: None,
                path: Some(path),
                errors: vec![TemplateError {
                    line: 1,
                    message: format!("can't be read: {e}"),
                }],
                template: None,
                listed: false,
            },
        });
    }
    // A name is listed once: a User Library template replaces the bundled
    // one, and of two in the User Library the first by path wins.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for index in 0..found.len() {
        let Some(name) = found[index].template.as_ref().map(|t| t.name.clone()) else {
            continue;
        };
        match by_name.get(&name).copied() {
            Some(earlier) if found[earlier].path.is_none() => found[earlier].listed = false,
            Some(_) => {
                found[index].errors.push(TemplateError {
                    line: 1,
                    message: format!("another template file already has the name \"{name}\""),
                });
                found[index].template = None;
                continue;
            }
            None => {}
        }
        by_name.insert(name, index);
        found[index].listed = true;
    }
    found
}

fn found_of(path: Option<PathBuf>, text: &str) -> Found {
    match parse(text) {
        Ok(template) => Found {
            name: Some(template.name.clone()),
            path,
            errors: Vec::new(),
            template: Some(Arc::new(template)),
            listed: false,
        },
        Err(errors) => Found {
            name: None,
            path,
            errors,
            template: None,
            listed: false,
        },
    }
}

/// The `.bltemplate.toml` files under `folder`, to `depth` folders down; a
/// link to a folder isn't followed.
fn walk(folder: &Path, depth: u8, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, files);
            }
        } else if kind.is_file()
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(EXTENSION))
        {
            files.push(path);
        }
    }
}

/// The surfaces the chooser offers for the templates found, by name (R43).
#[must_use]
pub fn kinds(found: &[Found]) -> Vec<Arc<Kind>> {
    let mut listed: Vec<&Arc<Template>> = found
        .iter()
        .filter(|f| f.listed)
        .filter_map(|f| f.template.as_ref())
        .collect();
    listed.sort_by_key(|t| (t.name.to_lowercase(), t.name.clone()));
    listed.into_iter().map(|t| Arc::new(t.kind())).collect()
}
