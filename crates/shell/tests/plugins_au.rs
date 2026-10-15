use bl_model::TrackKind;
use bl_shell::Shell;
use bl_shell::plugin_commands as ids;
use serde_json::json;

#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use bl_model::{DeviceId, ParamId};
#[cfg(target_os = "macos")]
use bl_shell::commands;
#[cfg(target_os = "macos")]
use bl_shell::plugin_commands::{PluginInfo, PluginLoaded};
#[cfg(target_os = "macos")]
use serde_json::Value;

fn audio_track(shell: &Shell) -> bl_model::TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track")
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    /// AUDelay's Dry/Wet Mix, its first parameter.
    const MIX: u32 = 0;

    fn value(info: &PluginInfo, param: u32) -> Option<f64> {
        info.values
            .iter()
            .find(|(id, _)| *id == ParamId(param))
            .map(|(_, v)| *v)
    }

    /// Polls the shell, as the app's frames do, until the device's unit
    /// reports `ready`.
    fn wait(
        shell: &mut Shell,
        device: DeviceId,
        ready: impl Fn(&PluginInfo) -> bool,
    ) -> PluginInfo {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            shell.poll();
            if let Ok(value) = shell.run(ids::INFO, json!({ "device": device }))
                && let Ok(info) = serde_json::from_value::<PluginInfo>(value)
                && ready(&info)
            {
                return info;
            }
            assert!(Instant::now() < until, "the unit never got ready");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // Spec: au-hosting R1, R8, R9, R11, R16
    #[test]
    fn an_audio_unit_loads_runs_and_keeps_its_state_with_the_set() {
        let scratch = std::env::temp_dir().join(format!("bl-shell-au-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();

        let mut shell = Shell::headless();
        shell.start_audio();
        assert!(shell.audio().engine_running(), "{:?}", shell.audio());
        let track = audio_track(&shell);
        let loaded: PluginLoaded = serde_json::from_value(
            shell
                .run(
                    ids::LOAD_AU,
                    json!({
                        "type": "aufx",
                        "subtype": "dely",
                        "manufacturer": "appl",
                        "track": track,
                    }),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            (loaded.plugin.as_str(), loaded.name.as_str()),
            ("aufx/dely/appl", "AUDelay")
        );
        assert_eq!(shell.editor().undo_history().next(), Some("Insert Device"));
        let device = loaded.device;

        // The engine runs it, and a parameter change reaches it.
        let info = wait(&mut shell, device, |i| i.active);
        assert_eq!(info.failure, None);
        shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": MIX, "value": 25.0 }),
            )
            .unwrap();
        wait(&mut shell, device, |i| value(i, MIX) == Some(25.0));

        // Its editor window isn't hosted yet; asking changes nothing.
        let history = shell.editor().undo_history().len();
        let refused = shell.run(ids::SHOW_WINDOW, json!({ "device": device }));
        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(shell.editor().undo_history().len(), history);

        // Saved, the set holds its state, taken in without an undo step.
        let saved = shell
            .run(
                commands::SET_SAVE_AS,
                json!({ "path": scratch.join("Au.blset") }),
            )
            .unwrap();
        let path = saved["path"].as_str().unwrap().to_owned();
        let state = shell
            .editor()
            .document()
            .device(device)
            .and_then(|d| d.plugin.clone())
            .expect("the state, taken in before the save");
        assert_eq!(&state.data[..8], b"bplist00", "a binary property list");
        assert!(state.path.is_empty(), "an Audio Unit has no file");
        assert_eq!(shell.editor().undo_history().len(), history);

        // Opened again, a new unit starts from it.
        shell.run(commands::SET_NEW, json!({})).unwrap();
        shell
            .run(commands::SET_OPEN, json!({ "path": path }))
            .unwrap();
        let info = wait(&mut shell, device, |i| i.active);
        assert_eq!(value(&info, MIX), Some(25.0));
        assert_eq!(
            shell.editor().document().device(device).unwrap().plugin,
            Some(state)
        );

        // A description no unit has is refused, changing nothing.
        let before = shell.editor().undo_history().len();
        let refused = shell.run(
            ids::LOAD_AU,
            json!({
                "type": "aufx",
                "subtype": "zzzz",
                "manufacturer": "appl",
                "track": track,
            }),
        );
        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(shell.editor().undo_history().len(), before);

        // A device that isn't a plug-in has no unit to report.
        let not_a_plugin: Value =
            json!({ "device": DeviceId(bl_core::id::Id::new(999_999).unwrap()) });
        assert!(shell.run(ids::INFO, not_a_plugin).is_err());
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

// Spec: au-hosting R1, R16
#[cfg(not(target_os = "macos"))]
#[test]
fn loading_an_audio_unit_says_they_are_macos_only() {
    let mut shell = Shell::headless();
    let track = audio_track(&shell);
    let before = shell.editor().undo_history().len();
    let refused = shell.run(
        ids::LOAD_AU,
        json!({
            "type": "aufx",
            "subtype": "dely",
            "manufacturer": "appl",
            "track": track,
        }),
    );
    let message = format!("{refused:?}");
    assert!(message.contains("macOS"), "{message}");
    assert_eq!(shell.editor().undo_history().len(), before);
}
