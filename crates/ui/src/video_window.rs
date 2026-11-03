//! The Video Window (WP-9.26; the video spec R16 to R23): the frame at the
//! playhead in a window of its own, black around it and wherever no video
//! clip plays. The window itself, its place and its full screen are the
//! host's; this draws what is in it and turns a double-click into the
//! command that goes full screen or to the movie's own size.

use std::sync::Arc;

use bl_shell::video::SET_WINDOW;
use bl_shell::{Invocation, Shell, VideoFrame, VideoView};
use egui::accesskit::Role;
use egui::{
    Color32, ColorImage, Key, Rect, Sense, TextureHandle, TextureOptions, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::main_window::UiEvent;

/// How soon the window draws again while the frame it wants is still being
/// decoded.
const WAIT: std::time::Duration = std::time::Duration::from_millis(16);

/// The window's state between frames: the picture last uploaded as a
/// texture.
#[derive(Default)]
pub struct VideoWindowUi {
    uploaded: Option<(Arc<VideoFrame>, TextureHandle)>,
}

impl VideoWindowUi {
    /// Draws the window's contents: the picture scaled to fit with its
    /// aspect kept (R17). It draws the frames the video thread has already
    /// decoded (R23).
    pub fn show(&mut self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let view = shell.video_view();
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("video-picture"), Sense::click());
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        if let Some(frame) = &view.picture {
            let texture = self.texture(ui, frame);
            let shown = fit(rect, frame.width(), frame.height());
            ui.painter().image(
                texture.id(),
                shown,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if view.pending {
            ui.ctx().request_repaint_after(WAIT);
        }
        describe(ui, &response, &view);

        let full_screen = shell.video_window_full_screen();
        let set = |args| UiEvent::Invoke(Invocation::new(SET_WINDOW, args));
        if response.double_clicked() {
            // R20. Option-double-click (Alt) sets the movie's own size.
            events.push(if ui.input(|i| i.modifiers.alt) {
                set(json!({ "original_size": true }))
            } else {
                set(json!({ "full_screen": !full_screen }))
            });
        } else if full_screen && ui.input(|i| i.key_pressed(Key::Escape)) {
            events.push(set(json!({ "full_screen": false })));
        }
    }

    /// The texture holding `frame`, uploaded when it isn't the one last.
    fn texture(&mut self, ui: &Ui, frame: &Arc<VideoFrame>) -> &TextureHandle {
        if !self
            .uploaded
            .as_ref()
            .is_some_and(|(held, _)| Arc::ptr_eq(held, frame))
        {
            let image = ColorImage::from_rgba_unmultiplied(
                [frame.width() as usize, frame.height() as usize],
                frame.rgba(),
            );
            let texture = ui
                .ctx()
                .load_texture("video-frame", image, TextureOptions::LINEAR);
            self.uploaded = Some((Arc::clone(frame), texture));
        }
        &self.uploaded.as_ref().expect("uploaded above").1
    }
}

/// The largest rectangle of a picture's aspect inside `within`, centred.
fn fit(within: Rect, width: u32, height: u32) -> Rect {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (within.width() / w).min(within.height() / h);
    Rect::from_center_size(within.center(), vec2(w * scale, h * scale))
}

/// The picture for assistive technology (R22): an image described by the
/// clip's name and the frame's time.
fn describe(ui: &Ui, response: &egui::Response, view: &VideoView) {
    let description = match (view.clip, view.frame_time) {
        (Some(_), Some(time)) => format!("{}, {}", view.name, minutes(time)),
        (Some(_), None) => format!("{}, black", view.name),
        (None, _) => "Black, no video clip at the playhead".to_owned(),
    };
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Picture"));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Image);
        node.set_label("Picture");
        node.set_description(description);
    });
}

/// Seconds as minutes and seconds with the thousandths: 75.5 is `1:15.500`.
fn minutes(seconds: f64) -> String {
    let thousandths = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{}:{:02}.{:03}",
        thousandths / 60_000,
        thousandths / 1000 % 60,
        thousandths % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_with_its_aspect_kept() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 400.0));
        let wide = fit(window, 200, 100);
        assert_eq!((wide.width(), wide.height()), (400.0, 200.0));
        assert_eq!(wide.center(), window.center());
        let tall = fit(window, 100, 200);
        assert_eq!((tall.width(), tall.height()), (200.0, 400.0));
    }

    #[test]
    fn times_read_as_minutes_seconds_and_thousandths() {
        assert_eq!(minutes(75.5), "1:15.500");
        assert_eq!(minutes(0.0), "0:00.000");
    }
}
