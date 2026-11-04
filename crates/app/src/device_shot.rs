//! `--device-shot DEVICE [--page PAGE] [--expanded]` with `--screenshot FILE`:
//! adds a new device to a track of a new set, shows the page asked for (and
//! the device's expanded part, if asked), draws the
//! window offscreen as `--screenshot` does, and saves only the device's
//! panel, at 100% zoom, so a pull request can put Beatlayer's panel beside
//! Live's picture of the same device (`cargo xtask device-shot`).

use bl_engine::DeviceType;
use bl_model::{DeviceId, DeviceKind, DeviceRole, TrackKind};
use bl_shell::Shell;
use egui::{ColorImage, Rect};
use serde_json::json;

/// The device and page `--device-shot` names.
#[derive(Clone, Debug)]
pub struct Request {
    /// The device's name, or its native type: case and the difference
    /// between a space and a hyphen don't matter.
    pub device: String,
    /// A page to show, by name, on every pages node that has one.
    pub page: Option<String>,
    /// Whether to show the device's expanded part.
    pub expanded: bool,
}

/// The name as compared: lower case, with spaces and hyphens as one.
fn plain(name: &str) -> String {
    name.to_lowercase().replace(['-', '_', ' '], "")
}

/// The device type the request names.
///
/// # Errors
///
/// A message naming the nearest choices where nothing matches.
fn find(request: &Request) -> Result<DeviceType, String> {
    let types = bl_devices::device_types();
    let wanted = plain(&request.device);
    types
        .iter()
        .find(|t| {
            plain(&t.name) == wanted
                || matches!(&t.kind, DeviceKind::Native { device_type } if plain(device_type) == wanted)
        })
        .cloned()
        .ok_or_else(|| {
            let mut names: Vec<&str> = types
                .iter()
                .filter(|t| !t.description.params.is_empty())
                .map(|t| t.name.as_str())
                .collect();
            names.sort_unstable();
            format!(
                "no device called {}; the described devices are {}",
                request.device,
                names.join(", ")
            )
        })
}

/// Adds `device` to the first track of `kind` and selects that track.
fn add(shell: &mut Shell, kind: TrackKind, device: serde_json::Value) -> Result<DeviceId, String> {
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == kind)
        .map(bl_model::Track::id)
        .ok_or("the new set has no track to put the device on")?;
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .map_err(|e| e.to_string())?;
    let added = shell
        .run("device.add", json!({ "device": device }))
        .map_err(|e| e.to_string())?;
    serde_json::from_value(added["device"].clone())
        .map_err(|e| format!("device.add didn't say which device it added: {e}"))
}

/// Adds the device to the first track it fits (a MIDI track for an
/// instrument or a MIDI effect, an audio track for an audio effect), selects
/// that track, and shows the page.
///
/// # Errors
///
/// A message if the device or page isn't found, or a command fails.
pub fn prepare(shell: &mut Shell, request: &Request) -> Result<DeviceId, String> {
    // A Drum Rack isn't a described device: it is a rack of the drum kind.
    if plain(&request.device) == "drumrack" {
        return add(
            shell,
            TrackKind::Midi,
            json!({
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            }),
        );
    }
    let device_type = find(request)?;
    let kind = match device_type.role {
        DeviceRole::AudioEffect => TrackKind::Audio,
        DeviceRole::Instrument | DeviceRole::MidiEffect => TrackKind::Midi,
    };
    let device = json!({
        "kind": device_type.kind,
        "name": device_type.name,
        "role": device_type.role,
    });
    let id = add(shell, kind, device)?;
    if request.expanded {
        shell
            .invoke("device.expand", json!({ "device": id, "expanded": true }))
            .map_err(|e| e.to_string())?;
    }
    if let Some(page) = &request.page {
        let wanted = plain(page);
        let mut shown = false;
        for node in device_type.description.layout.pages() {
            if let Some(name) = node.pages.iter().find(|p| plain(&p.name) == wanted) {
                shell
                    .invoke(
                        "device.set_page",
                        json!({ "device": id, "node": node.name, "page": name.name }),
                    )
                    .map_err(|e| e.to_string())?;
                shown = true;
            }
        }
        if !shown {
            let names: Vec<&str> = device_type
                .description
                .layout
                .pages()
                .iter()
                .flat_map(|n| n.pages.iter().map(|p| p.name.as_str()))
                .collect();
            return Err(format!(
                "{} has no page called {page}; its pages are {}",
                device_type.name,
                names.join(", ")
            ));
        }
    }
    Ok(id)
}

/// The device's frame in the image of the window: `image` is `pixels` per
/// layout point. The frame gets a margin of 4 points all round.
#[must_use]
pub fn crop(image: &ColorImage, frame: Rect, pixels: f32) -> ColorImage {
    let [width, height] = image.size;
    let margin = 4.0;
    let from = |v: f32, size: usize| ((v * pixels).floor().max(0.0) as usize).min(size);
    let (x0, y0) = (
        from(frame.left() - margin, width),
        from(frame.top() - margin, height),
    );
    let (x1, y1) = (
        from(frame.right() + margin, width),
        from(frame.bottom() + margin, height),
    );
    let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        pixels.extend_from_slice(&image.pixels[y * width + x0..y * width + x1]);
    }
    ColorImage::new([x1 - x0, y1 - y0], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    #[test]
    fn a_device_is_found_by_name_whatever_the_case_or_hyphen() {
        for name in ["Arpeggiator", "arpeggiator", "ARPEGGIATOR"] {
            let found = find(&Request {
                device: name.to_owned(),
                page: None,
                expanded: false,
            });
            assert_eq!(found.unwrap().name, "Arpeggiator");
        }
        assert!(
            find(&Request {
                device: "auto filter".to_owned(),
                page: None,
                expanded: false
            })
            .is_ok()
        );
        let missing = find(&Request {
            device: "nothing".to_owned(),
            page: None,
            expanded: false,
        })
        .unwrap_err();
        assert!(missing.contains("Arpeggiator"), "{missing}");
    }

    #[test]
    fn a_crop_keeps_the_frame_and_a_margin_of_four_points() {
        let size = [100, 80];
        let pixels: Vec<Color32> = (0..size[0] * size[1])
            .map(|i| Color32::from_gray((i % 251) as u8))
            .collect();
        let image = ColorImage::new(size, pixels);
        let frame = Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 10.0));
        let cropped = crop(&image, frame, 2.0);
        // (20 + 8) by (10 + 8) points at 2 pixels a point.
        assert_eq!(cropped.size, [56, 36]);
        assert_eq!(cropped.pixels[0], image.pixels[12 * 100 + 12]);
        // A frame at the window's edge is cut there, not past it.
        let edge = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        assert_eq!(crop(&image, edge, 1.0).size, [100, 80]);
    }
}
