//! Host-supplied Workspace identity.
//!
//! A Workspace ID is 16 opaque bytes. Hosts generate them (UUIDv7 in
//! practice); Andamento only compares, orders and prints them, so the core
//! stays deterministic for replay.
//!
//! ABI 2's 64-bit IDs embed as the IDs whose first eight bytes are zero, with
//! the `u64` in the last eight bytes, big-endian. A UUIDv7 never has that form
//! (its version nibble sits in byte 6), so embedded and generated IDs never
//! collide, and both kinds can be used side by side.
use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId([u8; 16]);

impl WorkspaceId {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// The ABI 2 embedding of a 64-bit ID.
    pub const fn from_u64(id: u64) -> Self {
        Self((id as u128).to_be_bytes())
    }

    /// The 64-bit ID this embeds, or `None` for a wider ID.
    pub fn as_u64(self) -> Option<u64> {
        u64::try_from(u128::from_be_bytes(self.0)).ok()
    }
}

impl From<u64> for WorkspaceId {
    fn from(id: u64) -> Self {
        Self::from_u64(id)
    }
}

/// Embedded IDs print as their decimal `u64`, so existing keys and fallback
/// entity IDs are unchanged; wider IDs print as a lowercase hyphenated UUID.
impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(id) = self.as_u64() {
            return write!(f, "{id}");
        }
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorkspaceId({self})")
    }
}

/// Accepts the printed forms: a decimal `u64`, 32 hex digits, or a hyphenated
/// UUID (8-4-4-4-12), in either case.
impl FromStr for WorkspaceId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("invalid workspace ID {text:?}");
        let hex = match text.len() {
            32 => text.to_owned(),
            36 => {
                let parts: Vec<_> = text.split('-').collect();
                if parts.iter().map(|p| p.len()).ne([8, 4, 4, 4, 12]) {
                    return Err(invalid());
                }
                parts.concat()
            }
            _ => {
                if !text.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid());
                }
                return text.parse::<u64>().map(Self::from).map_err(|_| invalid());
            }
        };
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let value = u128::from_str_radix(&hex, 16).map_err(|_| invalid())?;
        Ok(Self(value.to_be_bytes()))
    }
}

/// Embedded IDs serialize as JSON numbers, as ABI 2 IDs always have; wider IDs
/// as UUID strings. Both forms, and decimal strings, deserialize.
impl Serialize for WorkspaceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.as_u64() {
            Some(id) => serializer.serialize_u64(id),
            None => serializer.collect_str(self),
        }
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = WorkspaceId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 64-bit workspace ID or a UUID string")
            }

            fn visit_u64<E: serde::de::Error>(self, id: u64) -> Result<WorkspaceId, E> {
                Ok(id.into())
            }

            fn visit_i64<E: serde::de::Error>(self, id: i64) -> Result<WorkspaceId, E> {
                u64::try_from(id)
                    .map(WorkspaceId::from)
                    .map_err(|_| E::custom("negative workspace ID"))
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<WorkspaceId, E> {
                text.parse().map_err(E::custom)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceId;

    const UUID: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b";

    #[test]
    fn abi_2_ids_embed_with_zero_high_bytes() {
        let id = WorkspaceId::from(0x0102_0304_0506_0708);
        assert_eq!(
            id.to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert_eq!(id.as_u64(), Some(0x0102_0304_0506_0708));
        assert_eq!(id.to_string(), "72623859790382856");
        assert_eq!(WorkspaceId::from(0).to_string(), "0");
    }

    #[test]
    fn wide_ids_print_and_parse_as_uuids() {
        let id: WorkspaceId = UUID.parse().unwrap();
        assert_eq!(id.as_u64(), None);
        assert_eq!(id.to_bytes()[6] >> 4, 7);
        assert_eq!(id.to_string(), UUID);
        assert_eq!(UUID.to_uppercase().parse::<WorkspaceId>(), Ok(id));
        assert_eq!(UUID.replace('-', "").parse::<WorkspaceId>(), Ok(id));
        assert_eq!("7".parse::<WorkspaceId>(), Ok(WorkspaceId::from(7)));
        for bad in [
            "",
            "-7",
            "x",
            "01920a6b7c3d-7e4f-8a1b-2c3d4e5f6a7b-",
            "18446744073709551616",
        ] {
            assert!(bad.parse::<WorkspaceId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn json_keeps_numbers_for_embedded_ids_and_strings_for_wide_ones() {
        let narrow = WorkspaceId::from(7);
        let wide: WorkspaceId = UUID.parse().unwrap();
        assert_eq!(serde_json::to_string(&narrow).unwrap(), "7");
        assert_eq!(serde_json::to_string(&wide).unwrap(), format!("\"{UUID}\""));
        for (json, id) in [
            ("7", narrow),
            ("\"7\"", narrow),
            (&format!("\"{UUID}\""), wide),
        ] {
            assert_eq!(serde_json::from_str::<WorkspaceId>(json).unwrap(), id);
        }
        assert!(serde_json::from_str::<WorkspaceId>("-1").is_err());
    }
}
