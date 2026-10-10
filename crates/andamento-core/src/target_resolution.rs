//! Portable Target Resolutions saved with a workspace (Wheelhouse
//! `CONTEXT.md`, Target Resolution; andamento#158).
//!
//! Resolving a slot's Target Reference yields a Target Resolution: a way of
//! connecting, such as a Cleat session on a host. A **portable** resolution
//! works from any device that can reach it, so it is saved in the
//! `workspace/<id>` record; a machine-local one (a local socket, an attach
//! token) stays with the device. The host decides which is which and
//! supplies the portable ones as an opaque typed value: a kind and named
//! text fields. Andamento never interprets them.
//!
//! A saved resolution is a cache. It records the identity of the slot
//! resolution it was made for (`against`, a [`Resolved::id`]); once the
//! slot resolves to something else, it is cleared (see
//! [`SavedResolutions::retain`]).
//!
//! [`Resolved::id`]: crate::managed::Resolved::id
use std::collections::BTreeMap;

/// A portable Target Resolution as the host supplies it, for example kind
/// `cleat-session` with fields `host=feta session=S daemon=D`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortableResolution {
    /// The host's name for what kind of resolution this is.
    pub kind: String,
    /// Named text fields, kept in name order.
    pub fields: BTreeMap<String, String>,
}

impl PortableResolution {
    /// A kind and field names are 1 to 64 of `[a-z0-9_-]`, starting with a
    /// letter or digit; field values are any text.
    pub fn check(&self) -> Result<(), String> {
        crate::slots::check_key(&self.kind)
            .map_err(|_| format!("invalid resolution kind {:?}", self.kind))?;
        for name in self.fields.keys() {
            crate::slots::check_key(name)
                .map_err(|_| format!("invalid resolution field name {name:?}"))?;
        }
        Ok(())
    }
}

/// One slot's saved portable resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedResolution {
    pub resolution: PortableResolution,
    /// The identity of the slot resolution this resolves: what the host
    /// applied, as it passes it to plan.
    pub against: String,
    /// Nonzero; never reused within the workspace.
    pub generation: u64,
}

/// Why a save or clear changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionError {
    /// The expected generation is not the slot's current one (0 for none).
    Stale {
        current: u64,
    },
    Invalid(String),
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale { current } => write!(
                f,
                "the saved resolution changed: its generation is {current}; read it again"
            ),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

/// A workspace's saved portable resolutions, by slot key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedResolutions {
    pub by_slot: BTreeMap<String, SavedResolution>,
    /// The last generation given out, so a clear never lets a generation be
    /// reused.
    pub last_generation: u64,
}

impl SavedResolutions {
    pub fn is_empty(&self) -> bool {
        self.by_slot.is_empty() && self.last_generation == 0
    }

    pub fn get(&self, key: &str) -> Option<&SavedResolution> {
        self.by_slot.get(key)
    }

    /// The slot's saved generation; 0 when none is saved.
    pub fn generation(&self, key: &str) -> u64 {
        self.get(key).map_or(0, |saved| saved.generation)
    }

    fn check_expected(&self, key: &str, expected: u64) -> Result<(), ResolutionError> {
        let current = self.generation(key);
        if current != expected {
            return Err(ResolutionError::Stale { current });
        }
        Ok(())
    }

    /// Save `resolution` for `key`, made for the slot resolution `against`,
    /// if `expected` is the slot's current generation. Saving what is
    /// already saved keeps its generation. Returns the generation after.
    pub fn set(
        &mut self,
        key: &str,
        resolution: PortableResolution,
        against: &str,
        expected: u64,
    ) -> Result<u64, ResolutionError> {
        self.check_expected(key, expected)?;
        resolution.check().map_err(ResolutionError::Invalid)?;
        if against.is_empty() {
            return Err(ResolutionError::Invalid(
                "a saved resolution names the slot resolution it resolves".into(),
            ));
        }
        if let Some(saved) = self.by_slot.get(key) {
            if saved.resolution == resolution && saved.against == against {
                return Ok(saved.generation);
            }
        }
        self.last_generation += 1;
        self.by_slot.insert(
            key.to_owned(),
            SavedResolution {
                resolution,
                against: against.to_owned(),
                generation: self.last_generation,
            },
        );
        Ok(self.last_generation)
    }

    /// Clear `key`'s saved resolution if `expected` is its current
    /// generation. Returns whether there was one.
    pub fn clear(&mut self, key: &str, expected: u64) -> Result<bool, ResolutionError> {
        self.check_expected(key, expected)?;
        Ok(self.by_slot.remove(key).is_some())
    }

    /// Drop the slot's saved resolution, whatever its generation. Returns
    /// whether there was one.
    pub fn forget(&mut self, key: &str) -> bool {
        self.by_slot.remove(key).is_some()
    }

    /// Keep only the saved resolutions `keep` accepts. Returns whether any
    /// were dropped.
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &SavedResolution) -> bool) -> bool {
        let before = self.by_slot.len();
        self.by_slot.retain(|key, saved| keep(key, saved));
        self.by_slot.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleat(session: &str) -> PortableResolution {
        PortableResolution {
            kind: "cleat-session".into(),
            fields: [("host", "feta"), ("session", session), ("daemon", "D")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        }
    }

    #[test]
    fn saves_compare_generations_and_never_reuse_one() {
        let mut saved = SavedResolutions::default();
        assert_eq!(saved.set("a", cleat("S"), "id1", 0), Ok(1));
        // Saving the same again keeps the generation.
        assert_eq!(saved.set("a", cleat("S"), "id1", 1), Ok(1));
        assert_eq!(
            saved.set("a", cleat("T"), "id1", 0),
            Err(ResolutionError::Stale { current: 1 })
        );
        assert_eq!(saved.clear("a", 1), Ok(true));
        assert_eq!(saved.generation("a"), 0);
        // A cleared generation is not given out again.
        assert_eq!(saved.set("a", cleat("T"), "id1", 0), Ok(2));
        assert_eq!(saved.clear("b", 0), Ok(false));
    }

    #[test]
    fn kinds_field_names_and_against_are_checked() {
        let mut saved = SavedResolutions::default();
        let mut bad = cleat("S");
        bad.kind = "Cleat Session".into();
        assert!(matches!(
            saved.set("a", bad, "id", 0),
            Err(ResolutionError::Invalid(_))
        ));
        let mut bad = cleat("S");
        bad.fields.insert(String::new(), "x".into());
        assert!(matches!(
            saved.set("a", bad, "id", 0),
            Err(ResolutionError::Invalid(_))
        ));
        assert!(matches!(
            saved.set("a", cleat("S"), "", 0),
            Err(ResolutionError::Invalid(_))
        ));
        assert!(saved.is_empty());
    }
}
