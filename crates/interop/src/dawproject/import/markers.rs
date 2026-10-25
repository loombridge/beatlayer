//! Markers and metadata (the DAWproject spec, R63, R64).

use quick_xml::Reader;
use quick_xml::events::Event;

use super::*;

impl Importer<'_, '_> {
    /// Each marker of the Arrangement, or of a timeline of markers in its
    /// lanes, becomes a locator at its time with its name; of two at one
    /// tick, the first is kept (R63).
    pub(super) fn build_markers(&mut self, project: &DawProject) {
        let Some(arrangement) = &project.arrangement else {
            return;
        };
        let mut found: Vec<(model::Markers, TimeUnit)> = Vec::new();
        if let Some(markers) = &arrangement.markers {
            let unit = markers.info.time_unit.unwrap_or(TimeUnit::Beats);
            found.push((markers.clone(), unit));
        }
        for lane in &arrangement.lanes {
            collect_markers(lane, TimeUnit::Beats, &mut found);
        }
        for (markers, unit) in found {
            for marker in &markers.markers {
                let Some(time) = marker.time.filter(|time| time.is_finite()) else {
                    self.report
                        .add("a marker without its time", "in the markers", "left out");
                    continue;
                };
                let at = self.span(unit, time, Ticks::ZERO);
                let place = format!(
                    "the marker \"{}\"",
                    marker.info.name.as_deref().unwrap_or("")
                );
                if at < Ticks::ZERO {
                    self.report
                        .add("a marker before the start", &place, "left out");
                    continue;
                }
                if self
                    .editor
                    .document()
                    .locators()
                    .iter()
                    .any(|locator| locator.position == at)
                {
                    self.report
                        .add("a marker at the tick of an earlier one", &place, "left out");
                    continue;
                }
                let id = self.editor.new_id();
                let name = marker.info.name.clone().unwrap_or_default();
                self.apply(bl_model::Command::AddLocator {
                    locator: bl_model::Locator::new(id, at, name),
                });
            }
        }
    }

    /// The fields `metadata.xml` held are listed: they aren't kept (R64).
    pub(super) fn report_metadata(&mut self) {
        let Some(text) = &self.container.metadata else {
            return;
        };
        let mut reader = Reader::from_str(text);
        let mut depth = 0usize;
        let mut fields: Vec<String> = Vec::new();
        while let Ok(event) = reader.read_event() {
            match event {
                Event::Start(element) => {
                    depth += 1;
                    if depth == 2 {
                        fields.push(String::from_utf8_lossy(element.name().as_ref()).into_owned());
                    }
                }
                Event::Empty(element) if depth == 1 => {
                    fields.push(String::from_utf8_lossy(element.name().as_ref()).into_owned());
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Eof => break,
                _ => {}
            }
        }
        for field in fields {
            self.report.add(
                &format!("the metadata field `{field}`"),
                "in the container",
                "left out",
            );
        }
    }
}

/// The timelines of markers in a timeline, with the unit of their times.
fn collect_markers(
    timeline: &Timeline,
    unit: TimeUnit,
    found: &mut Vec<(model::Markers, TimeUnit)>,
) {
    let unit = timeline.info().time_unit.unwrap_or(unit);
    match timeline {
        Timeline::Markers(markers) => found.push((markers.clone(), unit)),
        Timeline::Lanes(lanes) => {
            for child in &lanes.children {
                collect_markers(child, unit, found);
            }
        }
        _ => {}
    }
}
