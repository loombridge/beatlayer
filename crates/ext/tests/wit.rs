//! The WIT worlds under `wit/` and the host agree (extensions R1, R2).

use bl_ext::{Kind, WORLD};
use wit_parser::{Resolve, WorldItem};

#[test]
fn the_worlds_are_the_hosts_kinds_at_the_hosts_version() {
    let mut resolve = Resolve::default();
    let (package, _) = resolve
        .push_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../wit"))
        .unwrap();
    let package = &resolve.packages[package];
    assert_eq!(package.name.to_string(), format!("beatlayer:ext@{WORLD}"));

    for kind in Kind::ALL {
        let world = package
            .worlds
            .get(&format!("{}-world", kind.name()))
            .unwrap_or_else(|| panic!("no world for {}", kind.name()));
        let exports: Vec<&str> = resolve.worlds[*world]
            .exports
            .iter()
            .map(|(_, item)| match item {
                WorldItem::Interface { id, .. } => resolve.interfaces[*id].name.as_deref().unwrap(),
                other => panic!("unexpected export {other:?}"),
            })
            .collect();
        assert_eq!(exports, [kind.name()]);
    }
    assert!(package.interfaces.contains_key("set"), "R38's import");
}
