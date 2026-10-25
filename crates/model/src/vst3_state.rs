//! A VST3 plug-in's saved state, and the `.vstpreset` file that carries one
//! (the VST3 hosting spec, R9; the DAWproject spec, R48).
//!
//! The host keeps a device's state as the component's state and the
//! controller's, each length-prefixed ([`plugin_state`]). A `.vstpreset`
//! file holds the same two streams in chunks, so reading one gives the
//! bytes the host loads, and the host and the DAWproject reader share this
//! one definition of the layout.

/// What a saved state starts with: the component's state and the
/// controller's, each with its length.
const STATE_MAGIC: &[u8; 4] = b"BLTV";

/// The state bytes a VST3 host saves: the component's state and the
/// controller's, each length-prefixed, little-endian, after the magic.
/// Tests and tools use it to hand a plug-in a state.
#[must_use]
pub fn plugin_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(12 + component.len() + controller.len());
    data.extend_from_slice(STATE_MAGIC);
    for chunk in [component, controller] {
        data.extend_from_slice(&u32::try_from(chunk.len()).unwrap_or(u32::MAX).to_le_bytes());
        data.extend_from_slice(chunk);
    }
    data
}

/// Splits [`plugin_state`]'s bytes back into the component's state and the
/// controller's.
#[must_use]
pub fn split_state(data: &[u8]) -> Option<(&[u8], &[u8])> {
    if data.get(..4) != Some(STATE_MAGIC.as_slice()) {
        return None;
    }
    let length = |at: usize| -> Option<usize> {
        let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(bytes)).ok()
    };
    let component_len = length(4)?;
    let component = data.get(8..8 + component_len)?;
    let controller_at = 8 + component_len;
    let controller_len = length(controller_at)?;
    let controller = data.get(controller_at + 4..controller_at + 4 + controller_len)?;
    Some((component, controller))
}

/// A `.vstpreset` file starts with this.
const PRESET_MAGIC: &[u8; 4] = b"VST3";
/// The chunk list starts with this.
const LIST_MAGIC: &[u8; 4] = b"List";
/// The file version the VST3 SDK writes.
const PRESET_VERSION: u32 = 1;
/// The header: the magic, the version, the 32-character class ID and the
/// offset of the chunk list.
const HEADER_LEN: usize = 48;
/// The chunk holding the component's state.
const COMPONENT_CHUNK: &[u8; 4] = b"Comp";
/// The chunk holding the controller's state.
const CONTROLLER_CHUNK: &[u8; 4] = b"Cont";

/// A `.vstpreset` file's class ID and the two states it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VstPreset {
    /// The plug-in's class ID, as the file spells it: 32 characters.
    pub class_id: String,
    /// The component's state, from the `Comp` chunk.
    pub component: Vec<u8>,
    /// The controller's state, from the `Cont` chunk; empty where the file
    /// has none.
    pub controller: Vec<u8>,
}

/// Why a `.vstpreset` file didn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotAPreset;

impl VstPreset {
    /// Reads a `.vstpreset` file.
    ///
    /// # Errors
    ///
    /// [`NotAPreset`] if the bytes aren't one, or a chunk reaches past them.
    pub fn read(bytes: &[u8]) -> Result<Self, NotAPreset> {
        if bytes.get(..4) != Some(PRESET_MAGIC.as_slice()) || bytes.len() < HEADER_LEN {
            return Err(NotAPreset);
        }
        let class_id = std::str::from_utf8(&bytes[8..40]).map_err(|_| NotAPreset)?;
        let list_at = offset(&bytes[40..48])?;
        let list = bytes.get(list_at..).ok_or(NotAPreset)?;
        if list.get(..4) != Some(LIST_MAGIC.as_slice()) {
            return Err(NotAPreset);
        }
        let count = list
            .get(4..8)
            .and_then(|count| <[u8; 4]>::try_from(count).ok())
            .map(u32::from_le_bytes)
            .ok_or(NotAPreset)?;
        let mut preset = Self {
            class_id: class_id.to_owned(),
            component: Vec::new(),
            controller: Vec::new(),
        };
        let mut entry = list_at + 8;
        for _ in 0..count.min(64) {
            let record = bytes
                .get(entry..)
                .and_then(|rest| rest.get(..20))
                .ok_or(NotAPreset)?;
            let (at, size) = (offset(&record[4..12])?, offset(&record[12..20])?);
            let chunk = bytes.get(at..at.checked_add(size).ok_or(NotAPreset)?);
            match &record[..4] {
                id if id == COMPONENT_CHUNK => preset.component = chunk.ok_or(NotAPreset)?.to_vec(),
                id if id == CONTROLLER_CHUNK => {
                    preset.controller = chunk.ok_or(NotAPreset)?.to_vec();
                }
                _ => {}
            }
            entry += 20;
        }
        Ok(preset)
    }

    /// The bytes of the file: the header, the chunks, then the list.
    #[must_use]
    pub fn write(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(HEADER_LEN + self.component.len() + self.controller.len() + 48);
        out.extend_from_slice(PRESET_MAGIC);
        out.extend_from_slice(&PRESET_VERSION.to_le_bytes());
        out.extend_from_slice(self.class_id.as_bytes());
        out.extend_from_slice(&[0; 8]);
        let mut list = Vec::new();
        for (id, data) in [
            (COMPONENT_CHUNK, &self.component),
            (CONTROLLER_CHUNK, &self.controller),
        ] {
            if data.is_empty() && id == CONTROLLER_CHUNK {
                continue;
            }
            list.push((*id, out.len() as u64, data.len() as u64));
            out.extend_from_slice(data);
        }
        let list_at = out.len() as u64;
        out[40..48].copy_from_slice(&list_at.to_le_bytes());
        out.extend_from_slice(LIST_MAGIC);
        out.extend_from_slice(&(list.len() as u32).to_le_bytes());
        for (id, at, size) in list {
            out.extend_from_slice(&id);
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        out
    }

    /// The state bytes the VST3 host loads: [`plugin_state`] of the two
    /// streams.
    #[must_use]
    pub fn host_state(&self) -> Vec<u8> {
        plugin_state(&self.component, &self.controller)
    }
}

/// A little-endian 64-bit offset or size as a `usize`.
fn offset(bytes: &[u8]) -> Result<usize, NotAPreset> {
    let bytes: [u8; 8] = bytes.try_into().map_err(|_| NotAPreset)?;
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| NotAPreset)
}

/// Whether two VST3 class IDs are the same, ignoring case and dashes (the
/// DAWproject spec, R48).
#[must_use]
pub fn same_class_id(a: &str, b: &str) -> bool {
    let digits = |id: &str| -> String {
        id.chars()
            .filter(|c| *c != '-')
            .map(|c| c.to_ascii_uppercase())
            .collect()
    };
    digits(a) == digits(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: dawproject R48. A preset reads back what it wrote, and gives the
    // host's own state layout.
    #[test]
    fn a_preset_reads_back_the_streams_it_wrote() {
        let preset = VstPreset {
            class_id: "0123456789ABCDEF0123456789ABCDEF".to_owned(),
            component: vec![1, 2, 3],
            controller: vec![4, 5],
        };
        let read = VstPreset::read(&preset.write()).unwrap();
        assert_eq!(read, preset);
        assert_eq!(
            split_state(&read.host_state()),
            Some((&[1u8, 2, 3][..], &[4u8, 5][..]))
        );
    }

    #[test]
    fn a_preset_without_a_controller_chunk_has_an_empty_controller() {
        let preset = VstPreset {
            class_id: "0123456789ABCDEF0123456789ABCDEF".to_owned(),
            component: vec![9],
            controller: Vec::new(),
        };
        assert_eq!(VstPreset::read(&preset.write()).unwrap(), preset);
    }

    #[test]
    fn damaged_bytes_are_not_a_preset() {
        let bytes = VstPreset {
            class_id: "0123456789ABCDEF0123456789ABCDEF".to_owned(),
            component: vec![1; 8],
            controller: vec![2; 8],
        }
        .write();
        for cut in 0..bytes.len() {
            assert!(VstPreset::read(&bytes[..cut]).is_err());
        }
        assert!(VstPreset::read(b"nope").is_err());
    }

    #[test]
    fn offsets_past_the_end_are_not_a_preset() {
        let mut bytes = VstPreset {
            class_id: "0123456789ABCDEF0123456789ABCDEF".to_owned(),
            component: vec![1],
            controller: Vec::new(),
        }
        .write();
        bytes[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(VstPreset::read(&bytes).is_err());
    }

    #[test]
    fn class_ids_compare_ignoring_case_and_dashes() {
        assert!(same_class_id(
            "01234567-89ab-cdef-0123-456789abcdef",
            "0123456789ABCDEF0123456789ABCDEF"
        ));
        assert!(!same_class_id("01", "02"));
    }
}
