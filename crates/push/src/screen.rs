//! What the display shows, as data: the bands' labels, the eight columns,
//! the notification, the count-in bar and the progress bars
//! (`docs/specs/push-2.md`, R18 to R20).
//!
//! The surface fills a [`Screen`]; the display thread draws it ([`crate::paint`])
//! and sends a frame when it differs from the one before. `push.state` reads
//! the same data back as text.

/// A picture beside or in place of a label's text: Beatlayer's own icons for
/// the four kinds of rack, the arrows that move a window of devices or banks
/// (push-2-tracks R15, R17, R22), and a group's or a rack's beside a track
/// that unfolds (R37).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Icon {
    /// Nothing.
    #[default]
    None,
    /// A group track that unfolds.
    Group,
    /// A track holding an instrument rack or a drum rack, which unfolds.
    Rack,
    /// An instrument rack.
    InstrumentRack,
    /// An audio effect rack.
    AudioEffectRack,
    /// A MIDI effect rack.
    MidiEffectRack,
    /// A drum rack.
    DrumRack,
    /// An arrow pointing left.
    Left,
    /// An arrow pointing right.
    Right,
}

impl Icon {
    /// The icon's name, for `push.state`'s text.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Group => "group",
            Self::Rack => "rack",
            Self::InstrumentRack => "instrument rack",
            Self::AudioEffectRack => "audio effect rack",
            Self::MidiEffectRack => "MIDI effect rack",
            Self::DrumRack => "drum rack",
            Self::Left => "left arrow",
            Self::Right => "right arrow",
        }
    }

    /// Whether the icon is an arrow, which stands alone in its label.
    #[must_use]
    pub const fn is_arrow(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// What a display button's label shows (R18): empty means the button does
/// nothing and its LED is off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Label {
    /// The text, empty for none.
    pub text: String,
    /// The colour of what the label stands for.
    pub colour: [u8; 3],
    /// Whether it stands for the selected one.
    pub selected: bool,
    /// Whether it stands for a track armed by hand, which shows an arm icon
    /// (notes R4).
    pub armed: bool,
    /// The picture that goes with it: an arrow or a rack's icon in Device
    /// Mode, a group's or a rack's beside a track that unfolds in Mix Mode
    /// (push-2-tracks R22, R37).
    pub icon: Icon,
}

impl Label {
    /// A label with `text` in `colour`.
    #[must_use]
    pub fn new(text: impl Into<String>, colour: [u8; 3], selected: bool) -> Self {
        Self {
            text: text.into(),
            colour,
            selected,
            armed: false,
            icon: Icon::None,
        }
    }

    /// The same label, with an arm icon if `armed`.
    #[must_use]
    pub fn with_armed(mut self, armed: bool) -> Self {
        self.armed = armed;
        self
    }

    /// The same label with `icon` beside its text.
    #[must_use]
    pub const fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }

    /// Whether the button under it does anything (R18).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty() && matches!(self.icon, Icon::None)
    }
}

/// The graphic of a parameter's value (R20).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Graphic {
    /// Nothing.
    #[default]
    None,
    /// An arc for a continuous parameter, its value from 0 to 1.
    Arc(f32),
    /// The chosen entry of a list.
    Choice(String),
    /// A vertical fader for a volume, its value from 0 (bottom) to 1.
    Fader(f32),
}

/// The dot beside a parameter's name (R20).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dot {
    /// No automation.
    #[default]
    None,
    /// Automation that plays: a white dot.
    Automated,
    /// Automation that is overridden: a grey dot.
    Overridden,
}

/// How strongly a column is drawn (push-2-tracks R18).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shade {
    /// As any column.
    #[default]
    Normal,
    /// Dimmed: a parameter a macro holds, which the encoder doesn't move.
    Dimmed,
    /// Grey: a parameter of a device that is switched off.
    Grey,
}

/// One column of the content band, under an encoder (R20). A column whose
/// encoder controls nothing is empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Column {
    /// The parameter's name.
    pub name: String,
    /// Its value as text.
    pub value: String,
    /// The graphic of its value.
    pub graphic: Graphic,
    /// The automation dot.
    pub dot: Dot,
    /// Whether the encoder is touched.
    pub touched: bool,
}

impl Column {
    /// Whether the column shows nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A list in a column of the content band: a browser column (push-2-tracks
/// R6).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    /// The entries: each one's name, and whether it can hold entries.
    pub rows: Vec<(String, bool)>,
    /// The selected row.
    pub selected: Option<usize>,
    /// Whether it is the column in use.
    pub in_use: bool,
}

/// A box of text over the content band, which a notification or a mode's
/// name uses (R19).
pub type Message = String;

/// One note of a MIDI clip as Clip Mode draws it (push-2-tracks R44).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// The row it sits on, from the top of the clip's rows.
    pub row: usize,
    /// Where it starts, in beats of the clip's contents.
    pub from: f64,
    /// Where it ends.
    pub to: f64,
    /// How opaque it is, from 0 to 1: its velocity.
    pub opacity: f32,
    /// Its colour: its pad's on a drum rack, else the clip's.
    pub colour: [u8; 3],
}

/// What Clip Mode shows of a clip (push-2-tracks R43, R44).
#[derive(Clone, Debug, PartialEq)]
pub enum ClipBody {
    /// A message instead of a clip: an empty slot, or no clip at all.
    Prompt(String),
    /// A MIDI clip's notes: the pitches of its rows, highest first, and the
    /// notes in view.
    Notes {
        /// The pitch of each row, from the top.
        rows: Vec<u8>,
        /// The notes that overlap the span shown.
        bars: Vec<Bar>,
    },
    /// An audio clip's waveform across the span shown: the lowest and
    /// highest sample of each of equal stretches of it.
    Waveform(Vec<[f32; 2]>),
}

/// The sequenceable area a step sequencing layout marks (push-2-tracks R51).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaBox {
    /// Where the page of steps starts, in beats.
    pub from: f64,
    /// Where it ends.
    pub to: f64,
    /// The rows the pads edit, first and last from the top, or every row.
    pub rows: Option<(usize, usize)>,
}

/// What Clip Mode draws over the content band: the clip, its markers and
/// the sequencers' boxes (push-2-tracks R44, R50 to R55).
#[derive(Clone, Debug, PartialEq)]
pub struct ClipScreen {
    /// The clip's name, highlighted at the top left.
    pub name: String,
    /// The clip's colour.
    pub colour: [u8; 3],
    /// What the clip shows.
    pub body: ClipBody,
    /// The span of beats from the display's left edge to its right.
    pub view: (f64, f64),
    /// Where the clip plays now, in beats of its contents.
    pub playhead: Option<f64>,
    /// The clip's markers by name, at their beats.
    pub markers: Vec<(&'static str, f64)>,
    /// The sequenceable area.
    pub area: Option<AreaBox>,
    /// The box at the left edge: the pitches the pads edit as a share of
    /// the 128 pitches, from the bottom, low to high.
    pub range: Option<(f32, f32)>,
    /// The lines at the left edge: where pitches hold notes as a share of
    /// the 128 from the bottom, and how thick, from 1 to 4.
    pub lines: Vec<(f32, u8)>,
    /// The spans of the steps held, in beats.
    pub held: Vec<(f64, f64)>,
}

impl ClipScreen {
    /// A clip's text lines, for `push.state`.
    fn text(&self, lines: &mut Vec<String>) {
        lines.push(format!("clip: {}", self.name));
        lines.push(match &self.body {
            ClipBody::Prompt(message) => format!("clip shows: {message}"),
            ClipBody::Notes { rows, bars } => {
                format!("clip shows: {} notes on {} rows", bars.len(), rows.len())
            }
            ClipBody::Waveform(peaks) => format!("clip shows: waveform of {}", peaks.len()),
        });
        lines.push(format!(
            "clip view: {:.2} to {:.2}",
            self.view.0, self.view.1
        ));
        if let Some(at) = self.playhead {
            lines.push(format!("clip playhead: {at:.2}"));
        }
        for (name, at) in &self.markers {
            lines.push(format!("clip marker {name}: {at:.2}"));
        }
        if let Some(area) = &self.area {
            lines.push(format!("clip area: {:.2} to {:.2}", area.from, area.to));
        }
        if let Some((low, high)) = self.range {
            lines.push(format!("clip range: {:.2} to {:.2}", low, high));
        }
        for (from, to) in &self.held {
            lines.push(format!("clip held: {from:.2} to {to:.2}"));
        }
    }
}

/// The display's content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Screen {
    /// The upper display buttons' labels, left to right.
    pub upper: [Label; 8],
    /// The lower display buttons' labels.
    pub lower: [Label; 8],
    /// The content band's columns.
    pub columns: [Column; 8],
    /// How strongly each column is drawn.
    pub shades: [Shade; 8],
    /// Lists drawn in place of a column's parameter, as Browse Mode does.
    pub lists: [Option<List>; 8],
    /// A short message over the content band for 1.5 s (R19).
    pub notification: Option<Message>,
    /// While a count-in runs, whether its bar is lit on this beat (R19).
    pub count_in: Option<bool>,
    /// Under each lower label, where the track's playing clip is in its loop,
    /// from 0 to 1 (R19).
    pub progress: [Option<f32>; 8],
    /// A line across the middle of the content band: a mode's name where its
    /// content isn't built yet, or the User Mode screen (R21, R33).
    pub title: Option<String>,
    /// The Setup menu's velocity curve, drawn over columns 4 and 5 (R31).
    pub curve: Option<Vec<u8>>,
    /// Lines under the upper band, each from a rack's column to the column of
    /// the last device it holds, both from 0 (push-2-tracks R22).
    pub folds: Vec<(usize, usize)>,
    /// Lines under the lower labels from one column to another, inclusive,
    /// in a colour: the tracks inside an unfolded group or rack (push-2-tracks
    /// R37).
    pub underlines: Vec<(u8, u8, [u8; 3])>,
    /// A clip drawn over the content band's upper part, with the columns
    /// beneath it (push-2-tracks R44).
    pub clip: Option<ClipScreen>,
}

impl Screen {
    /// The text on the display, band by band and column by column: what
    /// `push.state` reports (R34).
    #[must_use]
    pub fn text(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(lit) = self.count_in {
            lines.push(format!("count-in {}", if lit { "on" } else { "off" }));
        }
        let band = |name: &str, labels: &[Label; 8], lines: &mut Vec<String>| {
            for (n, label) in labels.iter().enumerate().filter(|(_, l)| !l.is_empty()) {
                let selected = if label.selected { " (selected)" } else { "" };
                let icon = match label.icon {
                    Icon::None => String::new(),
                    i if i.is_arrow() => format!("<{}>", i.name()),
                    i => format!(" [{}]", i.name()),
                };
                let armed = if label.armed { " (armed)" } else { "" };
                lines.push(format!(
                    "{name} {}: {}{icon}{selected}{armed}",
                    n + 1,
                    label.text
                ));
            }
        };
        band("upper", &self.upper, &mut lines);
        for (n, column) in self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_empty())
        {
            let dot = match column.dot {
                Dot::None => "",
                Dot::Automated => " [white dot]",
                Dot::Overridden => " [grey dot]",
            };
            let touched = if column.touched { " (touched)" } else { "" };
            let shade = match self.shades[n] {
                Shade::Normal => "",
                Shade::Dimmed => " (dimmed)",
                Shade::Grey => " (grey)",
            };
            lines.push(format!(
                "column {}: {} = {}{dot}{touched}{shade}",
                n + 1,
                column.name,
                column.value
            ));
        }
        for (n, list) in self.lists.iter().enumerate() {
            let Some(list) = list else {
                continue;
            };
            let rows: Vec<String> = list
                .rows
                .iter()
                .enumerate()
                .map(|(row, (name, folder))| {
                    let mark = if list.selected == Some(row) { "*" } else { "" };
                    format!("{mark}{name}{}", if *folder { "/" } else { "" })
                })
                .collect();
            let in_use = if list.in_use { " (in use)" } else { "" };
            lines.push(format!("list {}{in_use}: {}", n + 1, rows.join(", ")));
        }
        for (from, to) in &self.folds {
            lines.push(format!("fold: {} to {}", from + 1, to + 1));
        }
        band("lower", &self.lower, &mut lines);
        for (n, progress) in self.progress.iter().enumerate() {
            if let Some(progress) = progress {
                lines.push(format!("progress {}: {:.0}%", n + 1, progress * 100.0));
            }
        }
        for (from, to, _) in &self.underlines {
            lines.push(format!("underline: {} to {}", from + 1, to + 1));
        }
        if let Some(clip) = &self.clip {
            clip.text(&mut lines);
        }
        if let Some(title) = &self.title {
            lines.push(format!("title: {title}"));
        }
        if let Some(message) = &self.notification {
            lines.push(format!("notification: {message}"));
        }
        lines
    }
}
