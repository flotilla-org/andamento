//! What the sidebar holds, by kind: its strings, how many are copies of one
//! another, and its string-keyed maps, by shape (keyset). Each `Rc`
//! allocation is counted once, however many hold it.
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::rc::Rc;
use std::sync::Arc;

use crate::{EntityRef, MetadataValue, NodeKey, PlacementKey};

#[derive(Default)]
pub(crate) struct Census {
    seen: HashSet<usize>,
    strings: BTreeMap<String, Strings>,
    maps: BTreeMap<String, Maps>,
    blobs: BTreeMap<String, (usize, usize)>,
}

#[derive(Default)]
struct Strings {
    count: usize,
    bytes: usize,
    distinct: HashMap<String, usize>,
}

#[derive(Default)]
struct Maps {
    count: usize,
    entries: usize,
    shapes: HashMap<Vec<String>, usize>,
}

/// A String's own heap allocation is its contents; its header (24 bytes)
/// lives in whatever holds it.
const STRING_HEADER: usize = 24;

impl Census {
    /// Whether `rc`'s allocation is seen for the first time (count it then).
    pub(crate) fn first<T: ?Sized>(&mut self, rc: &Rc<T>) -> bool {
        self.seen.insert(Rc::as_ptr(rc) as *const () as usize)
    }

    pub(crate) fn first_shared<T: ?Sized>(&mut self, arc: &Arc<T>) -> bool {
        self.seen.insert(Arc::as_ptr(arc) as *const () as usize)
    }

    pub(crate) fn string(&mut self, kind: &str, value: &str) {
        let strings = self.strings.entry(kind.to_owned()).or_default();
        strings.count += 1;
        strings.bytes += value.len();
        *strings.distinct.entry(value.to_owned()).or_default() += 1;
    }

    /// A string-keyed map: its shape, and each key as a string of `kind`.
    pub(crate) fn map(&mut self, kind: &str, keys_kind: &str, keys: impl IntoIterator<Item = impl AsRef<str>>) {
        let keys: Vec<String> = keys.into_iter().map(|key| key.as_ref().to_owned()).collect();
        for key in &keys {
            self.string(keys_kind, key);
        }
        self.shape(kind, keys);
    }

    /// A map's shape alone, its keys counted elsewhere (shared ones, once).
    pub(crate) fn shape(&mut self, kind: &str, keys: impl IntoIterator<Item = impl AsRef<str>>) {
        let keys: Vec<String> = keys.into_iter().map(|key| key.as_ref().to_owned()).collect();
        let maps = self.maps.entry(kind.to_owned()).or_default();
        maps.count += 1;
        maps.entries += keys.len();
        let mut shape = keys;
        shape.sort();
        *maps.shapes.entry(shape).or_default() += 1;
    }

    /// Something counted only by its size, such as parsed template data.
    pub(crate) fn blob(&mut self, kind: &str, bytes: usize) {
        let blob = self.blobs.entry(kind.to_owned()).or_default();
        blob.0 += 1;
        blob.1 += bytes;
    }

    pub(crate) fn entity(&mut self, kind: &str, entity: &EntityRef) {
        if !self.first_shared(&entity.0) {
            return;
        }
        self.string(kind, &entity.provider);
        self.string(kind, &entity.kind);
        self.string(kind, &entity.id);
    }

    pub(crate) fn key(&mut self, kind: &str, key: &PlacementKey) {
        for segment in &key.0 {
            self.string(kind, &segment.loop_name);
            self.entity(kind, &segment.entity);
        }
    }

    pub(crate) fn node_key(&mut self, kind: &str, key: &NodeKey) {
        match key {
            NodeKey::Entity(entity) => self.entity(kind, entity),
            NodeKey::Placement(key) => self.key(kind, key),
            NodeKey::Root | NodeKey::Tab(_) => {}
        }
    }

    pub(crate) fn value(&mut self, kind: &str, value: &MetadataValue) {
        match value {
            MetadataValue::Text(text) => {
                if self.first_shared(text) {
                    self.string(kind, text)
                }
            }
            MetadataValue::StringList(list) => list.iter().for_each(|text| self.string(kind, text)),
            MetadataValue::EntityRefs(refs) => refs.iter().for_each(|entity| self.entity(kind, entity)),
            MetadataValue::GroupPath(path) => {
                for segment in path {
                    self.string(kind, &segment.key);
                    if let Some(label) = &segment.label {
                        self.string(kind, label);
                    }
                }
            }
            MetadataValue::Bool(_) | MetadataValue::Integer(_) => {}
        }
    }

    pub(crate) fn report(&self) -> String {
        let mut out = String::new();
        let mb = |bytes: usize| bytes as f64 / 1e6;
        let (mut all, mut all_bytes) = (0, 0);
        let mut everywhere: HashMap<&str, usize> = HashMap::new();
        writeln!(out, "strings: kind | count | MB (contents) | distinct | distinct MB | copies per distinct").unwrap();
        let mut kinds: Vec<_> = self.strings.iter().collect();
        kinds.sort_by_key(|(_, strings)| Reverse(strings.bytes + STRING_HEADER * strings.count));
        for (kind, strings) in kinds {
            let distinct_bytes: usize = strings.distinct.keys().map(String::len).sum();
            writeln!(
                out,
                "  {kind:34} {:>9} {:>8.2} {:>9} {:>8.2} {:>8.1}",
                strings.count,
                mb(strings.bytes),
                strings.distinct.len(),
                mb(distinct_bytes),
                strings.count as f64 / strings.distinct.len().max(1) as f64
            )
            .unwrap();
            all += strings.count;
            all_bytes += strings.bytes;
            for (value, count) in &strings.distinct {
                *everywhere.entry(value).or_default() += count;
            }
        }
        let distinct_bytes: usize = everywhere.keys().map(|value| value.len()).sum();
        writeln!(
            out,
            "  all: {all} strings, {:.2} MB of contents plus {:.2} MB of headers; {} distinct, {:.2} MB",
            mb(all_bytes),
            mb(all * STRING_HEADER),
            everywhere.len(),
            mb(distinct_bytes)
        )
        .unwrap();
        let mut top: Vec<_> = everywhere.iter().collect();
        top.sort_by_key(|(value, count)| Reverse(**count * (value.len() + STRING_HEADER)));
        writeln!(out, "  most held (count x size):").unwrap();
        for (value, count) in top.iter().take(12) {
            let shown: String = value.chars().take(48).collect();
            writeln!(out, "    {count:>8} x {shown:?}").unwrap();
        }
        writeln!(out, "maps: kind | count | mean entries | distinct shapes | maps in the 5 commonest shapes").unwrap();
        for (kind, maps) in &self.maps {
            let mut shapes: Vec<_> = maps.shapes.iter().collect();
            shapes.sort_by_key(|(_, count)| Reverse(**count));
            let commonest: usize = shapes.iter().take(5).map(|(_, count)| **count).sum();
            writeln!(
                out,
                "  {kind:34} {:>9} {:>8.1} {:>9} {:>8.0}%",
                maps.count,
                maps.entries as f64 / maps.count.max(1) as f64,
                maps.shapes.len(),
                100.0 * commonest as f64 / maps.count.max(1) as f64
            )
            .unwrap();
            for (shape, count) in shapes.iter().take(3) {
                writeln!(out, "      {count:>7} x {} keys: {}", shape.len(), shape.iter().take(6).cloned().collect::<Vec<_>>().join(", "))
                    .unwrap();
            }
        }
        if !self.blobs.is_empty() {
            writeln!(out, "other: kind | count | MB (serialized size)").unwrap();
            for (kind, (count, bytes)) in &self.blobs {
                writeln!(out, "  {kind:34} {count:>9} {:>8.2}", mb(*bytes)).unwrap();
            }
        }
        out
    }
}
