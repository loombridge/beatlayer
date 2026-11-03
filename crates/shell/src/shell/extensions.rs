//! The extension commands through the shell (WP-9.29; the extensions spec,
//! R4 to R7 and R16): `extension.install`, `extension.remove`,
//! `extension.set_enabled`, `extension.list` and `extension.describe`, over
//! the host in `bl-ext`. Which extensions are on is a preference; the rest
//! is the User Library's `Extensions` folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_shell_spec::extensions::{
    self as ids, ComponentRow, ExtensionDescription, ExtensionList, ExtensionRow, IdArgs,
    InstallArgs, SetEnabledArgs,
};

use super::*;
use crate::packages;

/// The extension host, made by the first extension command and made again
/// if the User Library moves.
pub(super) struct ExtensionState {
    folder: PathBuf,
    host: Arc<bl_ext::Host>,
}

impl Shell {
    /// The folder extensions install into: the User Library's `Extensions`
    /// folder, or `Extensions` in the headless run's own folder when there
    /// is no User Library (R16).
    fn extensions_folder(&self) -> PathBuf {
        self.browser_view.user_library().map_or_else(
            || packages::folder(self.config_dir.as_deref()).with_file_name("Extensions"),
            |library| library.join("Extensions"),
        )
    }

    fn extension_host(&mut self, id: &str) -> Result<Arc<bl_ext::Host>, ShellError> {
        let folder = self.extensions_folder();
        if let Some(state) = self
            .extensions
            .as_ref()
            .filter(|state| state.folder == folder)
        {
            return Ok(state.host.clone());
        }
        let cache = self.cache_folder().join("Extensions");
        let host = bl_ext::Host::new(folder.clone(), cache, bl_ext::Options::default())
            .map_err(|e| extension_error(id, e))?;
        let host = Arc::new(host);
        self.extensions = Some(ExtensionState {
            folder,
            host: host.clone(),
        });
        Ok(host)
    }

    fn extension_enabled(&self, id: &str) -> bool {
        !self.prefs.disabled_extensions.contains(id)
    }

    fn extension_list(&self, host: &bl_ext::Host) -> ExtensionList {
        let extensions = host
            .installed()
            .into_iter()
            .map(|extension| {
                let manifest = extension.manifest;
                let mut kinds: Vec<String> = Vec::new();
                for component in &manifest.components {
                    let kind = component.kind.name().to_owned();
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
                ExtensionRow {
                    enabled: self.extension_enabled(&manifest.id),
                    last_failure: host.failure(&manifest.id),
                    id: manifest.id,
                    name: manifest.name,
                    version: manifest.version.to_string(),
                    kinds,
                }
            })
            .collect();
        ExtensionList {
            folder: self.extensions_folder().to_string_lossy().into_owned(),
            extensions,
        }
    }

    pub(super) fn run_extensions(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let host = self.extension_host(id)?;
        match id {
            ids::LIST => {}
            ids::INSTALL => {
                let InstallArgs { path } = Self::args(id, args)?;
                host.install(Path::new(&path))
                    .map_err(|e| extension_error(id, e))?;
                self.touch();
            }
            ids::REMOVE => {
                let IdArgs { id: extension } = Self::args(id, args)?;
                host.remove(&extension)
                    .map_err(|e| extension_error(id, e))?;
                self.prefs.disabled_extensions.remove(&extension);
                self.save_preferences();
                self.touch();
            }
            ids::SET_ENABLED => {
                let SetEnabledArgs {
                    id: extension,
                    enabled,
                } = Self::args(id, args)?;
                if host.extension(&extension).is_none() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("no extension {extension} is installed"),
                    });
                }
                if enabled {
                    // R12: a failed extension stays off until it is switched on.
                    host.clear_failure(&extension);
                    self.prefs.disabled_extensions.remove(&extension);
                } else {
                    self.prefs.disabled_extensions.insert(extension);
                }
                self.save_preferences();
                self.touch();
            }
            ids::DESCRIBE => {
                let IdArgs { id: extension } = Self::args(id, args)?;
                let installed =
                    host.extension(&extension)
                        .ok_or_else(|| ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!("no extension {extension} is installed"),
                        })?;
                let m = installed.manifest;
                return Ok(to_value(&ExtensionDescription {
                    enabled: self.extension_enabled(&m.id),
                    last_failure: host.failure(&m.id),
                    components: m
                        .components
                        .iter()
                        .map(|c| ComponentRow {
                            file: c.file.clone(),
                            kind: c.kind.name().to_owned(),
                            world: c.world.to_string(),
                            imports: c.imports.clone(),
                            memory_mib: c.memory_mib,
                        })
                        .collect(),
                    id: m.id,
                    name: m.name,
                    version: m.version.to_string(),
                    author: m.author,
                    licence: m.licence,
                    description: m.description,
                    // R44: none until extensions reach the set (R38).
                    calls: Vec::new(),
                }));
            }
            _ => return Err(ShellError::UnknownCommand(id.to_owned())),
        }
        Ok(to_value(&self.extension_list(&host)))
    }
}

/// A refused package or request is the caller's to fix; anything else failed.
fn extension_error(command: &str, error: bl_ext::Error) -> ShellError {
    match error {
        bl_ext::Error::Refused(message) => ShellError::InvalidArgs {
            command: command.to_owned(),
            message,
        },
        other => ShellError::Failed {
            command: command.to_owned(),
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::Duration;

    use serde_json::json;
    use zip::write::SimpleFileOptions;

    use super::*;

    const COMPONENT: &str = r#"(component
  (core module $m (func (export "trap") unreachable))
  (core instance $i (instantiate $m))
  (func $trap (canon lift (core func $i "trap")))
  (instance $api (export "trap" (func $trap)))
  (export "beatlayer:ext/midi-tool@0.1.0" (instance $api)))"#;

    const MANIFEST: &str = r#"id = "org.example.tools"
name = "Test Tools"
version = "1.0.0"
author = "A. Person"
licence = "MIT"
description = "Tools for tests."

[[components]]
file = "tools.wasm"
kind = "midi-tool"
world = "0.1.0"
"#;

    /// A shell whose User Library is a new folder, and a package for it.
    fn fixture(name: &str, manifest: &str) -> (Shell, PathBuf) {
        let root = std::env::temp_dir().join(format!("bl-shell-ext-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let package = root.join("tools.blext");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&package).unwrap());
        for (file, bytes) in [
            ("extension.toml", manifest.as_bytes().to_vec()),
            ("tools.wasm", wat::parse_str(COMPONENT).unwrap()),
        ] {
            zip.start_file(file, SimpleFileOptions::default()).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
        let mut shell = Shell::headless();
        shell.set_user_library(Some(root.join("User Library")));
        (shell, package)
    }

    fn list(shell: &mut Shell) -> Value {
        shell.run(ids::LIST, json!({})).unwrap()["extensions"].clone()
    }

    // Spec: extensions R4, R6, R7, R16. An extension installs, lists, switches
    // off and on, describes and removes, each read back by the list.
    #[test]
    fn an_extension_installs_lists_switches_and_removes() {
        let (mut shell, package) = fixture("lifecycle", MANIFEST);
        assert_eq!(list(&mut shell), json!([]));

        let path = package.to_string_lossy();
        let installed = shell.run(ids::INSTALL, json!({ "path": path })).unwrap();
        assert_eq!(installed["extensions"], list(&mut shell));
        assert_eq!(
            list(&mut shell),
            json!([{
                "id": "org.example.tools", "name": "Test Tools", "version": "1.0.0",
                "kinds": ["midi-tool"], "enabled": true
            }])
        );
        assert!(
            installed["folder"]
                .as_str()
                .unwrap()
                .ends_with("User Library/Extensions")
                || installed["folder"]
                    .as_str()
                    .unwrap()
                    .ends_with("User Library\\Extensions")
        );

        shell
            .run(
                ids::SET_ENABLED,
                json!({ "id": "org.example.tools", "enabled": false }),
            )
            .unwrap();
        assert_eq!(list(&mut shell)[0]["enabled"], json!(false));
        let described = shell
            .run(ids::DESCRIBE, json!({ "id": "org.example.tools" }))
            .unwrap();
        assert_eq!(described["enabled"], json!(false));
        assert_eq!(described["author"], json!("A. Person"));
        assert_eq!(described["components"][0]["world"], json!("0.1.0"));
        shell
            .run(
                ids::SET_ENABLED,
                json!({ "id": "org.example.tools", "enabled": true }),
            )
            .unwrap();
        assert_eq!(list(&mut shell)[0]["enabled"], json!(true));

        shell
            .run(ids::REMOVE, json!({ "id": "org.example.tools" }))
            .unwrap();
        assert_eq!(list(&mut shell), json!([]));
        assert!(
            shell
                .run(ids::DESCRIBE, json!({ "id": "org.example.tools" }))
                .is_err()
        );
    }

    // Spec: extensions R3. A refused package gives its reason and keeps nothing.
    #[test]
    fn a_refused_package_gives_its_reason_and_keeps_nothing() {
        let (mut shell, package) = fixture("refused", &MANIFEST.replace("0.1.0", "0.2.0"));
        let error = shell
            .run(ids::INSTALL, json!({ "path": package.to_string_lossy() }))
            .unwrap_err();
        assert!(error.to_string().contains("world version 0.2.0"), "{error}");
        assert_eq!(list(&mut shell), json!([]));
        let missing = shell
            .run(ids::INSTALL, json!({ "path": "/nowhere/x.blext" }))
            .unwrap_err();
        assert!(matches!(missing, ShellError::Failed { .. }), "{missing}");
        let unknown = shell
            .run(
                ids::SET_ENABLED,
                json!({ "id": "org.example.none", "enabled": false }),
            )
            .unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("no extension org.example.none"),
            "{unknown}"
        );
    }

    // Spec: extensions R7, R10, R12. A component that traps is stopped with its
    // reason, which the list shows until the extension is switched on again.
    #[test]
    fn a_stopped_component_shows_its_reason_until_it_is_switched_on_again() {
        let (mut shell, package) = fixture("failure", MANIFEST);
        shell
            .run(ids::INSTALL, json!({ "path": package.to_string_lossy() }))
            .unwrap();
        let host = shell.extension_host(ids::LIST).unwrap();
        let mut instance = host.instantiate("org.example.tools", "tools.wasm").unwrap();
        assert!(instance.call("trap", &[], Duration::from_secs(1)).is_err());

        let row = list(&mut shell)[0].clone();
        let reason = row["last_failure"].as_str().unwrap();
        assert!(reason.starts_with("tools.wasm: trapped"), "{reason}");

        shell
            .run(
                ids::SET_ENABLED,
                json!({ "id": "org.example.tools", "enabled": true }),
            )
            .unwrap();
        assert!(list(&mut shell)[0].get("last_failure").is_none());
    }
}
