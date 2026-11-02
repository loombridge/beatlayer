//! Typed object IDs, parameter IDs, and media content hashes.

use core::fmt;
use core::str::FromStr;
use std::borrow::Cow;

use bl_core::id::Id;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! object_id {
    ($(#[$doc:meta])* $name:ident, $label:literal) => {
        $(#[$doc])*
        ///
        /// It wraps the project-wide [`Id`], so it never matches the ID of any
        /// other object in the project. It serialises as a plain integer.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(pub Id);

        impl $name {
            /// The project-wide ID.
            #[must_use]
            pub const fn id(self) -> Id {
                self.0
            }
        }

        impl From<Id> for $name {
            fn from(id: Id) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Id {
            fn from(id: $name) -> Id {
                id.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0.get())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($label, " {}"), self.0)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Borrowed(concat!("bl_model::", stringify!($name)))
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "description": concat!(
                        "The ID of a ", $label,
                        ": a positive integer, unique within the project and never reused."
                    ),
                    "type": "integer",
                    "format": "uint64",
                    "minimum": 1
                })
            }
        }
    };
}

object_id!(
    /// The ID of a track.
    TrackId,
    "track"
);
object_id!(
    /// The ID of a clip.
    ClipId,
    "clip"
);
object_id!(
    /// The ID of a scene: a row of the Session grid.
    SceneId,
    "scene"
);
object_id!(
    /// The ID of a device in a track's device chain.
    DeviceId,
    "device"
);
object_id!(
    /// The ID of a chain in a rack (WP-5.1).
    ChainId,
    "chain"
);
object_id!(
    /// The ID of a take lane under a track's main lane (WP-6.21).
    TakeLaneId,
    "take lane"
);
object_id!(
    /// The ID of a group of linked tracks (WP-6.23).
    LinkGroupId,
    "link group"
);
object_id!(
    /// The ID of a locator.
    LocatorId,
    "locator"
);
object_id!(
    /// The ID of a region of the chord lane (WP-6.48).
    ChordRegionId,
    "chord region"
);
object_id!(
    /// The ID of a groove in the Groove Pool (WP-6.19).
    GrooveId,
    "groove"
);

// A parameter of a device, numbered by the device: the plug-in kit's
// `lspk-params` defines it, with the parameter descriptors.
pub use lspk_params::ParamId;

/// The content hash of a media file: 32 bytes, shown as 64 lowercase hex
/// digits.
///
/// Clips refer to audio by content hash, never by path, and the document
/// never holds samples. The media store (WP-1.1) decides how the hash is
/// computed and maps it to a file.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

/// Why a string isn't a [`ContentHash`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseContentHashError;

impl fmt::Display for ParseContentHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a content hash is 64 hex digits")
    }
}

impl std::error::Error for ParseContentHashError {}

impl FromStr for ContentHash {
    type Err = ParseContentHashError;

    /// Parses 64 hex digits, in either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s.as_bytes();
        if digits.len() != 64 {
            return Err(ParseContentHashError);
        }
        let mut bytes = [0; 32];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, &[high, low]) in bytes.iter_mut().zip(pairs) {
            let high = hex_digit(high).ok_or(ParseContentHashError)?;
            let low = hex_digit(low).ok_or(ParseContentHashError)?;
            *byte = high << 4 | low;
        }
        Ok(Self(bytes))
    }
}

fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl JsonSchema for ContentHash {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ContentHash")
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("bl_model::ContentHash")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "The content hash of a media file in the project's media store, as 64 hex digits. Clips name their audio by hash, never by path.",
            "type": "string",
            "pattern": "^[0-9a-fA-F]{64}$",
            "minLength": 64,
            "maxLength": 64
        })
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_ids_show_their_kind() {
        let id = Id::new(5).unwrap();
        assert_eq!(TrackId(id).to_string(), "track #5");
        assert_eq!(format!("{:?}", ClipId(id)), "ClipId(5)");
        assert_eq!(serde_json::to_string(&DeviceId(id)).unwrap(), "5");
    }

    #[test]
    fn content_hash_round_trips_through_hex() {
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::try_from(i * 8 + 1).unwrap();
        }
        let hash = ContentHash(bytes);
        let text = hash.to_string();
        assert_eq!(text.len(), 64);
        assert!(text.starts_with("0109111921"));
        assert_eq!(text.parse::<ContentHash>(), Ok(hash));
        assert_eq!(text.to_uppercase().parse::<ContentHash>(), Ok(hash));

        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<ContentHash>(&json).unwrap(), hash);
    }

    #[test]
    fn content_hash_rejects_bad_text() {
        assert!("".parse::<ContentHash>().is_err());
        assert!("ab".repeat(31).parse::<ContentHash>().is_err());
        assert!("zz".repeat(32).parse::<ContentHash>().is_err());
        assert!("é".repeat(32).parse::<ContentHash>().is_err());
    }
}
