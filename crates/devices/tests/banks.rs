//! Every native device type's banks for control surfaces (control surfaces
//! spec, R18, R19).

use bl_model::{BANK_SIZE, DeviceKind, ParamId};

/// The types whose parameters run to the hundreds: they leave their banks to
/// the generic ones, eight parameters at a time in descriptor order.
const GENERIC: &[&str] = &["beatlayer.mosaic", "beatlayer.kitbag"];

// Spec: control-surfaces R19
#[test]
fn every_native_device_types_banks_hold_every_parameter_and_none_more_than_eight() {
    let types = bl_devices::device_types();
    assert!(types.len() > 50, "the registry lists the native types");
    for device_type in types {
        let name = match &device_type.kind {
            DeviceKind::Native { device_type } => device_type.clone(),
            other => panic!("{other:?} isn't a native device"),
        };
        let description = &device_type.description;
        description
            .check()
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let banks = description.banks();
        assert!(!banks.is_empty(), "{name} has no banks");
        for bank in &banks {
            assert!(
                (1..=BANK_SIZE).contains(&bank.params.len()),
                "{name}'s bank {:?} holds {} parameters",
                bank.name,
                bank.params.len()
            );
        }
        let banked: Vec<ParamId> = banks
            .iter()
            .flat_map(|b| b.params.iter().copied())
            .collect();
        for param in &description.params {
            assert!(
                banked.contains(&param.id),
                "{name}'s {:?} is in no bank",
                param.name
            );
        }
        assert_eq!(
            description.banks.is_empty(),
            GENERIC.contains(&name.as_str()),
            "{name} declares banks unless it is a generic one"
        );
        // The title bar's switch and the menu options come last, in banks of
        // their own.
        let options: Vec<ParamId> = description
            .title_switch
            .into_iter()
            .chain(description.menu_options.iter().copied())
            .collect();
        if !options.is_empty() {
            let last = banks.last().unwrap();
            assert_eq!(
                last.params,
                options[options.len().saturating_sub(last.params.len())..]
            );
            assert!(
                banks[..banks.len() - 1]
                    .iter()
                    .all(|b| b.params.iter().all(|p| !options.contains(p))),
                "{name}: an option is in a bank before the last"
            );
        }
    }
}

// Spec: control-surfaces R19
#[test]
fn a_description_without_banks_has_the_generic_ones_named_by_position() {
    use bl_model::{DeviceDescription, ParamDescriptor};
    let params: Vec<ParamDescriptor> = (0..11)
        .map(|n| ParamDescriptor::toggle(ParamId(n), &format!("P{n}"), false))
        .collect();
    let banks = DeviceDescription::new(params)
        .with_menu_options(&[ParamId(10)])
        .banks();
    let names: Vec<_> = banks
        .iter()
        .map(|b| (b.name.as_str(), b.params.len()))
        .collect();
    assert_eq!(names, [("1–8", 8), ("9–10", 2), ("Options", 1)]);
}
