//! The side browser: a virtualised list of 100,000 file names.

use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div, px, rgb,
    uniform_list,
};

use crate::data::{BROWSER_FILES, browser_file_name};

pub const BROWSER_W: f32 = 260.0;
const ROW_H: f32 = 20.0;

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

pub struct Browser {
    selected: Option<usize>,
    pub scroll: UniformListScrollHandle,
}

impl Browser {
    pub fn new() -> Self {
        Self {
            selected: None,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("browser")
            .role(Role::Group)
            .aria_label("Browser")
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(0x23262B))
            .border_l_1()
            .border_color(hex(0x15171A))
            .text_size(px(12.0))
            .text_color(hex(0xC8CDD4))
            .child(
                div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .text_color(hex(0x8A9099))
                    .child(SharedString::new_static("Browser: 100,000 files")),
            )
            .child(
                uniform_list(
                    "files",
                    BROWSER_FILES,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|ix| {
                                let name = SharedString::from(browser_file_name(ix));
                                let selected = this.selected == Some(ix);
                                div()
                                    .id(ix)
                                    .role(Role::ListItem)
                                    .aria_label(name.clone())
                                    .aria_position_in_set(ix + 1)
                                    .aria_size_of_set(BROWSER_FILES)
                                    .aria_selected(selected)
                                    .h(px(ROW_H))
                                    .px(px(8.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .bg(if selected {
                                        hex(0x3B5B8C)
                                    } else if ix % 2 == 0 {
                                        hex(0x23262B)
                                    } else {
                                        hex(0x25282E)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected = Some(ix);
                                        cx.notify();
                                    }))
                                    .child(name)
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .id("file-list")
                .role(Role::List)
                .aria_label("Files")
                .flex_1(),
            )
    }
}
