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

use crate::slots;
use crate::versioned::CommitError;

/// A portable Target Resolution as the host supplies it, for example kind
/// `cleat-session` with fields `host=feta session=S daemon=D`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        slots::check_key(&self.kind)
            .map_err(|_| format!("invalid resolution kind {:?}", self.kind))?;
        for name in self.fields.keys() {
            slots::check_key(name)
                .map_err(|_| format!("invalid resolution field name {name:?}"))?;
        }
        Ok(())
    }
}

/// One slot's saved portable resolution.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedResolution {
    pub resolution: PortableResolution,
    /// The identity of the slot resolution this resolves: what the host
    /// applied, as it passes it to plan.
    pub against: String,
    /// The generation it was saved at: from the sidebar's clock, so never
    /// reused. Not persisted: an imported record's are new.
    #[serde(skip)]
    pub generation: u64,
}

/// A workspace's saved portable resolutions, by slot key.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SavedResolutions {
    pub by_slot: BTreeMap<String, SavedResolution>,
}

impl SavedResolutions {
    pub fn get(&self, key: &str) -> Option<&SavedResolution> {
        self.by_slot.get(key)
    }

    /// The slot's saved generation; 0 when none is saved.
    pub fn generation(&self, key: &str) -> u64 {
        self.get(key).map_or(0, |saved| saved.generation)
    }

    fn check_expected(&self, key: &str, expected: u64) -> Result<(), CommitError> {
        let current = self.generation(key);
        if current != expected {
            return Err(CommitError::Stale { current });
        }
        Ok(())
    }

    /// Save `resolution` for `key`, made for the slot resolution `against`,
    /// if `expected` is the slot's current generation; `stamp` is the
    /// generation the change takes. Saving what is already saved keeps its
    /// generation. Returns the generation after.
    pub fn set(
        &mut self,
        key: &str,
        resolution: PortableResolution,
        against: &str,
        expected: u64,
        stamp: u64,
    ) -> Result<u64, CommitError> {
        self.check_expected(key, expected)?;
        resolution.check()?;
        if against.is_empty() {
            return Err("a saved resolution names the slot resolution it resolves".into());
        }
        if let Some(saved) = self.by_slot.get(key) {
            if saved.resolution == resolution && saved.against == against {
                return Ok(saved.generation);
            }
        }
        self.by_slot.insert(
            key.to_owned(),
            SavedResolution {
                resolution,
                against: against.to_owned(),
                generation: stamp,
            },
        );
        Ok(stamp)
    }

    /// Clear `key`'s saved resolution if `expected` is its current
    /// generation. Returns whether there was one.
    pub fn clear(&mut self, key: &str, expected: u64) -> Result<bool, CommitError> {
        self.check_expected(key, expected)?;
        Ok(self.by_slot.remove(key).is_some())
    }

    /// Drop the slot's saved resolution, whatever its generation.
    pub fn forget(&mut self, key: &str) {
        self.by_slot.remove(key);
    }

    /// Keep only the saved resolutions `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &SavedResolution) -> bool) {
        self.by_slot.retain(|key, saved| keep(key, saved));
    }

    /// Give every saved resolution generation `generation`: an imported
    /// record's, which no earlier state had.
    pub fn restamp(&mut self, generation: u64) {
        for saved in self.by_slot.values_mut() {
            saved.generation = generation;
        }
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
    fn saves_compare_generations() {
        let mut saved = SavedResolutions::default();
        assert_eq!(saved.set("a", cleat("S"), "id1", 0, 5), Ok(5));
        // Saving the same again keeps the generation.
        assert_eq!(saved.set("a", cleat("S"), "id1", 5, 6), Ok(5));
        assert_eq!(
            saved.set("a", cleat("T"), "id1", 0, 7),
            Err(CommitError::Stale { current: 5 })
        );
        assert_eq!(saved.clear("a", 5), Ok(true));
        assert_eq!(saved.generation("a"), 0);
        assert_eq!(saved.clear("b", 0), Ok(false));
    }

    #[test]
    fn kinds_field_names_and_against_are_checked() {
        let mut saved = SavedResolutions::default();
        let mut bad_kind = cleat("S");
        bad_kind.kind = "Cleat Session".into();
        let mut bad_field = cleat("S");
        bad_field.fields.insert(String::new(), "x".into());
        for (resolution, against, reason) in [
            (bad_kind, "id", "invalid resolution kind"),
            (bad_field, "id", "invalid resolution field name"),
            (cleat("S"), "", "names the slot resolution"),
        ] {
            match saved.set("a", resolution, against, 0, 1) {
                Err(CommitError::Invalid(error)) => assert!(error.contains(reason), "{error}"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(saved, SavedResolutions::default());
    }
}
