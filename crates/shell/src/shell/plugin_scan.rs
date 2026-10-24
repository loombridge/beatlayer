//! Finding the plug-ins and the hosts that load them.

use super::Shell;
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::lv2::Lv2Host;
use bl_plugin_host::vst3::Vst3Host;
use bl_shell_spec::plugins as plugin_ids;
use std::sync::Arc;

impl Shell {
    /// What the plug-in scanner has found, with the browser's hidden and
    /// format filters applied (WP-2.9, IMP-07), pushed into the browser's
    /// Plug-Ins label.
    pub(super) fn publish_plugins(&mut self) {
        let formats = crate::plugins::listed_formats(&self.prefs);
        let mut plugins = self.scanner.visible(
            self.prefs.show_hidden_plugins,
            &self.prefs.hidden_plugins,
            self.prefs.plugin_format_preference,
        );
        // Spec: plugin-scanning R12. A format is listed only while its
        // Plug-In Sources switch is on, as in Live.
        plugins.retain(|p| formats.contains(&p.format));
        self.browser_view.set_plugins(plugins);
    }

    /// Starts scanning plug-ins by itself, as Live does at launch (WP-2.9b;
    /// the plug-in scanning spec, R13): a scan of the system folders of the
    /// formats switched on runs now in the background, answered from the
    /// cache for unchanged files, and again whenever a Plug-In Sources
    /// switch turns on. The browser lists what the cache knew until it
    /// finishes. The app calls it once its window is up; tests and tools
    /// that don't, scan only when asked.
    pub fn start_plugin_scan(&mut self) {
        self.auto_scan = true;
        self.spawn_plugin_scan(false);
    }

    /// Whether a background plug-in scan is under way.
    #[must_use]
    pub fn plugin_scan_running(&self) -> bool {
        self.plugin_scan.is_some()
    }

    pub(super) fn spawn_plugin_scan(&mut self, fresh: bool) {
        if self.plugin_scan.is_some() {
            self.plugin_scan_again = Some(self.plugin_scan_again.unwrap_or(false) | fresh);
            return;
        }
        let scanner = Arc::clone(&self.scanner);
        let formats = crate::plugins::listed_formats(&self.prefs);
        let waker = self.browser_view.waker().clone();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let spawned = std::thread::Builder::new()
            .name("plugin-scan".to_owned())
            .spawn(move || {
                let _ = crate::plugins::scan_report(
                    &scanner,
                    None,
                    fresh,
                    &formats,
                    &std::collections::BTreeSet::new(),
                );
                finished.store(true, std::sync::atomic::Ordering::Release);
                waker.wake();
            });
        self.plugin_scan = spawned.ok().map(|handle| (handle, done));
    }

    /// Takes a finished background scan's results into the browser, and
    /// starts the next one if one was asked for. Returns whether the
    /// browser changed.
    pub(super) fn poll_plugin_scan(&mut self) -> bool {
        if !self
            .plugin_scan
            .as_ref()
            .is_some_and(|(_, done)| done.load(std::sync::atomic::Ordering::Acquire))
        {
            return false;
        }
        if let Some((scan, _)) = self.plugin_scan.take() {
            let _ = scan.join();
        }
        self.publish_plugins();
        if let Some(fresh) = self.plugin_scan_again.take() {
            self.spawn_plugin_scan(fresh);
        }
        true
    }

    /// Waits for the background plug-in scan to finish, up to `timeout`,
    /// for tests and tools. Returns whether it finished.
    pub fn wait_for_plugin_scan(&mut self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.poll_plugin_scan();
            if self.plugin_scan.is_none() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The CLAP host, whose plug-ins are the set's CLAP devices
    /// (WP-2.6).
    #[must_use]
    pub fn plugins(&self) -> &ClapHost {
        &self.plugins
    }

    /// The VST3 host, whose plug-ins are the set's VST3 devices (WP-2.7).
    #[must_use]
    pub fn vst3(&self) -> &Vst3Host {
        &self.vst3
    }

    /// The LV2 host, whose plug-ins are the set's LV2 devices (WP-5.39).
    #[must_use]
    pub fn lv2(&self) -> &Lv2Host {
        &self.lv2
    }

    /// The hosts an offline render borrows the engine's plug-ins from
    /// (WP-2.6b, WP-4.21).
    #[must_use]
    pub fn plugin_hosts(&self) -> bl_plugin_host::Hosts<'_> {
        bl_plugin_host::Hosts {
            clap: Some(&self.plugins),
            vst3: Some(&self.vst3),
            au: Some(&self.au),
            lv2: Some(&self.lv2),
        }
    }

    /// What `plugin.info` reports for a plug-in device, for the Device
    /// View's window button (WP-2.10): whether it has an editor and whether
    /// its window is open. None for a device that isn't a loaded plug-in.
    #[must_use]
    pub fn plugin_info(&self, device: bl_model::DeviceId) -> Option<plugin_ids::PluginInfo> {
        crate::plugins::info(&self.plugins, &self.au, &self.vst3, &self.lv2, device)
    }

    /// How the Browser shows plug-ins, and the Plug-In Sources switches:
    /// what `plugin.settings` returns.
    #[must_use]
    pub fn plugin_settings(&self) -> plugin_ids::PluginSettings {
        plugin_ids::PluginSettings {
            hidden: self.prefs.hidden_plugins.iter().cloned().collect(),
            show_hidden: self.prefs.show_hidden_plugins,
            format_preference: self.prefs.plugin_format_preference,
            use_audio_units_v2: self.prefs.use_audio_units_v2,
            use_audio_units_v3: self.prefs.use_audio_units_v3,
            vst3_system_folders: self.prefs.vst3_system_folders,
        }
    }
}
