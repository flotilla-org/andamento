//! Host-supplied Workspace identity.
//!
//! A Workspace ID is 16 opaque bytes. Hosts generate them (UUIDv7 in
//! practice); Andamento only compares, orders and prints them, so the core
//! stays deterministic for replay.
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
}

/// A lowercase hyphenated UUID.
impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

/// Accepts a hyphenated UUID (8-4-4-4-12) or 32 hex digits, in either case.
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
            _ => return Err(invalid()),
        };
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let value = u128::from_str_radix(&hex, 16).map_err(|_| invalid())?;
        Ok(Self(value.to_be_bytes()))
    }
}

/// As its printed form.
impl Serialize for WorkspaceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceId;

    const UUID: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b";

    #[test]
    fn ids_print_and_parse_as_uuids() {
        let id: WorkspaceId = UUID.parse().unwrap();
        assert_eq!(id.to_bytes()[6] >> 4, 7);
        assert_eq!(id.to_string(), UUID);
        assert_eq!(UUID.to_uppercase().parse::<WorkspaceId>(), Ok(id));
        assert_eq!(UUID.replace('-', "").parse::<WorkspaceId>(), Ok(id));
        for bad in ["", "7", "x", "01920a6b7c3d-7e4f-8a1b-2c3d4e5f6a7b-"] {
            assert!(bad.parse::<WorkspaceId>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{UUID}\""));
        assert_eq!(serde_json::from_str::<WorkspaceId>(&format!("\"{UUID}\"")).unwrap(), id);
        assert!(serde_json::from_str::<WorkspaceId>("7").is_err());
    }
}
