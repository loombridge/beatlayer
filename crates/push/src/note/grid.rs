//! Where things are on the pads in a drum layout, and the step sizes and
//! pages the step area counts in (`docs/specs/push-2-notes.md`, R8 to R13,
//! R21 and R26).

use bl_core::time::Ticks;

/// How many step pads a page holds in a drum layout, and with a triplet size.
const PAGE_STEPS: i64 = 16;
const PAGE_STEPS_TRIPLET: i64 = 12;
/// How many loop length pads there are: one page each.
pub const PAGES: u8 = 16;
/// The highest first note of the drum bank: the view shows sixteen notes.
pub const LAST_FIRST_PAD: u8 = 112;

/// The three drum layouts (R8, R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Sixteen drum pads, sixteen loop length pads and 32 steps.
    #[default]
    LoopSelector,
    /// As the Loop Selector, with sixteen velocities in place of the loop
    /// length pads.
    Velocities,
    /// Drums across the whole grid.
    Pads,
}

impl Layout {
    /// The layout Layout steps to.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::LoopSelector => Self::Velocities,
            Self::Velocities => Self::Pads,
            Self::Pads => Self::LoopSelector,
        }
    }

    /// The layout's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LoopSelector => "Loop Selector",
            Self::Velocities => "16 Velocities",
            Self::Pads => "64 Pads",
        }
    }

    /// What holding Layout shows over this layout, in the lower-right 4 × 4
    /// pads (R9).
    #[must_use]
    pub const fn held(self) -> Overlay {
        match self {
            Self::LoopSelector => Overlay::Velocities,
            Self::Velocities | Self::Pads => Overlay::LoopPads,
        }
    }
}

/// What the lower-right 4 × 4 pads show besides their layout's own (R9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// Sixteen velocities.
    Velocities,
    /// The loop length pads.
    LoopPads,
}

impl Overlay {
    /// The overlay's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Velocities => "16 Velocities",
            Self::LoopPads => "Loop Length Pads",
        }
    }
}

/// What a pad is in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A drum pad, by its note.
    Drum(u8),
    /// A velocity pad, by its velocity.
    Velocity(u8),
    /// A loop length pad, by its page from 0.
    Loop(u8),
    /// A step pad, by its place in the step area counting each row from
    /// the left and the rows from the bottom.
    Step(u8),
    /// Nothing.
    Off,
}

/// A step size (R21): the length of one step pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size(pub u8);

/// The sizes, from 1/32 triplet at the top to a quarter note at the
/// bottom, by their scene button's row from the top.
const SIZES: [(&str, i64, bool); 8] = [
    ("1/32T", 32, true),
    ("1/32", 32, false),
    ("1/16T", 16, true),
    ("1/16", 16, false),
    ("1/8T", 8, true),
    ("1/8", 8, false),
    ("1/4T", 4, true),
    ("1/4", 4, false),
];

impl Default for Size {
    /// 1/16 for a new track.
    fn default() -> Self {
        Self(3)
    }
}

impl Size {
    /// The size at scene button row `row` from the top.
    #[must_use]
    pub const fn at(row: u8) -> Option<Self> {
        if row < 8 { Some(Self(row)) } else { None }
    }

    /// The scene button row from the top that chooses it.
    #[must_use]
    pub const fn row(self) -> u8 {
        self.0
    }

    /// The size's name, such as `1/16` or `1/8T`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        SIZES[self.0 as usize].0
    }

    /// Whether it is a triplet size.
    #[must_use]
    pub const fn triplet(self) -> bool {
        SIZES[self.0 as usize].2
    }

    /// How long one step is.
    #[must_use]
    pub const fn ticks(self) -> Ticks {
        let (_, division, triplet) = SIZES[self.0 as usize];
        let whole = Ticks::WHOLE.0 / division;
        Ticks(if triplet { whole * 2 / 3 } else { whole })
    }

    /// How many step pads a row of the step area uses: a triplet size
    /// leaves the right two columns off (R21).
    #[must_use]
    pub const fn per_row(self) -> u8 {
        if self.triplet() { 6 } else { 8 }
    }

    /// How many steps a page holds (R26).
    #[must_use]
    pub const fn page_steps(self) -> i64 {
        if self.triplet() {
            PAGE_STEPS_TRIPLET
        } else {
            PAGE_STEPS
        }
    }

    /// How long a page is.
    #[must_use]
    pub const fn page(self) -> Ticks {
        Ticks(self.ticks().0 * self.page_steps())
    }

    /// Where step pad `index` of the step area starts, with pages `2 * pair`
    /// and the one after it in the area (R26).
    #[must_use]
    pub const fn step_start(self, pair: u8, index: u8) -> Ticks {
        Ticks(self.page().0 * 2 * pair as i64 + self.ticks().0 * index as i64)
    }

    /// How many step pads the step area uses.
    #[must_use]
    pub const fn area(self) -> u8 {
        self.per_row() * 4
    }

    /// Where page `page` starts.
    #[must_use]
    pub const fn page_start(self, page: u8) -> Ticks {
        Ticks(self.page().0 * page as i64)
    }
}

/// What the pad in row `row` from the bottom and column `column` from the
/// left is, in `layout` with `overlay` over it, when the bank starts at note
/// `first` (R8 to R11).
#[must_use]
pub fn role(
    layout: Layout,
    overlay: Option<Overlay>,
    first: u8,
    size: Size,
    row: u8,
    column: u8,
) -> Role {
    let drum = |note: u16| u8::try_from(note).ok().filter(|n| *n < 128);
    let lower_right = row < 4 && column >= 4;
    let index = u16::from(row) * 4 + u16::from(column.saturating_sub(4));
    let over = overlay.or(match layout {
        Layout::LoopSelector => Some(Overlay::LoopPads),
        Layout::Velocities => Some(Overlay::Velocities),
        Layout::Pads => None,
    });
    if lower_right && let Some(over) = over {
        return match over {
            Overlay::LoopPads => Role::Loop(index as u8),
            // round(127 * (n + 1) / 16): 8 at the bottom left, 127 at the
            // top right (R10).
            Overlay::Velocities => Role::Velocity(((127 * (index + 1) + 8) / 16) as u8),
        };
    }
    if layout == Layout::Pads {
        let note = u16::from(first)
            + u16::from(column / 4) * 32
            + u16::from(row) * 4
            + u16::from(column % 4);
        return drum(note).map_or(Role::Off, Role::Drum);
    }
    if row < 4 {
        let note = u16::from(first) + u16::from(row) * 4 + u16::from(column);
        return drum(note).map_or(Role::Off, Role::Drum);
    }
    if column < size.per_row() {
        Role::Step((row - 4) * size.per_row() + column)
    } else {
        Role::Off
    }
}

/// The pad that a role is on in a layout, if any: the inverse of [`role`]
/// for the roles a test names.
#[must_use]
pub fn find(
    layout: Layout,
    overlay: Option<Overlay>,
    first: u8,
    size: Size,
    wanted: Role,
) -> Option<(u8, u8)> {
    (0..8)
        .flat_map(|row| (0..8).map(move |column| (row, column)))
        .find(|&(row, column)| role(layout, overlay, first, size, row, column) == wanted)
}

/// `rgb` a share of the way to white (R12, R23).
#[must_use]
pub fn lighter([r, g, b]: [u8; 3]) -> [u8; 3] {
    let mix = |c: u8| c + (255 - c) / 2;
    [mix(r), mix(g), mix(b)]
}

/// `rgb` at `share` percent of its brightness.
#[must_use]
pub fn scaled([r, g, b]: [u8; 3], share: u16) -> [u8; 3] {
    let scale = |c: u8| (u16::from(c) * share / 100) as u8;
    [scale(r), scale(g), scale(b)]
}

/// The touch strip's 31 LEDs for a bank starting at `first`: the LEDs that
/// the sixteen notes of the bank cover are lit (R13).
#[must_use]
pub fn strip_leds(first: u8) -> [u8; 31] {
    let mut levels = [0; 31];
    let (low, high) = (u32::from(first), u32::from(first) + 16);
    for (i, level) in levels.iter_mut().enumerate() {
        let (from, to) = (i as u32 * 128 / 31, (i as u32 + 1) * 128 / 31);
        if from < high && to > low {
            *level = 7;
        }
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: push-2-notes R8, R10. The Loop Selector's three blocks, and the
    // sixteen velocities from 8 at the bottom left to 127 at the top right.
    #[test]
    fn the_loop_selector_has_drums_loop_pads_and_steps() {
        let size = Size::default();
        let at = |row, column| role(Layout::LoopSelector, None, 36, size, row, column);
        assert_eq!(at(0, 0), Role::Drum(36));
        assert_eq!(at(3, 3), Role::Drum(51));
        assert_eq!(at(0, 4), Role::Loop(0));
        assert_eq!(at(3, 7), Role::Loop(15));
        assert_eq!(at(4, 0), Role::Step(0));
        assert_eq!(at(7, 7), Role::Step(31));
        let v = |row, column| role(Layout::Velocities, None, 36, size, row, column);
        assert_eq!(v(0, 4), Role::Velocity(8));
        assert_eq!(v(3, 7), Role::Velocity(127));
        assert_eq!(v(0, 5), Role::Velocity(16));
    }

    // Spec: push-2-notes R11. 64 Pads: the left four columns hold the first
    // 32 notes of the bank, four to a row from the bottom, the right four the
    // next 32; notes past 127 are off.
    #[test]
    fn sixty_four_pads_play_drums_across_the_grid() {
        let at = |first, row, column| role(Layout::Pads, None, first, Size::default(), row, column);
        assert_eq!(at(36, 0, 0), Role::Drum(36));
        assert_eq!(at(36, 0, 3), Role::Drum(39));
        assert_eq!(at(36, 1, 0), Role::Drum(40));
        assert_eq!(at(36, 7, 3), Role::Drum(67));
        assert_eq!(at(36, 0, 4), Role::Drum(68));
        assert_eq!(at(36, 7, 7), Role::Drum(99));
        assert_eq!(at(112, 7, 7), Role::Off);
    }

    // Spec: push-2-notes R21. A triplet size leaves the right two columns
    // of the step area off.
    #[test]
    fn a_triplet_size_has_six_steps_to_a_row() {
        let size = Size(2);
        assert!(size.triplet());
        assert_eq!(
            role(Layout::LoopSelector, None, 36, size, 4, 5),
            Role::Step(5)
        );
        assert_eq!(role(Layout::LoopSelector, None, 36, size, 4, 6), Role::Off);
        assert_eq!(
            role(Layout::LoopSelector, None, 36, size, 5, 0),
            Role::Step(6)
        );
        assert_eq!(size.ticks(), Ticks::WHOLE / 24);
        assert_eq!(Size::default().page(), Ticks::WHOLE);
    }

    // Spec: push-2-notes R9. Holding Layout shows the velocities over the
    // Loop Selector, and the loop length pads over the other two.
    #[test]
    fn holding_layout_shows_velocities_or_loop_pads() {
        assert_eq!(Layout::LoopSelector.held(), Overlay::Velocities);
        assert_eq!(Layout::Velocities.held(), Overlay::LoopPads);
        assert_eq!(
            role(
                Layout::Pads,
                Some(Overlay::LoopPads),
                36,
                Size::default(),
                0,
                4
            ),
            Role::Loop(0)
        );
    }
}
