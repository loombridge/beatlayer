//! `bl-cli devices`, and finding a device from what a person typed.

use std::process::ExitCode;

use bl_audio_io::{Backend, BackendKind, DeviceId, DeviceInfo};

use crate::Outcome;

#[derive(Debug, clap::Args)]
pub(crate) struct DevicesArgs {
    /// List only this backend: coreaudio, wasapi, asio, pipewire, jack, alsa,
    /// or null.
    #[arg(long)]
    backend: Option<String>,
}

pub(crate) fn run(args: &DevicesArgs) -> Outcome {
    let kinds = match &args.backend {
        Some(name) => vec![parse_backend(name)?],
        None => bl_audio_io::available_backends(),
    };
    let default = bl_audio_io::default_backend();
    for kind in kinds {
        let note = if kind == default { ", the default" } else { "" };
        println!("{} ({}{note})", kind.label(), kind.id());
        let devices = bl_audio_io::open_backend(kind).and_then(|backend| backend.devices());
        match devices {
            Ok(devices) if devices.is_empty() => println!("  No devices."),
            Ok(devices) => {
                for (index, device) in devices.iter().enumerate() {
                    print_device(index, device);
                }
            }
            Err(error) => println!("  Unavailable: {error}"),
        }
        println!();
    }
    Ok(ExitCode::SUCCESS)
}

fn print_device(index: usize, device: &DeviceInfo) {
    let mut roles = Vec::new();
    if device.is_default_output {
        roles.push("default output");
    }
    if device.is_default_input {
        roles.push("default input");
    }
    let roles = if roles.is_empty() {
        String::new()
    } else {
        format!(" ({})", roles.join(", "))
    };
    println!("  [{index}] {}{roles}", device.name);
    println!("      id:           {}", device.id);
    println!(
        "      channels:     {} out, {} in",
        device.output_channels, device.input_channels
    );
    println!("      sample rates: {}", sample_rates(device));
    let default_size = device
        .default_buffer_size
        .map(|size| format!(", default {size}"))
        .unwrap_or_default();
    println!("      buffer sizes: {}{default_size}", device.buffer_sizes);
}

fn sample_rates(device: &DeviceInfo) -> String {
    if device.sample_rates.is_empty() {
        return "unknown".into();
    }
    let rates: Vec<String> = device.sample_rates.iter().map(u32::to_string).collect();
    let current = device
        .current_sample_rate
        .map(|rate| format!(" (running at {rate})"))
        .unwrap_or_default();
    format!("{} Hz{current}", rates.join(", "))
}

/// Looks a backend up by its command-line name.
pub(crate) fn parse_backend(name: &str) -> Result<BackendKind, String> {
    BackendKind::from_id(name).ok_or_else(|| {
        let names: Vec<&str> = BackendKind::ALL.iter().map(|kind| kind.id()).collect();
        format!("`{name}` isn't a backend; try one of {}", names.join(", "))
    })
}

/// Finds the device `query` names: its ID, its index in `bl-cli devices`, or
/// its name or a unique part of it, ignoring case.
pub(crate) fn resolve(backend: &dyn Backend, query: &str) -> Result<DeviceInfo, String> {
    let devices = backend.devices().map_err(|e| e.to_string())?;
    find(devices, query, backend.kind())
}

fn find(devices: Vec<DeviceInfo>, query: &str, kind: BackendKind) -> Result<DeviceInfo, String> {
    if let Ok(id) = query.parse::<DeviceId>() {
        return devices
            .into_iter()
            .find(|device| device.id == id)
            .ok_or_else(|| format!("{kind} has no device `{id}`; see `bl-cli devices`"));
    }
    if let Ok(index) = query.parse::<usize>() {
        let count = devices.len();
        return devices
            .into_iter()
            .nth(index)
            .ok_or_else(|| format!("{kind} has {count} devices, so there's no device {index}"));
    }
    let wanted = query.to_lowercase();
    let (exact, partial): (Vec<DeviceInfo>, Vec<DeviceInfo>) = devices
        .into_iter()
        .filter(|device| device.name.to_lowercase().contains(&wanted))
        .partition(|device| device.name.to_lowercase() == wanted);
    let mut matches = if exact.is_empty() { partial } else { exact };
    match matches.len() {
        0 => Err(format!(
            "no {kind} device is called `{query}`; see `bl-cli devices`"
        )),
        1 => Ok(matches.remove(0)),
        _ => {
            let names: Vec<String> = matches.iter().map(|d| format!("`{}`", d.name)).collect();
            Err(format!(
                "`{query}` matches {}; use more of the name, or the ID",
                names.join(" and ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_audio_io::BufferSizes;

    fn device(local: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(BackendKind::Null, local),
            name: name.into(),
            is_default_output: false,
            is_default_input: false,
            output_channels: 2,
            input_channels: 0,
            sample_rates: vec![44_100, 48_000],
            current_sample_rate: Some(48_000),
            buffer_sizes: BufferSizes::Unknown,
            default_buffer_size: None,
        }
    }

    fn devices() -> Vec<DeviceInfo> {
        vec![
            device("a", "Studio Speakers"),
            device("b", "Studio"),
            device("c", "Headphones"),
        ]
    }

    fn found(query: &str) -> Result<String, String> {
        find(devices(), query, BackendKind::Null).map(|d| d.id.to_string())
    }

    #[test]
    fn devices_are_found_by_id_index_or_name() {
        assert_eq!(found("null:c"), Ok("null:c".into()));
        assert_eq!(found("0"), Ok("null:a".into()));
        assert_eq!(found("head"), Ok("null:c".into()));
        assert_eq!(found("STUDIO"), Ok("null:b".into()), "an exact name wins");
        assert_eq!(found("speak"), Ok("null:a".into()));
    }

    #[test]
    fn ambiguous_or_missing_devices_are_errors() {
        assert!(found("stu").unwrap_err().contains("matches"));
        assert!(found("mic").is_err());
        assert!(found("7").is_err());
        assert!(found("null:z").is_err());
    }

    #[test]
    fn the_current_rate_is_shown() {
        assert_eq!(
            sample_rates(&device("a", "A")),
            "44100, 48000 Hz (running at 48000)"
        );
    }

    #[test]
    fn unknown_backends_are_named_in_the_error() {
        assert_eq!(parse_backend("NULL"), Ok(BackendKind::Null));
        assert!(parse_backend("dsound").unwrap_err().contains("coreaudio"));
    }
}
