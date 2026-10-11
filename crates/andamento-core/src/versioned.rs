//! Versioned documents: one compare-and-set, numbered by one clock.
//!
//! A host reads a document with its generation and commits a whole new one
//! at the generation it read. Every generation comes from the sidebar's one
//! [`Clock`], so no two states of any document share a number: not a
//! document and the one an import replaced it with, nor two documents that
//! happen to change the same number of times. A generation is runtime state:
//! it is never persisted, and a host must not keep one across sessions.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::slots::ArrangementError;

/// The sidebar's generation counter. It only moves forward.
#[derive(Clone, Debug, Default)]
pub struct Clock(u64);

impl Clock {
    pub fn tick(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

/// A value and the generation it last changed at; 0 until it first does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Versioned<T> {
    generation: u64,
    value: T,
}

impl<T: Clone + PartialEq> Versioned<T> {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    /// Edit a copy if `expected` (when given) is the current generation, and
    /// keep it, at a new generation, only if it differs. A stale or failed
    /// edit changes nothing. Returns the generation after, and the edit's
    /// result.
    pub fn update<R>(
        &mut self,
        expected: Option<u64>,
        clock: &mut Clock,
        edit: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<(u64, R), ArrangementError> {
        if expected.is_some_and(|expected| expected != self.generation) {
            return Err(ArrangementError::Stale {
                current: self.generation,
            });
        }
        let mut next = self.value.clone();
        let out = edit(&mut next).map_err(ArrangementError::Invalid)?;
        if next != self.value {
            self.value = next;
            self.generation = clock.tick();
        }
        Ok((self.generation, out))
    }

    /// An edit no host commits (a provider's change): never stale, never
    /// invalid. Returns whether the value changed.
    pub fn follow(&mut self, clock: &mut Clock, edit: impl FnOnce(&mut T)) -> bool {
        let before = self.generation;
        let _ = self.update(None, clock, |value| {
            edit(value);
            Ok(())
        });
        self.generation != before
    }

    /// Give a value read from a record a generation no earlier state had.
    pub fn restamp(&mut self, clock: &mut Clock) {
        self.generation = clock.tick();
    }
}

/// A value read from a record: no generation yet.
impl<T> From<T> for Versioned<T> {
    fn from(value: T) -> Self {
        Self {
            generation: 0,
            value,
        }
    }
}

/// Only the value is persisted.
impl<T: Serialize> Serialize for Versioned<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Versioned<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::from)
    }
}
