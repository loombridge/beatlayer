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

/// How many parameters a bank holds at most: a control surface's eight
/// knobs (control surfaces spec, R18).
pub const BANK_SIZE: usize = 8;

/// A named run of up to [`BANK_SIZE`] of a device's parameters, which a
/// control surface's device controls take in order (control surfaces spec,
/// R18, R19).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Bank {
    /// The bank's name.
    pub name: String,
    /// Its parameters, in the order the controls take them.
    pub params: Vec<ParamId>,
}

/// Two parameters that share one control, which a switch turns between:
/// while `switch` is on the control is `on`'s, and otherwise `off`'s. The
/// layout places `off`'s control only (Limiter's Maximize turns Ceiling into
/// Threshold).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Alternate {
    /// The switch.
    pub switch: ParamId,
    /// The parameter the control is while the switch is off.
    pub off: ParamId,
    /// The parameter the control is while the switch is on.
    pub on: ParamId,
}

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
    /// The parameter a vertical drag on the display changes, one for each of
    /// `views`, in their order (Roar's shaper display drags the shown
    /// stage's Shaper Amount); empty for a display that isn't dragged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_drag: Vec<ParamId>,
    /// The switches the device lists as its own items in its Show Options
    /// menu, after Save as Default Preset (C-DEVICE-015), in this order.
    /// They are parameters like any other, but the layout places no control
    /// for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub menu_options: Vec<ParamId>,
    /// The switch the device draws in its title bar instead of its panel
    /// (L-DEV-004): Use Current Scale, on the five scale-aware devices. It
    /// is a parameter like any other, which the layout places no control
    /// for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_switch: Option<ParamId>,
    /// The banks the type declares for control surfaces, its main controls
    /// first; empty for a type that leaves them to [`DeviceDescription::banks`]'
    /// generic ones. They hold neither the menu options nor the title
    /// switch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub banks: Vec<Bank>,
    /// The controls that show one of two parameters, as a switch says. Both
    /// parameters keep their values and their lanes whichever is shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<Alternate>,
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
            display_drag: Vec::new(),
            menu_options: Vec::new(),
            title_switch: None,
            banks: Vec::new(),
            alternates: Vec::new(),
        }
    }

    /// The same, with a display that offers `views`, the first the one it
    /// starts in.
    #[must_use]
    pub fn with_views(mut self, views: &[&str]) -> Self {
        self.views = views.iter().map(|v| (*v).to_owned()).collect();
        self
    }

    /// The same, with `params` as the parameters a drag on the display
    /// changes, one for each view.
    #[must_use]
    pub fn with_display_drag(mut self, params: &[ParamId]) -> Self {
        self.display_drag = params.to_vec();
        self
    }

    /// The same, with its own layout.
    #[must_use]
    pub fn with_layout(mut self, layout: DeviceLayout) -> Self {
        self.layout = layout;
        self
    }

    /// The same, with the switches `options` as the device's own Show
    /// Options items. A device with the generic layout gets it again
    /// without them.
    #[must_use]
    pub fn with_menu_options(mut self, options: &[ParamId]) -> Self {
        let generic = self.layout == DeviceLayout::generic(&self.placed());
        self.menu_options = options.to_vec();
        if generic {
            self.layout = DeviceLayout::generic(&self.placed());
        }
        self
    }

    /// The same, with the switch `id` drawn in the device's title bar. A
    /// device with the generic layout gets it again without the switch.
    #[must_use]
    pub fn with_title_switch(mut self, id: ParamId) -> Self {
        let generic = self.layout == DeviceLayout::generic(&self.placed());
        self.title_switch = Some(id);
        if generic {
            self.layout = DeviceLayout::generic(&self.placed());
        }
        self
    }

    /// The same, with `on`'s parameter taking `off`'s control while `switch`
    /// is on. A device with the generic layout gets it again without `on`.
    #[must_use]
    pub fn with_alternate(mut self, switch: ParamId, off: ParamId, on: ParamId) -> Self {
        let generic = self.layout == DeviceLayout::generic(&self.placed());
        self.alternates.push(Alternate { switch, off, on });
        if generic {
            self.layout = DeviceLayout::generic(&self.placed());
        }
        self
    }

    /// The parameter whose control sits where `id`'s does, given the device's
    /// set `values`: the alternate while its switch is on, else `id`.
    #[must_use]
    pub fn drawn(&self, values: &OrdMap<ParamId, f64>, id: ParamId) -> ParamId {
        self.alternates
            .iter()
            .find(|a| a.off == id && self.value(values, a.switch).is_some_and(|v| v >= 0.5))
            .map_or(id, |a| a.on)
    }

    /// Whether `id`'s control is shown, given the device's set `values`: false
    /// for the half of an alternate its switch has put away.
    #[must_use]
    pub fn shown(&self, values: &OrdMap<ParamId, f64>, id: ParamId) -> bool {
        self.alternates
            .iter()
            .filter(|a| a.off == id || a.on == id)
            .all(|a| self.drawn(values, a.off) == id)
    }

    /// The same, with the banks `banks` for control surfaces: each a name
    /// and the names of up to eight parameters, in the order the controls
    /// take them; a device whose names repeat gives a parameter as `#` and
    /// its number. Descriptions are static data, so a name the device
    /// doesn't have is a bug, not an error.
    ///
    /// # Panics
    ///
    /// On a parameter name the device doesn't have, or has twice.
    #[must_use]
    pub fn with_banks(mut self, banks: &[(&str, &[&str])]) -> Self {
        self.banks = banks
            .iter()
            .map(|(name, params)| Bank {
                name: (*name).to_owned(),
                params: params
                    .iter()
                    .map(|param| {
                        if let Some(number) = param.strip_prefix('#') {
                            let id = ParamId(number.parse().expect("a number after the #"));
                            assert!(
                                self.param(id).is_some(),
                                "a bank names no parameter {param}"
                            );
                            return id;
                        }
                        let mut named = self.params.iter().filter(|p| p.name == *param);
                        let found = named
                            .next()
                            .unwrap_or_else(|| panic!("a bank names no parameter {param:?}"));
                        assert!(named.next().is_none(), "two parameters are named {param:?}");
                        found.id
                    })
                    .collect(),
            })
            .collect();
        self
    }

    /// The banks a control surface shows (control surfaces spec, R19): the
    /// declared ones, or without any the parameters in order, eight at a
    /// time, named "1–8", "9–16" and so on; then, in a bank of their own,
    /// the title bar's switch and the menu options.
    #[must_use]
    pub fn banks(&self) -> Vec<Bank> {
        let options: Vec<ParamId> = self
            .title_switch
            .into_iter()
            .chain(self.menu_options.iter().copied())
            .collect();
        let mut banks = if self.banks.is_empty() {
            let rest: Vec<ParamId> = self
                .params
                .iter()
                .map(|p| p.id)
                .filter(|id| !options.contains(id))
                .collect();
            numbered(&rest)
        } else {
            self.banks.clone()
        };
        for (i, chunk) in options.chunks(BANK_SIZE).enumerate() {
            banks.push(Bank {
                name: if i == 0 {
                    "Options".to_owned()
                } else {
                    format!("Options {}", i + 1)
                },
                params: chunk.to_vec(),
            });
        }
        banks
    }

    /// The parameters the layout places a control for: all but the menu
    /// options and the title bar's switch.
    fn placed(&self) -> Vec<ParamDescriptor> {
        self.params
            .iter()
            .filter(|p| {
                !self.menu_options.contains(&p.id)
                    && self.title_switch != Some(p.id)
                    && !self.alternates.iter().any(|a| a.on == p.id)
            })
            .cloned()
            .collect()
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
    /// ([`ParamDescriptor::check`]), two with one number, a menu option that
    /// isn't a switch of the device (or is listed twice), or a layout that
    /// doesn't place every other parameter exactly once
    /// ([`DeviceLayout::check`]).
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
        for (i, option) in self.menu_options.iter().enumerate() {
            let is_switch = self
                .param(*option)
                .is_some_and(|p| matches!(p.kind, ParamKind::Toggle));
            if !is_switch || self.menu_options[..i].contains(option) {
                return Err(format!(
                    "menu option {} isn't a switch of the device, or is listed twice",
                    option.0
                ));
            }
        }
        if let Some(id) = self.title_switch
            && (!self
                .param(id)
                .is_some_and(|p| matches!(p.kind, ParamKind::Toggle))
                || self.menu_options.contains(&id))
        {
            return Err(format!(
                "title switch {} isn't a switch of the device, or is also a menu option",
                id.0
            ));
        }
        for a in &self.alternates {
            let toggle = self
                .param(a.switch)
                .is_some_and(|p| matches!(p.kind, ParamKind::Toggle));
            if !toggle
                || a.off == a.on
                || self.param(a.off).is_none()
                || self.param(a.on).is_none()
                || self
                    .alternates
                    .iter()
                    .filter(|b| b.on == a.on || b.off == a.off)
                    .count()
                    > 1
            {
                return Err(format!(
                    "alternate {} and {} isn't a pair of parameters of the device, or its \
                     switch isn't a switch of it",
                    a.off.0, a.on.0
                ));
            }
        }
        let options =
            |id: &ParamId| self.menu_options.contains(id) || self.title_switch == Some(*id);
        for bank in &self.banks {
            if bank.params.len() > BANK_SIZE
                || bank
                    .params
                    .iter()
                    .any(|id| self.param(*id).is_none() || options(id))
            {
                return Err(format!(
                    "bank {:?} holds more than {BANK_SIZE} parameters, or one the device \
                     doesn't have, or an option",
                    bank.name
                ));
            }
        }
        if !self.banks.is_empty() {
            let banked = |id: &ParamId| self.banks.iter().any(|b| b.params.contains(id));
            if let Some(p) = self
                .params
                .iter()
                .find(|p| !options(&p.id) && !banked(&p.id))
            {
                return Err(format!("parameter {} is in no bank", p.id.0));
            }
        }
        self.layout.check(&self.placed())
    }
}

/// `params` in banks of [`BANK_SIZE`], named by their positions: "1–8",
/// "9–16" (control surfaces spec, R19, R21).
#[must_use]
pub fn numbered(params: &[ParamId]) -> Vec<Bank> {
    params
        .chunks(BANK_SIZE)
        .enumerate()
        .map(|(i, chunk)| Bank {
            name: format!("{}–{}", i * BANK_SIZE + 1, i * BANK_SIZE + chunk.len()),
            params: chunk.to_vec(),
        })
        .collect()
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

    // Spec: device-chains R35
    #[test]
    fn a_menu_option_is_a_switch_the_layout_places_no_control_for() {
        let params = vec![
            ParamDescriptor::gain(id(0), "Gain", 6.0, 0.0),
            ParamDescriptor::toggle(id(1), "Mono", true),
        ];
        let described = DeviceDescription::new(params.clone()).with_menu_options(&[id(1)]);
        described.check().unwrap();
        assert_eq!(
            described.layout,
            DeviceLayout::generic(&params[..1]),
            "the generic layout leaves it out"
        );
        // A menu option that isn't a switch, or isn't there, is refused.
        assert!(
            DeviceDescription::new(params.clone())
                .with_menu_options(&[id(0)])
                .check()
                .is_err()
        );
        assert!(
            DeviceDescription::new(params.clone())
                .with_menu_options(&[id(7)])
                .check()
                .is_err()
        );
        // So is a layout that places it as well.
        let mut placed = DeviceDescription::new(params);
        placed.menu_options = vec![id(1)];
        assert!(placed.check().is_err(), "a control for a menu option");
    }

    // Spec: scales R16
    #[test]
    fn a_title_switch_is_a_switch_the_layout_places_no_control_for() {
        let params = vec![
            ParamDescriptor::gain(id(0), "Gain", 6.0, 0.0),
            ParamDescriptor::toggle(id(1), "Use Current Scale", false),
        ];
        let described = DeviceDescription::new(params.clone()).with_title_switch(id(1));
        described.check().unwrap();
        assert_eq!(described.layout, DeviceLayout::generic(&params[..1]));
        // One that isn't a switch, or isn't there, or is a menu option too,
        // is refused.
        for bad in [
            DeviceDescription::new(params.clone()).with_title_switch(id(0)),
            DeviceDescription::new(params.clone()).with_title_switch(id(7)),
            DeviceDescription::new(params.clone())
                .with_menu_options(&[id(1)])
                .with_title_switch(id(1)),
        ] {
            assert!(bad.check().is_err());
        }
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

    // Spec: device-framework R50
    #[test]
    fn an_alternate_pairs_two_parameters_through_a_switch_and_bad_pairs_are_refused() {
        let params = || {
            vec![
                ParamDescriptor::toggle(id(0), "Switch", false),
                ParamDescriptor::toggle(id(1), "Off", false),
                ParamDescriptor::toggle(id(2), "On", false),
                ParamDescriptor::toggle(id(3), "Other", false),
            ]
        };
        let paired = DeviceDescription::new(params()).with_alternate(id(0), id(1), id(2));
        paired.check().unwrap();
        let mut values = OrdMap::new();
        assert!(paired.shown(&values, id(1)) && !paired.shown(&values, id(2)));
        assert!(
            paired.shown(&values, id(3)),
            "an unpaired parameter is always shown"
        );
        values.insert(id(0), 1.0);
        assert_eq!(paired.drawn(&values, id(1)), id(2));
        assert!(!paired.shown(&values, id(1)) && paired.shown(&values, id(2)));
        // The second of a pair has no control of its own in the layout.
        let placed = paired.layout.place(paired.layout.docked_size());
        assert!(placed.controls.iter().all(|c| c.param != id(2)));
        assert!(placed.controls.iter().any(|c| c.param == id(1)));
        // A pair of one parameter, one that uses a parameter twice, or one
        // whose switch isn't a switch is refused.
        let bad = |switch, off, on| {
            DeviceDescription::new(params())
                .with_alternate(id(0), id(1), id(2))
                .with_alternate(switch, off, on)
                .check()
        };
        assert!(bad(id(0), id(3), id(3)).is_err());
        assert!(bad(id(0), id(3), id(2)).is_err());
        let mut continuous = params();
        continuous[0] =
            ParamDescriptor::continuous(id(0), "Switch", (0.0, 1.0), 0.0, ParamUnit::None);
        assert!(
            DeviceDescription::new(continuous)
                .with_alternate(id(0), id(1), id(2))
                .check()
                .is_err()
        );
    }
}
