//! The user's own info text in the Info View: the item under the pointer
//! shows its text (R11), assistive technology reads it as the item's
//! description, and Edit Info Text turns the view into a text field (R10).
//!
//! Spec: info-view R10, R11.

use bl_shell::{InfoTarget, Shell};
use egui::text::{CCursor, CCursorRange};
use egui::{Align2, Context, Id, Key, Modifiers, Rect, Response, TextEdit, Ui, pos2};

use crate::main_window::{InfoViewContent, UiEvent};
use crate::theme::Theme;

fn hover_id() -> Id {
    Id::new("info-text-hover")
}

fn edit_id() -> Id {
    Id::new("info-view-edit")
}

/// An item's drawing says what it is: with info text of the user's own,
/// assistive technology reads it as the item's description, and the Info
/// View shows it while the pointer is over `response` (R11).
pub(crate) fn describe(ctx: &Context, response: &Response, shell: &Shell, target: InfoTarget) {
    let Some((_, text)) = shell.info_text_shown(target) else {
        return;
    };
    if response.hovered() {
        hover(ctx, target);
    }
    ctx.accesskit_node_builder(response.id, |node| node.set_description(text));
}

/// Says the pointer is over `target`, for the Info View to read.
pub(crate) fn hover(ctx: &Context, target: InfoTarget) {
    ctx.data_mut(|d| d.insert_temp(hover_id(), target));
}

/// What the Info View shows for the item the pointer is over, if it has
/// info text. Taken once a frame.
pub(crate) fn take_hovered(ctx: &Context, shell: &Shell) -> Option<InfoViewContent> {
    let target = ctx.data_mut(|d| {
        let target = d.get_temp::<InfoTarget>(hover_id());
        d.remove::<InfoTarget>(hover_id());
        target
    })?;
    let (title, body) = shell.info_text_shown(target)?;
    Some(InfoViewContent {
        title,
        body,
        shortcut: None,
    })
}

/// The field Edit Info Text opens in the Info View.
#[derive(Debug)]
pub(crate) struct InfoEditing {
    target: InfoTarget,
    /// The text so far; `None` until the first frame reads it from the set.
    text: Option<String>,
}

impl InfoEditing {
    pub(crate) fn new(target: InfoTarget) -> Self {
        Self { target, text: None }
    }
}

/// Draws the field over the Info View `rect`. Return keeps the text,
/// Option-Return (Alt-Return) starts a new line, Esc drops the change, and
/// the focus leaving keeps it (R10).
pub(crate) fn edit(
    ui: &mut Ui,
    theme: &Theme,
    shell: &Shell,
    rect: Rect,
    editing: &mut Option<InfoEditing>,
    events: &mut Vec<UiEvent>,
) {
    let Some(state) = editing.as_mut() else {
        return;
    };
    let Some((name, current)) = shell.info_item(state.target) else {
        *editing = None;
        return;
    };
    let id = edit_id();
    let first = state.text.is_none();
    let text = state.text.get_or_insert_with(|| current.clone());
    let (newline, enter, escape) = ui.input_mut(|i| {
        (
            i.consume_key(Modifiers::ALT, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if newline {
        insert_newline(ui.ctx(), id, text);
    }
    if first {
        ui.memory_mut(|m| m.request_focus(id));
    }
    ui.painter().text(
        pos2(rect.left() + 8.0, rect.top() + 8.0),
        Align2::LEFT_TOP,
        &name,
        egui::FontId::proportional(theme.sizes.heading),
        theme.colors.text.0,
    );
    let field = Rect::from_min_max(
        pos2(rect.left() + 8.0, rect.top() + 28.0),
        pos2(rect.right() - 8.0, rect.bottom() - 8.0),
    );
    let response = ui.put(
        field,
        TextEdit::multiline(text)
            .id(id)
            .font(theme.small_font())
            .desired_width(field.width()),
    );
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Info text".to_owned());
    });
    if enter || escape || (!first && response.lost_focus()) {
        if !escape && *text != current {
            events.push(UiEvent::Invoke(Shell::info_text_invocation(
                state.target,
                text,
            )));
        }
        ui.memory_mut(|m| m.surrender_focus(id));
        *editing = None;
    }
}

/// Puts a line break where the text cursor is.
fn insert_newline(ctx: &Context, id: Id, text: &mut String) {
    let mut state = TextEdit::load_state(ctx, id).unwrap_or_default();
    let at = state
        .cursor
        .char_range()
        .map_or_else(|| text.chars().count(), |range| range.primary.index.into());
    let byte = text
        .char_indices()
        .nth(at)
        .map_or(text.len(), |(byte, _)| byte);
    text.insert(byte, '\n');
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(at + 1))));
    state.store(ctx, id);
}
