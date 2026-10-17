//! Parameter descriptors (WP-2.5; the device framework spec): what each of
//! a device's parameters is, so the engine can clamp and smooth it, the
//! generic UI can draw it, presets can load it, and automation (WP-2.12)
//! and agents can reach it.
//!
//! The descriptors themselves are the plug-in kit's (`lspk-params`), re-exported
//! here at their old paths, with the two rules from the improvements list
//! that every descriptor keeps ([`ParamDescriptor::check`]): G1, every gain
//! and level reaches −∞ dB; G2, every parameter can be automated and
//! modulated. What's Beatlayer's own is [`DeviceDescription`]: a kind of
//! device's descriptors with its layout.

use imbl::OrdMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use lspk_params::{
    DEFAULT_FLOOR_DB, MINUS_INFINITY_DB, ParamDescriptor, ParamKind, ParamUnit, Taper, db_to_gain,
    gain_to_db,
};

use crate::device_layout::DeviceLayout;
use crate::ids::ParamId;

/// What a kind of device is to the framework: its parameters' descriptors,
/// in the order its generic UI and agents list them, and its layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceDescription {
    /// Its parameters.
    pub params: Vec<ParamDescriptor>,
    /// Where its controls and display cells sit (IMP-14).
    pub layout: DeviceLayout,
    /// The views its display offers, such as `Transfer Curve`, the first
    /// being the one it starts in; empty for a display with one view. Which
    /// is shown is view state (`device.set_display_view`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
}

// Every value in a description is a finite number (`check`), so equality is
// an equivalence.
impl Eq for DeviceDescription {}

impl DeviceDescription {
    /// A device with `params`, laid out by the generic UI: columns of
    /// controls in the parameters' order ([`DeviceLayout::generic`]).
    #[must_use]
    pub fn new(params: Vec<ParamDescriptor>) -> Self {
        let layout = DeviceLayout::generic(&params);
        Self {
            params,
            layout,
            views: Vec::new(),
        }
    }

    /// The same, with a display that offers `views`, the first the one it
    /// starts in.
    #[must_use]
    pub fn with_views(mut self, views: &[&str]) -> Self {
        self.views = views.iter().map(|v| (*v).to_owned()).collect();
        self
    }

    /// The same, with its own layout.
    #[must_use]
    pub fn with_layout(mut self, layout: DeviceLayout) -> Self {
        self.layout = layout;
        self
    }

    /// The descriptor of `id`.
    #[must_use]
    pub fn param(&self, id: ParamId) -> Option<&ParamDescriptor> {
        self.params.iter().find(|p| p.id == id)
    }

    /// The parameter `id`'s value among `values` (a device's set values),
    /// or its default, inside its range.
    #[must_use]
    pub fn value(&self, values: &OrdMap<ParamId, f64>, id: ParamId) -> Option<f64> {
        let param = self.param(id)?;
        Some(param.clamp(values.get(&id).copied().unwrap_or(param.default)))
    }

    /// `values` as this device can take them: each inside its parameter's
    /// range, and any for a parameter it doesn't have left out (a preset
    /// from another version of the device, say).
    #[must_use]
    pub fn sanitize(&self, values: &OrdMap<ParamId, f64>) -> OrdMap<ParamId, f64> {
        values
            .iter()
            .filter_map(|(id, value)| Some((*id, self.param(*id)?.clamp(*value))))
            .collect()
    }

    /// Why the description breaks a rule, if it does: a parameter that does
    /// ([`ParamDescriptor::check`]), two with one number, or a layout that
    /// doesn't place every parameter exactly once ([`DeviceLayout::check`]).
    ///
    /// # Errors
    ///
    /// The first broken rule.
    pub fn check(&self) -> Result<(), String> {
        for (i, param) in self.params.iter().enumerate() {
            param.check()?;
            if self.params[..i].iter().any(|p| p.id == param.id) {
                return Err(format!("parameter {} is described twice", param.id.0));
            }
        }
        self.layout.check(&self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> ParamId {
        ParamId(n)
    }

    // Spec: device-framework R3
    #[test]
    fn a_description_with_two_parameters_of_one_number_is_refused() {
        let twice = DeviceDescription::new(vec![
            ParamDescriptor::toggle(id(0), "A", false),
            ParamDescriptor::toggle(id(0), "B", false),
        ]);
        assert!(twice.check().is_err());
        let mut gain = ParamDescriptor::gain(id(0), "Gain", 6.0, 0.0);
        gain.automatable = false;
        assert!(DeviceDescription::new(vec![gain]).check().is_err());
    }

    // Spec: device-framework R7
    #[test]
    fn sanitizing_clamps_and_drops_unknown_parameters() {
        let description = DeviceDescription::new(vec![
            ParamDescriptor::gain(id(0), "Gain", 6.0, 0.0),
            ParamDescriptor::choice(id(1), "Mode", &["A", "B"], 0),
        ]);
        let values: OrdMap<ParamId, f64> = [(id(0), 100.0), (id(1), 0.7), (id(9), 1.0)]
            .into_iter()
            .collect();
        let clean = description.sanitize(&values);
        assert_eq!(clean.len(), 2);
        assert!((clean[&id(0)] - db_to_gain(6.0)).abs() < 1e-12);
        assert_eq!(clean[&id(1)], 1.0);
        assert_eq!(description.value(&OrdMap::new(), id(0)), Some(1.0));
    }
}
