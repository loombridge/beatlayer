//! The plug-ins an import finds on this machine, and the states a file
//! holds for them (the DAWproject spec, R46 to R49, R57).
//!
//! The import never loads a plug-in itself: the host that runs it says
//! which plug-ins are installed and loads one when the import needs its
//! parameters.

use bl_model::{DeviceRole, ParamDescriptor, PluginFormat};

/// A plug-in installed on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// Its format.
    pub format: PluginFormat,
    /// Its ID in its format, as the scanner lists it.
    pub plugin_id: String,
    /// The file or bundle it is in.
    pub path: String,
    /// Its name.
    pub name: String,
    /// Where it goes in a chain.
    pub role: DeviceRole,
}

/// What an import asks of the plug-ins on the machine.
pub trait Plugins: Send + Sync {
    /// Every plug-in the scanner found.
    fn installed(&self) -> &[Installed];

    /// Loads `plugin` and describes its parameters, or says why it
    /// couldn't (R57).
    ///
    /// # Errors
    ///
    /// Why the plug-in didn't load.
    fn describe(&self, plugin: &Installed) -> Result<Vec<ParamDescriptor>, String>;
}

/// A machine with no plug-ins, for an import with no host.
pub struct NoPlugins;

impl Plugins for NoPlugins {
    fn installed(&self) -> &[Installed] {
        &[]
    }

    fn describe(&self, _plugin: &Installed) -> Result<Vec<ParamDescriptor>, String> {
        Err("no plug-in host".to_owned())
    }
}

/// The ID a plug-in has in its format, from the file's `deviceID` (R46 to
/// R49), or `None` for one that does not read.
#[must_use]
pub fn plugin_id(format: PluginFormat, device_id: &str) -> Option<String> {
    let id = device_id.trim();
    match format {
        PluginFormat::Clap | PluginFormat::Lv2 => (!id.is_empty()).then(|| id.to_owned()),
        PluginFormat::Vst3 => {
            let digits: String = id.chars().filter(|c| *c != '-').collect();
            if digits.len() != 32 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let digits = digits.to_ascii_uppercase();
            Some(format!(
                "{}-{}-{}-{}-{}",
                &digits[0..8],
                &digits[8..12],
                &digits[12..16],
                &digits[16..20],
                &digits[20..32]
            ))
        }
        PluginFormat::AudioUnit => {
            let fields: Vec<&str> = id.split(['/', ':']).collect();
            (fields.len() == 3 && fields.iter().all(|field| field.len() == 4))
                .then(|| fields.join("/"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: dawproject R46, R48, R49.
    #[test]
    fn ids_read_in_the_formats_spelling() {
        assert_eq!(
            plugin_id(PluginFormat::Vst3, "01234567-89ab-cdef-0123-456789abcdef").as_deref(),
            Some("01234567-89AB-CDEF-0123-456789ABCDEF")
        );
        assert_eq!(
            plugin_id(PluginFormat::Vst3, "0123456789ABCDEF0123456789ABCDEF").as_deref(),
            Some("01234567-89AB-CDEF-0123-456789ABCDEF")
        );
        assert_eq!(plugin_id(PluginFormat::Vst3, "nope"), None);
        assert_eq!(
            plugin_id(PluginFormat::AudioUnit, "aufx:dely:appl").as_deref(),
            Some("aufx/dely/appl")
        );
        assert_eq!(plugin_id(PluginFormat::AudioUnit, "aufx/dely"), None);
        assert_eq!(
            plugin_id(PluginFormat::Clap, "com.example.x").as_deref(),
            Some("com.example.x")
        );
    }
}
