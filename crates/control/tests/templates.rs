//! Controller templates (`docs/specs/control-surfaces.md`, R39, R40, R41,
//! R43, R44, R46 and R48): the format, its checks with their lines, the
//! bundled template and a User Library folder read from disk.

use std::path::PathBuf;

use bl_control::template::{self, Act, BUNDLED, KEYS, ROLES, Reads, Source, parse};
use bl_control::{RingStep, Role};

/// A template of a ring four by two, with `controls` after its surface table.
fn file(controls: &str) -> String {
    format!("format = 1\n\n[surface]\nname = \"T\"\nring_width = 4\nring_height = 2\n\n{controls}")
}

fn control(lines: &str) -> String {
    format!("[[control]]\nid = \"c\"\nchannel = 1\n{lines}\n")
}

fn error_of(text: &str) -> (usize, String) {
    let errors = parse(text).expect_err("the file is refused");
    (errors[0].line, errors[0].message.clone())
}

/// The words in the comment lines of `text`.
fn comment_words(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.trim_start().starts_with('#'))
        .flat_map(|line| {
            line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

// Spec: control-surfaces R46. The bundled template parses, and its comments
// explain every key and every role.
#[test]
fn the_bundled_template_parses_and_every_key_and_role_is_commented() {
    let bundled = parse(BUNDLED).expect("the bundled template is whole");
    assert_eq!(bundled.name, "Generic 8 + 8");
    assert_eq!((bundled.ring_width, bundled.ring_height), (8, 2));
    let words = comment_words(BUNDLED);
    for key in KEYS {
        assert!(words.iter().any(|w| w == key), "no comment names `{key}`");
    }
    for role in ROLES {
        assert!(words.iter().any(|w| w == role), "no comment names `{role}`");
    }
    let slots = bundled
        .controls
        .iter()
        .filter(|c| matches!(c.act, Act::Role(Role::Slot { .. })))
        .count();
    assert_eq!(slots, 16, "sixteen pads are a two-row ring of eight");
    let knobs = bundled
        .controls
        .iter()
        .filter(|c| matches!(c.act, Act::Role(Role::Device(_))))
        .count();
    let faders = bundled
        .controls
        .iter()
        .filter(|c| matches!(c.act, Act::Role(Role::Volume(_))))
        .count();
    assert_eq!((knobs, faders), (8, 8));
}

// Spec: control-surfaces R39, R40. A control's message, kind and role are
// read as the file says.
#[test]
fn a_control_reads_its_message_kind_and_role() {
    let text = file(&control(
        "message = \"cc\"\nnumber = 20\nkind = \"relative\"\nencoding = \"bin-offset\"\naccelerated = true\nrole = \"ring-left\"",
    ));
    let template = parse(&text).unwrap();
    let c = &template.controls[0];
    assert_eq!(
        c.source,
        Source::Cc {
            channel: 0,
            controller: 20
        }
    );
    assert!(matches!(
        c.kind,
        Reads::Relative {
            accelerated: true,
            ..
        }
    ));
    assert_eq!(c.act, Act::Role(Role::Ring(RingStep::Left)));
    // Numbers after a role's word count from 1.
    let text = file(&control(
        "message = \"note\"\nnumber = 36\nkind = \"button\"\nrole = \"slot 3 2\"",
    ));
    assert_eq!(
        parse(&text).unwrap().controls[0].act,
        Act::Role(Role::Slot { column: 2, row: 1 })
    );
}

// Spec: control-surfaces R39, R44. A file of a later format is refused,
// naming the format, on the line of its `format`.
#[test]
fn format_two_is_refused_with_its_line() {
    let (line, message) = error_of("# a comment\nformat = 2\n[surface]\nname = \"T\"\n");
    assert_eq!(line, 2);
    assert!(message.contains("format 2"), "{message}");
}

// Spec: control-surfaces R44. An unknown role is refused with its line.
#[test]
fn an_unknown_role_is_refused_with_its_line() {
    let text = file(&control(
        "message = \"note\"\nnumber = 1\nkind = \"button\"\nrole = \"fly\"",
    ));
    let (line, message) = error_of(&text);
    assert_eq!(line, 14, "the line of the role");
    assert!(message.contains("fly"), "{message}");
}

// Spec: control-surfaces R44. Two controls on one message are refused, on
// the second's line; a 14-bit pair takes both its controllers.
#[test]
fn two_controls_on_one_message_are_refused_with_the_second_ones_line() {
    let one = control("message = \"cc\"\nnumber = 5\nkind = \"absolute\"\nrole = \"volume 1\"");
    let two = control("message = \"cc\"\nnumber = 5\nkind = \"absolute\"\nrole = \"pan 1\"");
    let text = file(&format!("{one}\n{two}"));
    let (line, message) = error_of(&text);
    assert_eq!(line, 16, "the second [[control]]");
    assert!(message.contains("already"), "{message}");
    // A 14-bit controller 5 also takes controller 37.
    let wide = control("message = \"cc14\"\nnumber = 5\nkind = \"absolute\"\nrole = \"volume 1\"");
    let low = control("message = \"cc\"\nnumber = 37\nkind = \"absolute\"\nrole = \"pan 1\"");
    assert!(parse(&file(&format!("{wide}\n{low}"))).is_err());
}

// Spec: control-surfaces R44. A slot outside the ring, and a column past its
// width, are refused with their line.
#[test]
fn a_slot_outside_its_ring_is_refused_with_its_line() {
    for role in ["slot 5 1", "slot 1 3", "volume 5", "scene 3"] {
        let text = file(&control(&format!(
            "message = \"note\"\nnumber = 1\nkind = \"button\"\nrole = \"{role}\""
        )));
        let (line, message) = error_of(&text);
        assert_eq!(line, 14, "{role}");
        assert!(message.contains("outside the ring"), "{role}: {message}");
    }
}

// Spec: control-surfaces R41, R44. A setup is whole system-exclusive or
// channel messages, as hexadecimal bytes.
#[test]
fn a_setup_must_be_whole_messages() {
    let ok = "format = 1\n[surface]\nname = \"T\"\n[[setup]]\nbytes = \"F0 7D 01 F7 90 3C 7F\"\n";
    let template = parse(ok).unwrap();
    assert_eq!(
        template.setup,
        [vec![0xF0, 0x7D, 0x01, 0xF7], vec![0x90, 0x3C, 0x7F]]
    );
    for bad in ["F0 7D 01", "90 3C", "3C 7F", "F0 80 F7", "zz"] {
        let text = format!("format = 1\n[surface]\nname = \"T\"\n[[goodbye]]\nbytes = \"{bad}\"\n");
        let (line, _) = error_of(&text);
        assert_eq!(line, 5, "{bad}");
    }
}

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-control-templates-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn named(name: &str) -> String {
    format!("format = 1\n[surface]\nname = \"{name}\"\n")
}

// Spec: control-surfaces R43, R44. A real folder with two templates and a
// subfolder lists all three after the bundled one; one named like the
// bundled one replaces it; a broken file is reported and not listed.
#[test]
fn a_folder_lists_its_templates_and_a_same_named_one_replaces_the_bundled() {
    let folder = Scratch::new("folder");
    folder.write("b.bltemplate.toml", &named("Beta"));
    folder.write("a.bltemplate.toml", &named("Alpha"));
    folder.write("Vendor/c.bltemplate.toml", &named("Gamma"));
    folder.write("notes.txt", "not a template");
    let found = template::load(Some(&folder.0));
    let names: Vec<String> = template::kinds(&found)
        .iter()
        .map(|k| k.name.clone())
        .collect();
    assert_eq!(names, ["Alpha", "Beta", "Gamma", "Generic 8 + 8"]);

    folder.write(
        "Vendor/generic.bltemplate.toml",
        "format = 1\n[surface]\nname = \"Generic 8 + 8\"\nring_width = 3\n",
    );
    folder.write("broken.bltemplate.toml", "format = 1\n[surface]\n");
    let found = template::load(Some(&folder.0));
    let kinds = template::kinds(&found);
    let generic: Vec<_> = kinds.iter().filter(|k| k.name == "Generic 8 + 8").collect();
    assert_eq!(generic.len(), 1, "named once");
    assert_eq!(
        generic[0].ring.map(|r| r.tracks),
        Some(3),
        "the User Library's"
    );
    let broken = found
        .iter()
        .find(|f| {
            f.path
                .as_ref()
                .is_some_and(|p| p.ends_with("broken.bltemplate.toml"))
        })
        .unwrap();
    assert!(!broken.listed && !broken.errors.is_empty());
    let bundled = found.iter().find(|f| f.path.is_none()).unwrap();
    assert!(!bundled.listed, "replaced");
}
