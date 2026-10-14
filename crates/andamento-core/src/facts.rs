//! An entity's resolved facts as a record: a spine of key names, shared by
//! every record with the same keys, and the facts in the spine's order.
//! Entities of a kind publish the same keys, so a catalog has a handful of
//! spines however many entities it holds.
use crate::{FastHashMap, FastHashSet};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::MetadataEntry;

/// The sorted key names of a record.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    keys: Box<[Rc<str>]>,
}

#[cfg(test)]
impl Shape {
    pub(crate) fn keys(&self) -> &[Rc<str>] {
        &self.keys
    }
}

/// One resolved fact: its winning entry, and the source it came from (none
/// for a fact Andamento made itself).
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// Shared with the store when it is a producer's entry as held.
    pub entry: Rc<MetadataEntry>,
    pub source: Option<Rc<str>>,
}

/// An entity's facts resolved: each key's winning entry, with its source.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFacts {
    shape: Rc<Shape>,
    facts: Vec<Fact>,
}

impl Default for ResolvedFacts {
    fn default() -> Self {
        Self { shape: Rc::new(Shape { keys: Box::new([]) }), facts: vec![] }
    }
}

/// Key names and spines, each made once: records with the same keys share
/// one spine, and spines share their key names.
#[derive(Debug, Default)]
pub struct Shapes {
    keys: FastHashSet<Rc<str>>,
    shapes: FastHashMap<Box<[Rc<str>]>, Rc<Shape>>,
}

impl Shapes {
    /// The one copy of `key`.
    pub fn key(&mut self, key: &str) -> Rc<str> {
        if let Some(key) = self.keys.get(key) {
            return key.clone();
        }
        let key: Rc<str> = Rc::from(key);
        self.keys.insert(key.clone());
        key
    }

    /// A record of `facts`, by key name in any order, on a shared spine.
    pub fn record(&mut self, mut facts: Vec<(Rc<str>, Fact)>) -> ResolvedFacts {
        facts.sort_by(|(left, _), (right, _)| left.cmp(right));
        let keys: Vec<Rc<str>> = facts.iter().map(|(key, _)| key.clone()).collect();
        let shape = match self.shapes.get(keys.as_slice()) {
            Some(shape) => shape.clone(),
            None => {
                let shape = Rc::new(Shape { keys: keys.clone().into_boxed_slice() });
                self.shapes.insert(keys.into_boxed_slice(), shape.clone());
                shape
            }
        };
        ResolvedFacts { shape, facts: facts.into_iter().map(|(_, fact)| fact).collect() }
    }
}

impl ResolvedFacts {
    /// A record of entries and their sources, on a spine of its own.
    pub fn from_maps(values: BTreeMap<String, MetadataEntry>, sources: &BTreeMap<String, String>) -> Self {
        let mut keys = Vec::with_capacity(values.len());
        let mut facts = Vec::with_capacity(values.len());
        for (key, entry) in values {
            facts.push(Fact { entry: Rc::new(entry), source: sources.get(&key).map(|source| Rc::from(source.as_str())) });
            keys.push(Rc::from(key));
        }
        Self { shape: Rc::new(Shape { keys: keys.into_boxed_slice() }), facts }
    }

    fn position(&self, key: &str) -> Result<usize, usize> {
        self.shape.keys.binary_search_by(|probe| (**probe).cmp(key))
    }

    #[cfg(test)]
    pub(crate) fn shape(&self) -> &Rc<Shape> {
        &self.shape
    }

    pub fn get(&self, key: &str) -> Option<&MetadataEntry> {
        self.fact(key).map(|fact| &*fact.entry)
    }

    pub fn fact(&self, key: &str) -> Option<&Fact> {
        self.position(key).ok().map(|at| &self.facts[at])
    }

    pub fn source(&self, key: &str) -> Option<&str> {
        self.fact(key).and_then(|fact| fact.source.as_deref())
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_ok()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Entries by key, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &MetadataEntry)> {
        self.facts().map(|(key, fact)| (key, &*fact.entry))
    }

    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.shape.keys.iter().map(|key| &**key)
    }

    /// Facts by key, in key order.
    pub fn facts(&self) -> impl Iterator<Item = (&str, &Fact)> {
        self.shape.keys.iter().map(|key| &**key).zip(&self.facts)
    }

    /// Facts by their shared key names, in key order.
    pub fn named(&self) -> impl Iterator<Item = (&Rc<str>, &Fact)> {
        self.shape.keys.iter().zip(&self.facts)
    }

    /// Entries as a map, for what reads one.
    pub fn to_values(&self) -> BTreeMap<String, MetadataEntry> {
        self.iter().map(|(key, entry)| (key.to_owned(), entry.clone())).collect()
    }

    /// Set `key`'s fact. A new key gives this record a spine of its own.
    pub fn insert(&mut self, key: &str, fact: Fact) {
        match self.position(key) {
            Ok(at) => self.facts[at] = fact,
            Err(at) => {
                let mut keys = self.shape.keys.to_vec();
                keys.insert(at, Rc::from(key));
                self.shape = Rc::new(Shape { keys: keys.into_boxed_slice() });
                self.facts.insert(at, fact);
            }
        }
    }

    /// Keep only the facts `keep` accepts, on a spine of its own if any go.
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &Fact) -> bool) {
        let kept: Vec<bool> = self.facts().map(|(key, fact)| keep(key, fact)).collect();
        if kept.iter().all(|kept| *kept) {
            return;
        }
        let keys = self.shape.keys.iter().zip(&kept).filter(|(_, kept)| **kept).map(|(key, _)| key.clone()).collect();
        self.shape = Rc::new(Shape { keys });
        let mut kept = kept.into_iter();
        self.facts.retain(|_| kept.next().unwrap_or(false));
    }

    pub fn remove(&mut self, key: &str) -> Option<Fact> {
        let mut removed = None;
        self.retain(|candidate, fact| {
            if candidate == key {
                removed = Some(fact.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    /// This record's facts over `base`'s: each key's from this record if it
    /// has one, else from `base`.
    pub fn over(&self, base: &ResolvedFacts) -> ResolvedFacts {
        let (mut keys, mut facts) = (vec![], vec![]);
        let mut top = self.shape.keys.iter().zip(&self.facts).peekable();
        let mut bottom = base.shape.keys.iter().zip(&base.facts).peekable();
        loop {
            let from_top = match (top.peek(), bottom.peek()) {
                (Some((top_key, _)), Some((bottom_key, _))) => match top_key.cmp(bottom_key) {
                    Ordering::Less => true,
                    Ordering::Greater => false,
                    Ordering::Equal => {
                        bottom.next();
                        true
                    }
                },
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            let (key, fact) = if from_top { top.next() } else { bottom.next() }.expect("peeked");
            keys.push(key.clone());
            facts.push(fact.clone());
        }
        let shape = if *keys == *self.shape.keys {
            self.shape.clone()
        } else if *keys == *base.shape.keys {
            base.shape.clone()
        } else {
            Rc::new(Shape { keys: keys.into_boxed_slice() })
        };
        ResolvedFacts { shape, facts }
    }
}

/// Entries read by key: a resolved record, or a plain map of them.
pub trait Entries {
    fn entry_at(&self, key: &str) -> Option<&MetadataEntry>;
}

impl Entries for ResolvedFacts {
    fn entry_at(&self, key: &str) -> Option<&MetadataEntry> {
        self.get(key)
    }
}

impl Entries for BTreeMap<String, MetadataEntry> {
    fn entry_at(&self, key: &str) -> Option<&MetadataEntry> {
        self.get(key)
    }
}

impl<T: Entries> Entries for Rc<T> {
    fn entry_at(&self, key: &str) -> Option<&MetadataEntry> {
        (**self).entry_at(key)
    }
}

impl<T: Entries + ?Sized> Entries for &T {
    fn entry_at(&self, key: &str) -> Option<&MetadataEntry> {
        (**self).entry_at(key)
    }
}
