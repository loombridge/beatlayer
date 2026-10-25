//! The Groove Pool (WP-6.20; the grooves spec R24 to R28, R31 and R35;
//! layout.md L-GRV-001 to L-GRV-006): the lower part of the browser column,
//! with a row for each groove of the set, its hot-swap, save and commit
//! buttons and five fields, and a footer holding the section's name, the
//! Auto Load Groove switch with its hot-swap button, and the Global Amount.
//!
//! Like the browser, it holds no state of the set: what it shows comes from
//! `groove.pool`, and every click, key, drag and drop becomes one of the
//! groove commands, which the host runs. What it keeps is view: the selected
//! row, a name being typed, and how far it is scrolled. A field's drag is one
//! undo step through a named gesture.
//!
//! The sizes past L-GRV-001's, the fields' widths and the look of the row
//! buttons are Beatlayer's own reading (the reference gives the pool's place
//! and its parts only).

use bl_model::{Groove, GrooveBase, GrooveId};
use bl_shell::groove_commands::{GrooveHotSwap, GroovePoolEntry, GroovePoolState};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{
    Align2, DragAndDrop, Id, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::layout::BROWSER_ROW;
use crate::main_panel::{Entry, Place, field};
use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice};

/// The gap between the browser and the pool in the column (L-GRV-001: the
/// browser ends at 392, the pool starts at 400).
const GAP: f32 = 8.0;
/// The column headers' height, and the footer's.
const HEADER: f32 = 16.0;
const FOOTER: f32 = 22.0;
/// A row's height: the browser's.
const ROW: f32 = BROWSER_ROW;
/// The name column's width, a row button's side and the gap between two. The
/// columns fit the column's default width, so every field is in reach of the
/// keyboard and of assistive technology; a wider column leaves room at the
/// right.
const NAME: f32 = 60.0;
const BUTTON: f32 = 12.0;
const BUTTON_GAP: f32 = 2.0;
/// The Base chooser's width and a percentage field's.
const BASE: f32 = 42.0;
const VALUE: f32 = 34.0;
/// The margin at a row's left, and between a row's columns.
const MARGIN: f32 = 4.0;
const COLUMN_GAP: f32 = 2.0;
/// The footer's switch and buttons.
const SWITCH: Vec2 = vec2(24.0, 14.0);
const AMOUNT: Vec2 = vec2(46.0, 15.0);
/// The column headers' text size, which "Quantize" fits a column at.
const HEADER_TEXT: f32 = 8.0;

/// What a drag of a groove carries: the file of one in the browser, or a
/// groove of the pool (R28). Dropped on a clip it becomes the clip's groove,
/// on a MIDI track or the drop area it becomes a clip of its notes, and a
/// file dropped on the pool loads there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrooveDrag {
    /// A groove of the pool.
    pub groove: Option<GrooveId>,
    /// A groove file in the browser.
    pub path: Option<String>,
}

/// The column headers, in order (L-GRV-002).
const HEADERS: [&str; 6] = [
    "Groove Name",
    "Base",
    "Quantize",
    "Timing",
    "Random",
    "Velocity",
];

/// The four fields that are percentages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Param {
    Quantize,
    Timing,
    Random,
    Velocity,
}

impl Param {
    const ALL: [Self; 4] = [Self::Quantize, Self::Timing, Self::Random, Self::Velocity];

    const fn name(self) -> &'static str {
        match self {
            Self::Quantize => "Quantize",
            Self::Timing => "Timing",
            Self::Random => "Random",
            Self::Velocity => "Velocity",
        }
    }

    /// The command's argument for it.
    const fn key(self) -> &'static str {
        match self {
            Self::Quantize => "quantize",
            Self::Timing => "timing",
            Self::Random => "random",
            Self::Velocity => "velocity",
        }
    }

    /// Its range, in percent (R2).
    const fn range(self) -> (f64, f64) {
        match self {
            Self::Velocity => (-100.0, 100.0),
            _ => (0.0, 100.0),
        }
    }

    fn of(self, groove: &Groove) -> f64 {
        match self {
            Self::Quantize => groove.quantize,
            Self::Timing => groove.timing,
            Self::Random => groove.random,
            Self::Velocity => groove.velocity,
        }
    }
}

/// A row button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    HotSwap,
    Save,
    Commit,
}

impl Button {
    const ALL: [Self; 3] = [Self::HotSwap, Self::Save, Self::Commit];

    const fn name(self) -> &'static str {
        match self {
            Self::HotSwap => "Hot-Swap Groove",
            Self::Save => "Save Groove",
            Self::Commit => "Commit Groove",
        }
    }
}

/// A Base as the chooser reads it.
const fn base_text(base: GrooveBase) -> &'static str {
    match base {
        GrooveBase::Quarter => "1/4",
        GrooveBase::Eighth => "1/8",
        GrooveBase::EighthTriplet => "1/8T",
        GrooveBase::Sixteenth => "1/16",
        GrooveBase::SixteenthTriplet => "1/16T",
        GrooveBase::ThirtySecond => "1/32",
    }
}

/// A percentage as a field reads it: `100 %`, `-40 %`, or `62.5 %`.
fn percent_text(percent: f64) -> String {
    bl_shell::transport::percent_text(percent)
}

/// The pool, which takes the keyboard focus (R24).
#[must_use]
pub fn pool_id() -> Id {
    Id::new("groove-pool")
}

fn region_id() -> Id {
    Id::new("region-groove-pool")
}

/// A groove's row.
#[must_use]
pub fn row_id(groove: GrooveId) -> Id {
    Id::new(("groove-pool-row", groove))
}

/// One of a groove's row buttons.
#[must_use]
pub fn button_id(groove: GrooveId, button: &str) -> Id {
    Id::new(("groove-pool-button", groove, button))
}

/// One of a groove's fields, by its column's name.
#[must_use]
pub fn field_id(groove: GrooveId, name: &str) -> Id {
    Id::new(("groove-pool-field", groove, name))
}

/// The footer's Auto Load Groove switch.
#[must_use]
pub fn auto_load_id() -> Id {
    Id::new("groove-pool-auto-load")
}

/// The footer's hot-swap button, for the auto-load groove.
#[must_use]
pub fn auto_load_hot_swap_id() -> Id {
    Id::new("groove-pool-auto-load-hot-swap")
}

/// The footer's Global Amount field.
#[must_use]
pub fn global_amount_id() -> Id {
    Id::new("groove-pool-global-amount")
}

fn rename_id() -> Id {
    Id::new("groove-pool-rename")
}

/// Where the pool goes in a browser column: the lower half, the browser
/// above it shrinking to make room (L-GRV-001). Returns the browser's part
/// and the pool's.
#[must_use]
pub fn split(column: Rect) -> (Rect, Rect) {
    let height = ((column.height() - GAP) / 2.0).floor().max(0.0);
    let pool = Rect::from_min_max(pos2(column.left(), column.bottom() - height), column.max);
    let browser = Rect::from_min_max(column.min, pos2(column.right(), pool.top() - GAP));
    (browser, pool)
}

/// Whether the pool, or one of its controls, has the keyboard focus.
#[must_use]
fn has_focus(ctx: &egui::Context, drawn: &[Id]) -> bool {
    ctx.memory(|m| m.focused())
        .is_some_and(|id| drawn.contains(&id))
}

/// A name being typed in a groove's place.
#[derive(Debug)]
struct Rename {
    groove: GrooveId,
    text: String,
    /// Whether the field has been given the keyboard yet.
    started: bool,
}

/// The pool's view between frames.
#[derive(Debug, Default)]
pub struct GroovePoolUi {
    /// How far it is scrolled: across the columns, and down the rows.
    scroll: Vec2,
    /// How tall the rows' area was last frame, for scrolling a row into view.
    rows_height: f32,
    /// The selected groove.
    selected: Option<GrooveId>,
    /// A name being typed, for a groove.
    renaming: Option<Rename>,
    /// The focus requests of the browser the pool has acted on.
    seen_focus: u64,
    /// The controls drawn last frame, the ones the pool's keys are for.
    drawn: Vec<Id>,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl GroovePoolUi {
    /// A pool scrolled to its top left, with nothing selected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a name is being typed, so the keys are the field's.
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.renaming.is_some()
    }

    /// Drops a name being typed, for a frame the pool isn't drawn in.
    pub fn hidden(&mut self) {
        self.renaming = None;
    }

    /// Draws the pool in `rect` and adds what the person did to `events`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let state = shell.groove_pool();
        let c = &theme.colors;
        ui.painter().rect_filled(rect, 0.0, c.panel.0);
        let region = region_id();
        let _ = ui.interact(rect, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Groove Pool".to_owned());
            node.set_bounds(crate::browser::bounds(rect));
        });
        if self
            .selected
            .is_some_and(|id| !state.grooves.iter().any(|g| g.groove.id == id))
        {
            self.selected = None;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|rename| !state.grooves.iter().any(|g| g.groove.id == rename.groove))
        {
            self.renaming = None;
        }
        // A command moved the focus to the pool (R24).
        let view = shell.browser_view();
        if view.focus_requests() != self.seen_focus {
            self.seen_focus = view.focus_requests();
            if view.focus() == bl_shell::browser::Part::GroovePool
                && !has_focus(ui.ctx(), &self.drawn)
            {
                ui.memory_mut(|m| m.request_focus(pool_id()));
            }
        }
        let drawn_before = std::mem::take(&mut self.drawn);
        self.drawn = vec![pool_id()];

        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FOOTER), rect.max);
        let rows = Rect::from_min_max(
            pos2(rect.left(), header.bottom()),
            pos2(rect.right(), footer.top()),
        );
        let content = Vec2::new(
            columns().last().map_or(0.0, |column| column.right) + MARGIN,
            state.grooves.len() as f32 * ROW,
        );
        self.rows_height = rows.height();
        let above_footer = Rect::from_min_max(rect.min, pos2(rect.right(), footer.top()));
        self.scroll = scrolled(ui, above_footer, self.scroll, rows.size(), content);

        // The table, the keyboard's target.
        let table = ui.interact(rows, pool_id(), Sense::click_and_drag());
        if table.clicked() || table.drag_started() {
            table.request_focus();
            self.focus_part(shell, events);
        }
        crate::focus::register(ui.ctx(), pool_id(), Area::Browser, None);
        ui.ctx().accesskit_node_builder(pool_id(), |node| {
            node.set_role(Role::Table);
            node.set_label("Groove Pool".to_owned());
            node.set_bounds(crate::browser::bounds(rows));
        });
        if state.grooves.is_empty() {
            ui.painter_at(rows).text(
                rows.center(),
                Align2::CENTER_CENTER,
                "Drop Clips or Grooves Here",
                theme.small_font(),
                c.text_dim.0,
            );
        }
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .accessibility_parent(pool_id()),
            |ui| {
                ui.set_clip_rect(rect);
                self.headers(ui, theme, header);
                ui.set_clip_rect(rows);
                let first = (self.scroll.y / ROW).floor().max(0.0) as usize;
                let shown = ((rows.height() / ROW).ceil() as usize + 1)
                    .min(state.grooves.len().saturating_sub(first));
                for (at, entry) in state.grooves.iter().skip(first).take(shown).enumerate() {
                    let top = rows.top() + (first + at) as f32 * ROW - self.scroll.y;
                    let row = Rect::from_min_size(pos2(rows.left(), top), vec2(rows.width(), ROW));
                    self.row(ui, shell, theme, &state, entry, row, events);
                }
                // A name typed in a row scrolled out of view is dropped, so
                // its field doesn't hold the keys while nothing shows it.
                if self.renaming.as_ref().is_some_and(|rename| {
                    !state.grooves[first..first + shown]
                        .iter()
                        .any(|g| g.groove.id == rename.groove)
                }) {
                    self.renaming = None;
                }
            },
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(footer)
                .accessibility_parent(region),
            |ui| {
                ui.set_clip_rect(footer);
                self.footer(ui, theme, &state, footer, events);
            },
        );
        // A control of the pool gained the keyboard: the pool has it, so the
        // browser's keys give way.
        if has_focus(ui.ctx(), &self.drawn)
            && !has_focus(ui.ctx(), &drawn_before)
            && view.focus() != bl_shell::browser::Part::GroovePool
        {
            self.focus_part(shell, events);
        }
        self.keys(ui, &state, events);
        self.drops(ui, theme, rect, events);
    }

    /// Tells the shell the pool has the keyboard, so the browser's keys give
    /// way, unless it knows already.
    fn focus_part(&self, shell: &Shell, events: &mut Vec<UiEvent>) {
        if shell.browser_view().focus() != bl_shell::browser::Part::GroovePool {
            invoke(
                events,
                bl_shell::browser_commands::FOCUS_BROWSER,
                json!({ "part": "groove_pool" }),
            );
        }
    }

    /// Draws the column headers (L-GRV-002).
    fn headers(&self, ui: &Ui, theme: &Theme, header: Rect) {
        let c = &theme.colors;
        ui.painter().rect_filled(header, 0.0, c.bar.0);
        let painter = ui.painter_at(header);
        for (column, text) in columns()
            .into_iter()
            .filter(|column| column.header.is_some())
            .map(|column| (column, HEADERS[column.header.unwrap_or(0)]))
        {
            painter.text(
                pos2(
                    header.left() + column.left - self.scroll.x + 2.0,
                    header.center().y,
                ),
                Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(HEADER_TEXT),
                c.text_dim.0,
            );
        }
    }

    /// Draws one groove's row: its name, its three buttons and its five
    /// fields (R25 to R27, L-GRV-003).
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        state: &GroovePoolState,
        entry: &GroovePoolEntry,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let groove = &entry.groove;
        let id = groove.id;
        let selected = self.selected == Some(id);
        let response = ui.interact(rect, row_id(id), Sense::hover());
        if selected {
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, c.accent.0.gamma_multiply(0.45));
        } else if response.hovered() {
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, c.control_hover.0);
        }
        let row = row_id(id);
        ui.ctx().accesskit_node_builder(row, |node| {
            node.set_role(Role::Row);
            node.set_label(groove.name.clone());
            node.set_bounds(crate::browser::bounds(rect));
            node.set_selected(selected);
        });
        let scroll_x = self.scroll.x;
        let x = |left: f32| rect.left() + left - scroll_x;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .accessibility_parent(row),
            |ui| {
                let [name, buttons, base, values @ ..] = columns();
                // The name: a click selects it, a drag takes the groove along
                // (R26, R28), and Cmd+R types a new one in its place.
                let name_rect = Rect::from_min_max(
                    pos2(x(name.left), rect.top()),
                    pos2(x(name.right), rect.bottom()),
                );
                if self
                    .renaming
                    .as_ref()
                    .is_some_and(|rename| rename.groove == id)
                {
                    self.rename_field(ui, theme, name_rect, events);
                } else {
                    let hit = ui.interact(
                        name_rect,
                        Id::new(("groove-pool-name", id)),
                        Sense::click_and_drag(),
                    );
                    ui.painter_at(name_rect).text(
                        pos2(name_rect.left() + 2.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &groove.name,
                        theme.small_font(),
                        c.text.0,
                    );
                    if hit.clicked() || hit.drag_started() {
                        self.selected = Some(id);
                        ui.memory_mut(|m| m.request_focus(pool_id()));
                        self.focus_part(shell, events);
                    }
                    if hit.drag_started() {
                        DragAndDrop::set_payload(
                            ui.ctx(),
                            GrooveDrag {
                                groove: Some(id),
                                path: None,
                            },
                        );
                    }
                    if hit.dragged() {
                        crate::browser::drag_label(ui, theme, &groove.name);
                    }
                    ui.ctx().accesskit_node_builder(hit.id, |node| {
                        node.set_role(Role::Cell);
                        node.set_label(groove.name.clone());
                    });
                }
                // The three buttons (R27).
                let mut left = buttons.left;
                for button in Button::ALL {
                    let at = Rect::from_min_size(
                        pos2(x(left), rect.center().y - BUTTON / 2.0),
                        vec2(BUTTON, BUTTON),
                    );
                    left += BUTTON + BUTTON_GAP;
                    self.row_button(ui, theme, state, entry, button, at, events);
                }
                // Base, then the four percentages (R26).
                let base_rect = Rect::from_min_size(
                    pos2(x(base.left), rect.center().y - 7.5),
                    vec2(BASE, 15.0),
                );
                let choices: Vec<Choice<GrooveBase>> = GrooveBase::ALL
                    .iter()
                    .map(|b| Choice {
                        label: base_text(*b).to_owned(),
                        value: *b,
                        leds: widgets::NO_LEDS,
                    })
                    .collect();
                let salt = format!("groove-pool-base-{}", id.0.get());
                crate::focus::register(
                    ui.ctx(),
                    Id::new(("chooser", salt.as_str())),
                    Area::Browser,
                    None,
                );
                self.drawn.push(Id::new(("chooser", salt.as_str())));
                if let Some(chosen) = widgets::chooser(
                    ui,
                    theme,
                    &salt,
                    "Base",
                    base_rect,
                    &choices,
                    Some(&groove.base),
                    true,
                ) {
                    invoke(
                        events,
                        "groove.set",
                        json!({ "groove": id, "base": chosen }),
                    );
                }
                for (param, column) in Param::ALL.into_iter().zip(values) {
                    let at = Rect::from_min_size(
                        pos2(x(column.left), rect.center().y - 7.5),
                        vec2(VALUE, 15.0),
                    );
                    self.percent(
                        ui,
                        theme,
                        id,
                        param,
                        param.of(groove),
                        at,
                        entry.active,
                        events,
                    );
                }
            },
        );
    }

    /// One percentage field of a groove: dragged, stepped with the arrow
    /// keys or typed, each gesture one undo step (R26).
    #[expect(clippy::too_many_arguments, reason = "the field's inputs")]
    fn percent(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        groove: GrooveId,
        param: Param,
        value: f64,
        rect: Rect,
        active: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let id = field_id(groove, param.name());
        self.drawn.push(id);
        let (low, high) = param.range();
        // An inactive groove's fields are drawn grey, and still edit.
        let (entry, response) = field(
            ui,
            theme,
            id,
            param.name(),
            rect,
            &percent_text(value),
            Place {
                area: Area::Browser,
                dim: !active,
            },
        );
        let label = format!("Change {}", param.name());
        gesture(ui, &response, &label, events);
        let fine = ui.input(|i| i.modifiers.shift);
        let next = entry.and_then(|entry| match entry {
            Entry::Steps(steps) => Some(value + f64::from(steps) * if fine { 0.1 } else { 1.0 }),
            Entry::Typed(text) => text.trim().replace(',', ".").parse::<f64>().ok(),
        });
        if let Some(next) = next.filter(|next| next.is_finite()) {
            let next = ((next.clamp(low, high)) * 10.0).round() / 10.0;
            if (next - value).abs() > f64::EPSILON {
                invoke(
                    events,
                    "groove.set",
                    json!({ "groove": groove, param.key(): next }),
                );
            }
        }
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_label(param.name().to_owned());
            node.set_numeric_value(value);
            node.set_min_numeric_value(low);
            node.set_max_numeric_value(high);
        });
    }

    /// A row's hot-swap, save or commit button (R27, L-GRV-003).
    #[expect(clippy::too_many_arguments, reason = "the button's inputs")]
    fn row_button(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        state: &GroovePoolState,
        entry: &GroovePoolEntry,
        button: Button,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let groove = entry.groove.id;
        let id = button_id(groove, button.name());
        let lit =
            button == Button::HotSwap && state.view.hot_swap == Some(GrooveHotSwap::Groove(groove));
        // Commit works on the clips that use the groove: an inactive groove
        // has none (R27).
        let enabled = button != Button::Commit || entry.active;
        self.drawn.push(id);
        if !icon_button(
            ui,
            theme,
            id,
            button.name(),
            &entry.groove.name,
            rect,
            button,
            lit,
            enabled,
            Area::Browser,
        ) {
            return;
        }
        match button {
            Button::HotSwap => invoke(
                events,
                bl_shell::groove_commands::HOT_SWAP,
                json!({ "groove": groove }),
            ),
            Button::Save => invoke(
                events,
                bl_shell::groove_commands::SAVE,
                json!({ "groove": groove }),
            ),
            Button::Commit => invoke(
                events,
                bl_shell::groove_commands::COMMIT,
                json!({ "groove": groove }),
            ),
        }
    }

    /// The name of the selected groove, typed in its place (R26).
    fn rename_field(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
        let Some(rename) = self.renaming.as_mut() else {
            return;
        };
        let first = !rename.started;
        let edit = egui::TextEdit::singleline(&mut rename.text)
            .id(rename_id())
            .font(theme.small_font())
            .margin(vec2(2.0, 0.0));
        let response = ui.put(rect.shrink2(vec2(0.0, 1.0)), edit);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Groove Name".to_owned());
        });
        if first {
            rename.started = true;
            response.request_focus();
            let len = rename.text.chars().count();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), rename_id()) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(len),
                    )));
                state.store(ui.ctx(), rename_id());
            }
        }
        // Enter keeps the name; Esc, or the focus going elsewhere, drops it.
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        if (response.lost_focus() || escape) && !first {
            let entered = ui.input(|i| i.key_pressed(Key::Enter));
            let (groove, text) = (rename.groove, rename.text.trim().to_owned());
            self.renaming = None;
            if entered && !escape && !text.is_empty() {
                invoke(
                    events,
                    "groove.rename",
                    json!({ "groove": groove, "name": text }),
                );
            }
            ui.memory_mut(|m| m.request_focus(pool_id()));
        }
    }

    /// The footer (R25, R31, L-GRV-005, L-GRV-006): the section's name, the
    /// Auto Load Groove switch and its hot-swap button, then the Global
    /// Amount at the right.
    fn footer(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        state: &GroovePoolState,
        footer: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        ui.painter().rect_filled(footer, 0.0, c.bar.0);
        let middle = footer.center().y;
        let name = ui.painter().text(
            pos2(footer.left() + 6.0, middle),
            Align2::LEFT_CENTER,
            "Groove Pool",
            theme.small_font(),
            c.text.0,
        );
        // The Auto Load Groove switch.
        let switch = Rect::from_min_size(pos2(name.right() + 8.0, middle - SWITCH.y / 2.0), SWITCH);
        self.drawn.push(auto_load_id());
        if let Some(on) = auto_load_switch(ui, theme, auto_load_id(), switch, state.auto_load) {
            invoke(events, "groove.set_auto_load", json!({ "enabled": on }));
        }
        // The button that chooses the auto-load groove (R31).
        let swap = Rect::from_min_size(
            pos2(switch.right() + BUTTON_GAP + 2.0, middle - BUTTON / 2.0),
            vec2(BUTTON, BUTTON),
        );
        self.drawn.push(auto_load_hot_swap_id());
        let lit = state.view.hot_swap == Some(GrooveHotSwap::AutoLoad);
        if icon_button(
            ui,
            theme,
            auto_load_hot_swap_id(),
            "Hot-Swap Auto Load Groove",
            "",
            swap,
            Button::HotSwap,
            lit,
            true,
            Area::Browser,
        ) {
            invoke(
                events,
                bl_shell::groove_commands::HOT_SWAP,
                json!({ "auto_load": true }),
            );
        }
        // The Global Amount, at the right (R25, R26).
        let amount = Rect::from_min_size(
            pos2(footer.right() - 6.0 - AMOUNT.x, middle - AMOUNT.y / 2.0),
            AMOUNT,
        );
        ui.painter().text(
            pos2(amount.left() - 4.0, middle),
            Align2::RIGHT_CENTER,
            "Global Amount",
            theme.small_font(),
            c.text_dim.0,
        );
        self.drawn.push(global_amount_id());
        let value = state.global_amount;
        let (entry, response) = field(
            ui,
            theme,
            global_amount_id(),
            "Global Amount",
            amount,
            &percent_text(value),
            Place {
                area: Area::Browser,
                dim: false,
            },
        );
        gesture(ui, &response, "Change Global Amount", events);
        let fine = ui.input(|i| i.modifiers.shift);
        let next = entry.and_then(|entry| match entry {
            Entry::Steps(steps) => Some(value + f64::from(steps) * if fine { 0.1 } else { 1.0 }),
            Entry::Typed(text) => text.trim().replace(',', ".").parse::<f64>().ok(),
        });
        if let Some(next) = next.filter(|next| next.is_finite()) {
            let next =
                (next.clamp(0.0, bl_shell::transport::GLOBAL_AMOUNT_MAX) * 10.0).round() / 10.0;
            if (next - value).abs() > f64::EPSILON {
                invoke(
                    events,
                    bl_shell::groove_commands::SET_GLOBAL_AMOUNT,
                    json!({ "amount": next }),
                );
            }
        }
        ui.ctx().accesskit_node_builder(global_amount_id(), |node| {
            node.set_label("Global Amount".to_owned());
            node.set_numeric_value(value);
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(bl_shell::transport::GLOBAL_AMOUNT_MAX);
        });
    }

    /// The keys the pool takes while it has the keyboard, not one of its
    /// fields (R26): Up and Down move between the grooves, Cmd+R renames the
    /// selected one and Delete deletes it.
    fn keys(&mut self, ui: &mut Ui, state: &GroovePoolState, events: &mut Vec<UiEvent>) {
        if !ui.memory(|m| m.has_focus(pool_id())) || self.renaming.is_some() {
            return;
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                pool_id(),
                egui::EventFilter {
                    vertical_arrows: true,
                    ..egui::EventFilter::default()
                },
            );
        });
        let presses: Vec<(Key, egui::Modifiers)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        let ids: Vec<GrooveId> = state.grooves.iter().map(|g| g.groove.id).collect();
        for (key, modifiers) in presses {
            let command = if cfg!(target_os = "macos") {
                modifiers.mac_cmd
            } else {
                modifiers.ctrl
            };
            match key {
                Key::ArrowUp | Key::ArrowDown if modifiers.is_none() || modifiers.shift => {
                    let at = self
                        .selected
                        .and_then(|id| ids.iter().position(|g| *g == id));
                    let next = match (key, at) {
                        (Key::ArrowDown, None) => 0,
                        (Key::ArrowDown, Some(at)) => (at + 1).min(ids.len().saturating_sub(1)),
                        (_, None) => ids.len().saturating_sub(1),
                        (_, Some(at)) => at.saturating_sub(1),
                    };
                    self.selected = ids.get(next).copied();
                    self.reveal(next);
                }
                Key::R if command && !modifiers.alt => {
                    if let Some((at, groove)) = state
                        .grooves
                        .iter()
                        .enumerate()
                        .find(|(_, g)| Some(g.groove.id) == self.selected)
                    {
                        self.reveal(at);
                        self.renaming = Some(Rename {
                            groove: groove.groove.id,
                            text: groove.groove.name.clone(),
                            started: false,
                        });
                    }
                }
                Key::Delete | Key::Backspace if modifiers.is_none() => {
                    if let Some(groove) = self.selected {
                        invoke(events, "groove.delete", json!({ "groove": groove }));
                    }
                }
                _ => {}
            }
        }
    }

    /// Scrolls the row at `index` into view.
    fn reveal(&mut self, index: usize) {
        let top = index as f32 * ROW;
        if top < self.scroll.y {
            self.scroll.y = top;
        } else if top + ROW > self.scroll.y + self.rows_height {
            self.scroll.y = top + ROW - self.rows_height;
        }
    }

    /// What is dropped on the pool (R28): a clip, from the Session or the
    /// Arrangement, extracts a groove; a groove file, from the browser or
    /// from Finder, loads.
    fn drops(&self, ui: &Ui, theme: &Theme, rect: Rect, events: &mut Vec<UiEvent>) {
        crate::browser::note_clip_target(
            ui,
            theme,
            rect,
            crate::browser::ClipDropTarget::GroovePool,
        );
        let ctx = ui.ctx();
        let pointer = ctx.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()));
        let over = pointer.is_some_and(|p| rect.contains(p));
        if let Some(drag) = DragAndDrop::payload::<GrooveDrag>(ctx) {
            if over && drag.path.is_some() {
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(2.0, theme.colors.accent.0),
                    StrokeKind::Inside,
                );
            }
            if over && ctx.input(|i| i.pointer.any_released()) {
                DragAndDrop::clear_payload(ctx);
                if let Some(path) = &drag.path {
                    invoke(
                        events,
                        bl_shell::groove_commands::LOAD,
                        json!({ "path": path }),
                    );
                }
            }
        }
        // A groove file dropped from Finder or Explorer.
        let dropped: Vec<String> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_string_lossy().into_owned())
                .filter(|path| {
                    bl_shell::browser::listing::file_kind(path)
                        == Some(bl_shell::browser_commands::ItemKind::Groove)
                })
                .collect()
        });
        if over {
            for path in dropped {
                invoke(
                    events,
                    bl_shell::groove_commands::LOAD,
                    json!({ "path": path }),
                );
            }
        }
    }
}

/// Opens a named gesture while a field is dragged, so the drag is one undo
/// step, and closes it when the drag ends (R26).
fn gesture(ui: &Ui, response: &egui::Response, label: &str, events: &mut Vec<UiEvent>) {
    let flag = response.id.with("groove-gesture");
    let open = ui.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false);
    if response.dragged() && !open {
        ui.data_mut(|d| d.insert_temp(flag, true));
        events.push(UiEvent::BeginNamedGesture(label.to_owned()));
    } else if !response.dragged() && open {
        ui.data_mut(|d| d.remove::<bool>(flag));
        events.push(UiEvent::EndGesture);
    }
}

/// Where a column sits in a row, from the row's left edge, and which header
/// it has.
#[derive(Clone, Copy, Debug)]
struct Column {
    left: f32,
    right: f32,
    header: Option<usize>,
}

/// The columns, in order: the name, the buttons, Base and the four
/// percentages.
fn columns() -> [Column; 7] {
    let mut left = MARGIN;
    let mut next = |width: f32, header: Option<usize>| {
        let column = Column {
            left,
            right: left + width,
            header,
        };
        left += width + COLUMN_GAP;
        column
    };
    [
        next(NAME, Some(0)),
        next(3.0 * (BUTTON + BUTTON_GAP) - BUTTON_GAP, None),
        next(BASE, Some(1)),
        next(VALUE, Some(2)),
        next(VALUE, Some(3)),
        next(VALUE, Some(4)),
        next(VALUE, Some(5)),
    ]
}

/// Scrolls with the wheel and the trackpad while the pointer is over the
/// pool; returns the new offset, kept within the content.
fn scrolled(ui: &Ui, area: Rect, offset: Vec2, shown: Vec2, content: Vec2) -> Vec2 {
    let over = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| area.contains(p));
    let delta = if over {
        ui.input(|i| i.smooth_scroll_delta())
    } else {
        Vec2::ZERO
    };
    vec2(
        (offset.x - delta.x).clamp(0.0, (content.x - shown.x).max(0.0)),
        (offset.y - delta.y).clamp(0.0, (content.y - shown.y).max(0.0)),
    )
}

/// A small round button with Beatlayer's own mark: hot-swap (a ring), save
/// (a tray with an arrow into it) or commit (a tick). Returns whether it was
/// clicked, or Enter was pressed on it.
#[expect(clippy::too_many_arguments, reason = "the button's inputs")]
pub(crate) fn icon_button(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    name: &str,
    of: &str,
    rect: Rect,
    kind: Button,
    lit: bool,
    enabled: bool,
    area: Area,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    // A disabled button is no stop for Tab.
    if enabled {
        crate::focus::register(ui.ctx(), id, area, None);
    }
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(2.0));
    let face = if lit {
        c.accent.0
    } else if response.hovered() && enabled {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.circle_filled(rect.center(), rect.width() / 2.0, face);
    let ink = if !enabled {
        c.pending.0
    } else if lit {
        c.accent_text.0
    } else {
        c.text.0
    };
    let centre = rect.center();
    let s = rect.width() * 0.22;
    match kind {
        Button::HotSwap => {
            painter.circle_filled(centre, s * 0.8, ink);
            painter.circle_stroke(centre, s * 1.9, Stroke::new(1.0, ink));
        }
        Button::Save => {
            painter.line_segment(
                [centre + vec2(0.0, -s * 1.6), centre + vec2(0.0, s * 0.6)],
                Stroke::new(1.2, ink),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    centre + vec2(-s * 1.0, s * 0.0),
                    centre + vec2(s * 1.0, s * 0.0),
                    centre + vec2(0.0, s * 1.2),
                ],
                ink,
                Stroke::NONE,
            ));
            painter.line_segment(
                [
                    centre + vec2(-s * 1.6, s * 1.9),
                    centre + vec2(s * 1.6, s * 1.9),
                ],
                Stroke::new(1.2, ink),
            );
        }
        Button::Commit => {
            let points: Vec<Pos2> = vec![
                centre + vec2(-s * 1.6, 0.0),
                centre + vec2(-s * 0.5, s * 1.3),
                centre + vec2(s * 1.7, -s * 1.4),
            ];
            painter.add(Shape::line(points, Stroke::new(1.4, ink)));
        }
    }
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    if response.clicked() {
        response.request_focus();
    }
    let label = name.to_owned();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &label));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(label.clone());
        if !of.is_empty() {
            node.set_description(of.to_owned());
        }
        if kind == Button::HotSwap {
            node.set_toggled(if lit {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            });
        }
    });
    enabled
        && (response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
}

/// The footer's Auto Load Groove switch (R31): lit while it's on. Returns
/// the new state when it's clicked or Enter is pressed on it.
fn auto_load_switch(ui: &mut Ui, theme: &Theme, id: Id, rect: Rect, on: bool) -> Option<bool> {
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, Area::Browser, None);
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if on {
            c.accent.0
        } else if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Auto",
        theme.small_font(),
        if on { c.accent_text.0 } else { c.text.0 },
    );
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Outside,
        );
    }
    if response.clicked() {
        response.request_focus();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, "Auto Load Groove"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Switch);
        node.set_label("Auto Load Groove".to_owned());
        node.set_value(if on { "on" } else { "off" }.to_owned());
    });
    (response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
        .then_some(!on)
}
