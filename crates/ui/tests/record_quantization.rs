//! Edit ▸ Record Quantization in the UI (WP-7.32): the submenu's nine items
//! are one radio group whose ticked item is the set's value. The behaviour is
//! in the recording spec, R17 and R21.

use crate::common::{Harness, Window};
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::keymap::Platform;
use bl_ui::menus::{ItemState, item_state};
use serde_json::json;

const ROWS: [(&str, &str, &str); 9] = [
    ("M-EDIT-038a", "No Quantization", "none"),
    ("M-EDIT-038b", "Quarter-Note Quantization", "1/4"),
    ("M-EDIT-038c", "Eighth-Note Quantization", "1/8"),
    ("M-EDIT-038d", "Eighth-Note Triplets Quantization", "1/8t"),
    (
        "M-EDIT-038e",
        "Eighth-Note and Triplets Quantization",
        "1/8+1/8t",
    ),
    ("M-EDIT-038f", "Sixteenth-Note Quantization", "1/16"),
    (
        "M-EDIT-038g",
        "Sixteenth-Note Triplets Quantization",
        "1/16t",
    ),
    (
        "M-EDIT-038h",
        "Sixteenth-Note and Triplets Quantization",
        "1/16+1/16t",
    ),
    ("M-EDIT-038i", "Thirty-Second Note Quantization", "1/32"),
];

fn menu_item(h: &Harness, row: &str) -> ItemState {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: recording R17. Each item is enabled, labelled as the reference says
// and runs the command with its own value; choosing it ticks it alone.
#[test]
fn each_item_of_the_submenu_chooses_its_value_and_is_the_only_one_ticked() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let ticked = |h: &Harness| -> Vec<&'static str> {
        ROWS.iter()
            .filter(|(row, ..)| menu_item(h, row).checked == Some(true))
            .map(|(.., value)| *value)
            .collect()
    };
    assert_eq!(ticked(&h), ["none"], "a new set has No Quantization");
    for (row, label, value) in ROWS.iter().rev().chain(ROWS.iter()) {
        let item = menu_item(&h, row);
        assert_eq!(item.label, *label);
        assert!(item.enabled, "{row}");
        let invocation = item.invocation.unwrap();
        assert_eq!(invocation.id, "transport.set_record_quantization");
        h.shell.invoke(&invocation.id, invocation.args).unwrap();
        h.settle();
        assert_eq!(ticked(&h), [*value], "{row}");
        let state = h.shell.run("transport.state", json!({})).unwrap();
        assert_eq!(state["record_quantization"], json!(value));
    }
}
