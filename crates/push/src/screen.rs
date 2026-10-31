//! What the display shows, as data: the bands' labels, the eight columns,
//! the notification, the count-in bar and the progress bars
//! (`docs/specs/push-2.md`, R18 to R20).
//!
//! The surface fills a [`Screen`]; the display thread draws it ([`crate::paint`])
//! and sends a frame when it differs from the one before. `push.state` reads
//! the same data back as text.

/// A picture beside or in place of a label's text: Beatlayer's own icons for
/// the four kinds of rack, and the arrows that move a window of devices or
/// banks (push-2-tracks R15, R17, R22).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
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
    /// The picture that goes with it, if there is one.
    pub icon: Option<Icon>,
}

impl Label {
    /// A label with `text` in `colour`.
    #[must_use]
    pub fn new(text: impl Into<String>, colour: [u8; 3], selected: bool) -> Self {
        Self {
            text: text.into(),
            colour,
            selected,
            icon: None,
        }
    }

    /// The same label with `icon` beside its text.
    #[must_use]
    pub const fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Whether the button under it does anything (R18).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty() && self.icon.is_none()
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
                let icon = label.icon.map_or(String::new(), |i| {
                    if i.is_arrow() {
                        format!("<{}>", i.name())
                    } else {
                        format!(" [{}]", i.name())
                    }
                });
                lines.push(format!("{name} {}: {}{icon}{selected}", n + 1, label.text));
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
        if let Some(title) = &self.title {
            lines.push(format!("title: {title}"));
        }
        if let Some(message) = &self.notification {
            lines.push(format!("notification: {message}"));
        }
        lines
    }
}
