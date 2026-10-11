//! Versioned documents: one compare-and-set, numbered by one clock.
//!
//! A host reads a document with its generation and commits a whole new one
//! at the generation it read. Every generation comes from the sidebar's one
//! [`Clock`], so no two states of any document share a number: not a
//! document and the one an import replaced it with, nor two documents that
//! happen to change the same number of times. A generation is runtime state:
//! it is never persisted, and a host must not keep one across sessions.
//!
//! A [`Versioned`] value can only change through [`Versioned::update`] (or
//! [`follow`](Versioned::follow), which calls it), so it can't change
//! without moving its generation.
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The sidebar's generation counter. It only moves forward.
#[derive(Clone, Debug, Default)]
pub struct Clock(u64);

impl Clock {
    pub fn tick(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    /// The generation the next change will have.
    pub fn next(&self) -> u64 {
        self.0 + 1
    }
}

/// Why a compare-and-set changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    /// The expected generation is not the current one.
    Stale {
        current: u64,
    },
    Invalid(String),
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale { current } => {
                write!(f, "it changed: its generation is {current}; read it again")
            }
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl From<String> for CommitError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

impl From<&str> for CommitError {
    fn from(message: &str) -> Self {
        Self::Invalid(message.to_owned())
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
        edit: impl FnOnce(&mut T) -> Result<R, CommitError>,
    ) -> Result<(u64, R), CommitError> {
        if expected.is_some_and(|expected| expected != self.generation) {
            return Err(CommitError::Stale {
                current: self.generation,
            });
        }
        let mut next = self.value.clone();
        let out = edit(&mut next)?;
        if next != self.value {
            self.value = next;
            self.generation = clock.tick();
        }
        Ok((self.generation, out))
    }

    /// An edit nothing compares: [`update`](Self::update) with no expected
    /// generation.
    pub fn edit<R>(
        &mut self,
        clock: &mut Clock,
        edit: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<R, String> {
        self.update(None, clock, |value| Ok(edit(value)?))
            .map(|(_, out)| out)
            .map_err(|error| error.to_string())
    }

    /// An edit nothing compares or rejects, such as following a provider's
    /// change. Returns whether the value changed.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An edit that sets the value to `n`.
    fn set(n: i32) -> impl FnOnce(&mut i32) -> Result<(), CommitError> {
        move |value| {
            *value = n;
            Ok(())
        }
    }

    #[test]
    fn updates_compare_and_move_only_on_change() {
        let mut clock = Clock::default();
        let (mut a, mut b) = (Versioned::from(0), Versioned::from(0));
        assert_eq!(a.update(Some(0), &mut clock, set(1)), Ok((1, ())));
        // Another document's change takes the next generation, not its own 1.
        assert_eq!(b.update(None, &mut clock, set(1)), Ok((2, ())));
        assert_eq!(
            a.update(Some(0), &mut clock, set(2)),
            Err(CommitError::Stale { current: 1 })
        );
        // The same value keeps its generation; a failed edit changes nothing.
        assert_eq!(a.update(Some(1), &mut clock, set(1)), Ok((1, ())));
        let failed: Result<(u64, ()), _> = a.update(Some(1), &mut clock, |value| {
            *value = 3;
            Err("no".into())
        });
        assert_eq!(failed, Err(CommitError::Invalid("no".into())));
        assert_eq!((a.generation(), *a.get()), (1, 1));
        assert!(a.follow(&mut clock, |value| *value = 4));
        assert!(!a.follow(&mut clock, |value| *value = 4));
        assert_eq!(a.generation(), 3);
    }
}
