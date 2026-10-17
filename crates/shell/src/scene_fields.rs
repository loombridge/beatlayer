//! A scene's tempo and time signature fields, in the Main column and the
//! Scene View (WP-3.5; layout.md L-SES-007, L-SES-011): what each shows, and
//! the command each change makes. The fields take their values as the
//! Control Bar's do — a typed tempo comes within 20 to 999 BPM and to
//! hundredths, a typed numerator within 1 to 99, a typed denominator is the
//! largest of 1, 2, 4, 8, and 16 that doesn't pass it, and text that isn't a
//! number leaves the field as it was. A field is grey, showing the set's
//! current value, until the scene is given its own; a double-click or Delete
//! turns it off again (I-CLICK-010, C-SCENE-016).

use bl_core::time::{Bpm, TimeSignature};
use bl_model::SceneId;
use serde_json::json;

use crate::Invocation;
use crate::Shell;
use crate::transport::{DENOMINATORS, TEMPO_MAX, TEMPO_MIN};

/// One of a scene's fields. The time signature is two, as the Control Bar's
/// is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneField {
    /// The scene's tempo.
    Tempo,
    /// The time signature's numerator.
    Numerator,
    /// The time signature's denominator.
    Denominator,
}

/// What a field shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldView {
    /// The text in the field.
    pub text: String,
    /// Whether the scene has its own value; the field is grey, showing the
    /// set's current one, until it has.
    pub own: bool,
}

/// What `field` of `scene` shows.
#[must_use]
pub fn view(shell: &Shell, scene: SceneId, field: SceneField) -> FieldView {
    let own = shell.editor().document().scene(scene);
    match field {
        SceneField::Tempo => {
            let bpm = own.and_then(|s| s.tempo);
            FieldView {
                text: format!("{:.2}", bpm.unwrap_or_else(|| shell.current_tempo()).get()),
                own: bpm.is_some(),
            }
        }
        SceneField::Numerator | SceneField::Denominator => {
            let signature = own.and_then(|s| s.signature);
            let shown = signature.unwrap_or_else(|| shell.current_signature());
            FieldView {
                text: match field {
                    SceneField::Numerator => shown.numerator(),
                    _ => shown.denominator(),
                }
                .to_string(),
                own: signature.is_some(),
            }
        }
    }
}

/// `scene.set_tempo` with `bpm` brought within the field's range and to
/// hundredths, if it changes the scene.
fn set_tempo(shell: &Shell, scene: SceneId, bpm: f64) -> Option<Invocation> {
    if !bpm.is_finite() {
        return None;
    }
    let bpm = ((bpm.clamp(TEMPO_MIN, TEMPO_MAX)) * 100.0).round() / 100.0;
    let bpm = Bpm::new(bpm).ok()?;
    let current = shell.editor().document().scene(scene)?.tempo;
    (current != Some(bpm))
        .then(|| Invocation::new("scene.set_tempo", json!({ "scene": scene, "tempo": bpm })))
}

/// `scene.set_time_signature` with a part changed, the other as shown.
fn set_signature(
    shell: &Shell,
    scene: SceneId,
    field: SceneField,
    value: i64,
) -> Option<Invocation> {
    let shown = shell.current_signature();
    let own = shell.editor().document().scene(scene)?.signature;
    let base = own.unwrap_or(shown);
    let signature = match field {
        SceneField::Numerator => {
            TimeSignature::new(value.clamp(1, 99) as u8, base.denominator()).ok()?
        }
        _ => {
            let denominator = DENOMINATORS
                .iter()
                .rev()
                .copied()
                .find(|d| i64::from(*d) <= value)
                .unwrap_or(1);
            TimeSignature::new(base.numerator(), denominator).ok()?
        }
    };
    (own != Some(signature)).then(|| {
        Invocation::new(
            "scene.set_time_signature",
            json!({ "scene": scene, "signature": signature }),
        )
    })
}

/// The change typing `text` into a field makes, if it makes one.
#[must_use]
pub fn typed(shell: &Shell, scene: SceneId, field: SceneField, text: &str) -> Option<Invocation> {
    let text = text.trim();
    match field {
        SceneField::Tempo => set_tempo(shell, scene, text.replace(',', ".").parse().ok()?),
        _ => set_signature(shell, scene, field, text.parse().ok()?),
    }
}

/// The change a step up or down makes: a whole BPM for the tempo (a
/// hundredth when `fine`), one for the numerator, the next note value for the
/// denominator.
#[must_use]
pub fn step(
    shell: &Shell,
    scene: SceneId,
    field: SceneField,
    steps: i32,
    fine: bool,
) -> Option<Invocation> {
    let shown = view(shell, scene, field);
    let now: f64 = shown.text.parse().ok()?;
    match field {
        SceneField::Tempo => {
            let by = if fine { 0.01 } else { 1.0 };
            set_tempo(shell, scene, now + by * f64::from(steps))
        }
        SceneField::Numerator => set_signature(shell, scene, field, now as i64 + i64::from(steps)),
        SceneField::Denominator => {
            let index = DENOMINATORS
                .iter()
                .position(|d| f64::from(*d) == now)
                .unwrap_or(2) as i64;
            let next = (index + i64::from(steps)).clamp(0, DENOMINATORS.len() as i64 - 1);
            set_signature(shell, scene, field, i64::from(DENOMINATORS[next as usize]))
        }
    }
}

/// Switches the field's value off, as a double-click, Delete, and Return to
/// Default do: the tempo for the tempo field, the whole signature for either
/// part of it. `None` if the scene has no value of its own.
#[must_use]
pub fn off(shell: &Shell, scene: SceneId, field: SceneField) -> Option<Invocation> {
    let own = shell.editor().document().scene(scene)?;
    match field {
        SceneField::Tempo => own
            .tempo
            .is_some()
            .then(|| Invocation::new("scene.set_tempo", json!({ "scene": scene, "tempo": null }))),
        _ => own.signature.is_some().then(|| {
            Invocation::new(
                "scene.set_time_signature",
                json!({ "scene": scene, "signature": null }),
            )
        }),
    }
}
