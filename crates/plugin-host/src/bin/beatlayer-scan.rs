//! `beatlayer-scan`: the plug-in scanner's child process (WP-2.9).
//!
//! It loads one plug-in file and prints what it found as JSON on its
//! standard output, so the scanner can run it with a timeout and know a
//! crashing or hanging plug-in only killed the scan. It's started as
//! `beatlayer-scan <format> <path>`; CLAP (`clap`) is the only format so far.

use std::io::Write;
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
    let mut out = reply_pipe();
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
    // printed without one, where it shares the standard output.
    let _ = writeln!(
        out,
        "\n{}",
        serde_json::to_string(&reply).unwrap_or_default()
    );
    let _ = out.flush();
    if !reply.ok {
        std::process::exit(1);
    }
}

/// Where the reply is written. On Unix this is a private close-on-exec copy of
/// the scanner's standard output, and fd 1 is pointed at `/dev/null` before
/// any plug-in loads, so a helper process the plug-in starts can't inherit the
/// pipe and hold it open after this process exits (the plug-in scanning spec,
/// R4). Elsewhere, and if that fails, it's the standard output itself.
fn reply_pipe() -> Box<dyn Write> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        if let Ok(null) = std::fs::File::options().write(true).open("/dev/null") {
            // SAFETY: fd 1 is open; `copy` is a new descriptor that nothing
            // else owns, so the `File` may take it.
            unsafe {
                let copy = libc::fcntl(1, libc::F_DUPFD_CLOEXEC, 3);
                if copy >= 0 {
                    if libc::dup2(null.as_raw_fd(), 1) >= 0 {
                        return Box::new(std::fs::File::from_raw_fd(copy));
                    }
                    libc::close(copy);
                }
            }
        }
    }
    Box::new(std::io::stdout())
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
