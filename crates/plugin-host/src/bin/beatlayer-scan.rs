//! `beatlayer-scan`: the plug-in scanner's child process (WP-2.9).
//!
//! It loads one plug-in file and prints what it found as JSON on its
//! standard output, so the scanner can run it with a timeout and know a
//! crashing or hanging plug-in only killed the scan. It's started as
//! `beatlayer-scan <format> <path>`; CLAP (`clap`) is the only format so far.

use std::path::PathBuf;

use bl_model::PluginFormat;
use bl_plugin_host::scan::{ChildReply, format_name};

fn main() {
    let mut args = std::env::args().skip(1);
    let format_arg = args.next().unwrap_or_default();
    let path = args.next().map_or_else(PathBuf::new, PathBuf::from);
    let Some(format) = parse_format(&format_arg) else {
        let reply = ChildReply {
            ok: false,
            plugins: Vec::new(),
            error: Some(format!("{format_arg} isn't a format Beatlayer scans")),
        };
        println!("{}", serde_json::to_string(&reply).unwrap_or_default());
        std::process::exit(2);
    };
    let reply = match ChildReply::describe(format, &path) {
        Ok(plugins) => ChildReply {
            ok: true,
            plugins,
            error: None,
        },
        Err(error) => ChildReply {
            ok: false,
            plugins: Vec::new(),
            error: Some(error),
        },
    };
    // Starting a new line keeps the reply off the end of any banner the plug-in
    // printed without one.
    println!("\n{}", serde_json::to_string(&reply).unwrap_or_default());
    if !reply.ok {
        std::process::exit(1);
    }
}

fn parse_format(name: &str) -> Option<PluginFormat> {
    [
        PluginFormat::Clap,
        PluginFormat::Vst3,
        PluginFormat::AudioUnit,
        PluginFormat::Lv2,
    ]
    .into_iter()
    .find(|format| format_name(*format) == name)
}
