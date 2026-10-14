#[cfg(test)]
use crate::census::Census;
use crate::facts::Shapes;
use crate::{FastHashMap, FastHashSet};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::Arc;

use crate::{
    EntityRef, MetadataEntry, MetadataPatch, MetadataSourceEntry, MetadataTarget, MetadataValue, MetadataValueUpdate,
    ResolvedMetadataTarget,
};

pub type EntityId = ResolvedMetadataTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEntry {
    pub source_id: String,
    pub entry: MetadataEntry,
}

impl CandidateEntry {
    pub fn new(source_id: impl Into<String>, entry: MetadataEntry) -> Self {
        Self { source_id: source_id.into(), entry }
    }
}

/// An interned provider (subscription) name.
type ProviderIx = u32;

/// Who contributed a fact: the provider whose patch set it, and the
/// producer's source ID. Facts with no provider are the core's or the host's
/// own; neither retraction nor staleness touches them. The same source ID
/// under two providers is two contributors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Contributor {
    provider: Option<ProviderIx>,
    source: SourceIx,
}

/// A producer's source ID, interned by the store.
type SourceIx = u32;

/// A contribution: who made it, and its entry, shared with resolutions.
type Contribution = (Contributor, Rc<MetadataEntry>);

/// One key's contributions, by contributor. Nearly always one, held in
/// place; more spill to a sorted Vec.
#[derive(Debug, Default)]
enum Contributions {
    #[default]
    None,
    One(Contribution),
    Many(Vec<Contribution>),
}

impl Contributions {
    fn as_slice(&self) -> &[Contribution] {
        match self {
            Self::None => &[],
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }

    fn as_mut_slice(&mut self) -> &mut [Contribution] {
        match self {
            Self::None => &mut [],
            Self::One(one) => std::slice::from_mut(one),
            Self::Many(many) => many,
        }
    }

    fn position(&self, contributor: &Contributor) -> Result<usize, usize> {
        self.as_slice().binary_search_by(|(c, _)| c.cmp(contributor))
    }

    fn insert(&mut self, contributor: Contributor, entry: MetadataEntry) -> Option<Rc<MetadataEntry>> {
        let entry = Rc::new(entry);
        match self.position(&contributor) {
            Ok(at) => Some(std::mem::replace(&mut self.as_mut_slice()[at].1, entry)),
            Err(at) => {
                *self = match std::mem::take(self) {
                    Self::None => Self::One((contributor, entry)),
                    Self::One(one) => {
                        let mut many = vec![one];
                        many.insert(at, (contributor, entry));
                        Self::Many(many)
                    }
                    Self::Many(mut many) => {
                        many.insert(at, (contributor, entry));
                        Self::Many(many)
                    }
                };
                None
            }
        }
    }

    fn remove(&mut self, contributor: &Contributor) -> Option<MetadataEntry> {
        let at = self.position(contributor).ok()?;
        let removed = match std::mem::take(self) {
            Self::None => unreachable!("found in none"),
            Self::One(one) => one.1,
            Self::Many(mut many) => {
                let removed = many.remove(at).1;
                *self = match many.len() {
                    0 => Self::None,
                    1 => Self::One(many.pop().expect("one left")),
                    _ => Self::Many(many),
                };
                removed
            }
        };
        Some(Rc::unwrap_or_clone(removed))
    }

    fn get(&self, contributor: &Contributor) -> Option<&MetadataEntry> {
        self.position(contributor).ok().map(|at| &*self.as_slice()[at].1)
    }

    fn contains_key(&self, contributor: &Contributor) -> bool {
        self.position(contributor).is_ok()
    }

    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn iter(&self) -> impl Iterator<Item = (&Contributor, &MetadataEntry)> {
        self.as_slice().iter().map(|(contributor, entry)| (contributor, &**entry))
    }

    /// Entries as held, to share rather than copy.
    fn shared(&self) -> impl Iterator<Item = (&Contributor, &Rc<MetadataEntry>)> {
        self.as_slice().iter().map(|(contributor, entry)| (contributor, entry))
    }

    /// Entries to change: `Rc::make_mut` one to change it, copying it only
    /// if a resolution shares it.
    fn iter_mut(&mut self) -> impl Iterator<Item = (&Contributor, &mut Rc<MetadataEntry>)> {
        self.as_mut_slice().iter_mut().map(|(contributor, entry)| (&*contributor, entry))
    }

    fn retain(&mut self, mut keep: impl FnMut(&Contributor, &MetadataEntry) -> bool) {
        *self = match std::mem::take(self) {
            Self::One(one) if !keep(&one.0, &one.1) => Self::None,
            Self::Many(mut many) => {
                many.retain(|(contributor, entry)| keep(contributor, entry));
                match many.len() {
                    0 => Self::None,
                    1 => Self::One(many.pop().expect("one left")),
                    _ => Self::Many(many),
                }
            }
            kept => kept,
        };
    }
}

/// An entity's contributions by fact name, sorted by name: names are the
/// store's interned ones, and an entity has a few dozen, so a sorted Vec
/// rather than a hash table per entity.
#[derive(Debug, Default)]
struct KeyedContributions(Vec<(Rc<str>, Contributions)>);

impl KeyedContributions {
    fn position(&self, key: &str) -> Result<usize, usize> {
        self.0.binary_search_by(|(name, _)| (**name).cmp(key))
    }

    fn get(&self, key: &str) -> Option<&Contributions> {
        self.position(key).ok().map(|at| &self.0[at].1)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut Contributions> {
        self.position(key).ok().map(|at| &mut self.0[at].1)
    }

    fn entry_or_default(&mut self, key: Rc<str>) -> &mut Contributions {
        let at = match self.position(&key) {
            Ok(at) => at,
            Err(at) => {
                self.0.insert(at, (key, Contributions::None));
                at
            }
        };
        &mut self.0[at].1
    }

    fn remove(&mut self, key: &str) {
        if let Ok(at) = self.position(key) {
            self.0.remove(at);
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&Rc<str>, &mut Contributions) -> bool) {
        self.0.retain_mut(|(key, contributions)| keep(key, contributions));
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &Contributions)> {
        self.0.iter().map(|(key, contributions)| (key, contributions))
    }

    #[cfg(test)]
    fn keys(&self) -> impl Iterator<Item = &Rc<str>> {
        self.0.iter().map(|(key, _)| key)
    }

    #[cfg(test)]
    fn values(&self) -> impl Iterator<Item = &Contributions> {
        self.0.iter().map(|(_, contributions)| contributions)
    }

    /// Give back what growing left spare: entities hold their facts for long.
    fn shrink(&mut self) {
        if self.0.capacity() > self.0.len() {
            self.0.shrink_to_fit();
        }
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut Contributions> {
        self.0.iter_mut().map(|(_, contributions)| contributions)
    }
}

// Entity resolutions (one entity's facts, every key resolved), for tests
// that bound the work a patch costs.
#[cfg(test)]
thread_local! { pub(crate) static ENTITY_RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
fn count_resolution() {
    ENTITY_RESOLUTIONS.with(|count| count.set(count.get() + 1));
}

/// What changed since it was last taken: the targets whose facts changed,
/// or everything (a provider went stale or fresh, or facts expired).
#[derive(Debug, Default)]
pub struct Changed {
    pub all: bool,
    pub targets: FastHashSet<EntityId>,
}

pub use crate::facts::{Fact, ResolvedFacts};

/// A resolution and when it holds: from `at` through `until`, the earliest
/// deadline among the facts live at `at` (none: no TTL fact can expire).
#[derive(Debug)]
struct Cached {
    resolved: Rc<ResolvedFacts>,
    at: u64,
    until: Option<u64>,
}

#[derive(Debug, Default)]
pub struct MetadataStore {
    /// Workspace, pane, root and identity facts, by contributor: several
    /// sources contend for these, and the winner is resolved per key.
    entries: FastHashMap<EntityId, Stored>,
    /// Entities' facts, each a whole record (`Record`).
    records: FastHashMap<EntityId, Record>,
    /// Key names and the spines of resolved records.
    shapes: RefCell<Shapes>,
    /// Text values, one copy of each while a fact holds it (`intern_text`).
    texts: FastHashSet<Arc<str>>,
    texts_swept: usize,
    /// Deadlines of the TTL facts that can expire: every one whose provider
    /// is not stale.
    expiry_counts: BTreeMap<u64, usize>,
    providers: Vec<String>,
    provider_ixs: FastHashMap<String, ProviderIx>,
    sources: Vec<Rc<str>>,
    source_ixs: FastHashMap<Rc<str>, SourceIx>,
    /// Stale providers, with the time each became stale. Their facts are
    /// judged live as of that time, so none expires while it stays stale.
    stale: BTreeMap<ProviderIx, u64>,
    changed: Changed,
    /// The same, for the catalog (`take_catalog_changes`), which reads it
    /// when it evaluates rather than when facts arrive.
    catalog_changed: RefCell<Changed>,
}

/// An entity's facts as one record, which is also their resolution. A
/// provider's entities are its own, and a local entity is Andamento's: each
/// fact is whatever its last writer set, with no lease, precedence or
/// ordinal. Liveness is the feed's: a provider whose connection drops is
/// stale, and one removed is retracted.
#[derive(Debug)]
struct Record {
    provider: Option<ProviderIx>,
    facts: Rc<ResolvedFacts>,
}

/// One entity's facts, by key, and its resolution: made when first read
/// and dropped when its facts change, everything may have changed, or a
/// fact in it expires.
#[derive(Debug, Default)]
struct Stored {
    facts: KeyedContributions,
    resolved: RefCell<Option<Cached>>,
}

impl Deref for Stored {
    type Target = KeyedContributions;
    fn deref(&self) -> &Self::Target {
        &self.facts
    }
}

impl DerefMut for Stored {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.facts
    }
}

impl MetadataStore {
    /// Run `f` with no resolutions kept, leaving those kept as they were.
    #[cfg(test)]
    pub(crate) fn cold<R>(&self, f: impl FnOnce() -> R) -> R {
        let kept: Vec<_> = self.entries.values().map(|stored| stored.resolved.take()).collect();
        let changed = self.catalog_changed.take();
        let result = f();
        for (stored, kept) in self.entries.values().zip(kept) {
            stored.resolved.replace(kept);
        }
        self.catalog_changed.replace(changed);
        result
    }

    /// Hold `value`'s text as the one copy of it. Whenever the copies have
    /// doubled since, forget those no fact (or anything else) holds now.
    fn intern_text(&mut self, value: &mut MetadataValue) {
        let MetadataValue::Text(text) = value else {
            return;
        };
        if let Some(held) = self.texts.get(&**text) {
            *text = held.clone();
            return;
        }
        self.texts.insert(text.clone());
        if self.texts.len() > 2 * self.texts_swept.max(512) {
            self.texts.retain(|text| Arc::strong_count(text) > 1);
            self.texts_swept = self.texts.len();
        }
    }

    fn touch(&mut self, target: &EntityId) {
        if let Some(stored) = self.entries.get_mut(target) {
            *stored.resolved.get_mut() = None;
        }
        for changed in [&mut self.changed, self.catalog_changed.get_mut()] {
            if !changed.all {
                changed.targets.insert(target.clone());
            }
        }
    }

    /// Everything may resolve differently: facts expired, or a provider
    /// went stale or fresh.
    pub fn change_all(&mut self) {
        for stored in self.entries.values_mut() {
            *stored.resolved.get_mut() = None;
        }
        self.changed = Changed { all: true, targets: FastHashSet::default() };
        *self.catalog_changed.get_mut() = Changed { all: true, targets: FastHashSet::default() };
    }

    /// What changed since the catalog last read it.
    pub(crate) fn take_catalog_changes(&self) -> Changed {
        self.catalog_changed.take()
    }

    /// What changed since the last call.
    pub fn take_changed(&mut self) -> Changed {
        std::mem::take(&mut self.changed)
    }

    pub fn targets(&self) -> impl Iterator<Item = &EntityId> {
        self.entries.keys().chain(self.records.keys())
    }

    fn intern(&mut self, provider: &str) -> ProviderIx {
        if let Some(ix) = self.provider_ixs.get(provider) {
            return *ix;
        }
        let ix = ProviderIx::try_from(self.providers.len()).expect("fewer than 2^32 providers");
        self.providers.push(provider.to_owned());
        self.provider_ixs.insert(provider.to_owned(), ix);
        ix
    }

    fn intern_source(&mut self, source: String) -> SourceIx {
        if let Some(ix) = self.source_ixs.get(source.as_str()) {
            return *ix;
        }
        let ix = SourceIx::try_from(self.sources.len()).expect("fewer than 2^32 sources");
        let source: Rc<str> = Rc::from(source);
        self.sources.push(source.clone());
        self.source_ixs.insert(source, ix);
        ix
    }

    fn source(&self, contributor: &Contributor) -> &str {
        &self.sources[contributor.source as usize]
    }

    fn contributor(&mut self, provider: Option<&str>, source: impl Into<String>) -> Contributor {
        Contributor { provider: provider.map(|p| self.intern(p)), source: self.intern_source(source.into()) }
    }

    /// The contributor if its provider is known; an unknown provider has
    /// contributed nothing.
    fn existing_contributor(&self, provider: Option<&str>, source: &str) -> Option<Contributor> {
        Some(Contributor {
            provider: match provider {
                Some(p) => Some(*self.provider_ixs.get(p)?),
                None => None,
            },
            source: *self.source_ixs.get(source)?,
        })
    }

    /// The time `contributor`'s facts are judged live at: now, or when its
    /// provider became stale.
    fn liveness_time(&self, contributor: &Contributor, now: u64) -> u64 {
        contributor.provider.and_then(|p| self.stale.get(&p)).map_or(now, |since| (*since).min(now))
    }

    fn is_live(&self, contributor: &Contributor, entry: &MetadataEntry, now: u64) -> bool {
        entry_is_live(entry, self.liveness_time(contributor, now))
    }

    fn indexed(&self, contributor: &Contributor) -> bool {
        contributor.provider.is_none_or(|p| !self.stale.contains_key(&p))
    }

    /// Set a fact the core or host owns (no provider).
    pub fn set(&mut self, entity_id: EntityId, key: impl Into<String>, source_id: impl Into<String>, entry: MetadataEntry) {
        if matches!(entity_id, EntityId::Entity(_)) {
            let update = MetadataValueUpdate { value: entry.value, ttl_ms: None, precedence: None, ordinal: None };
            self.apply_record_patch(None, entity_id, source_id.into(), BTreeMap::from([(key.into(), update)]), vec![], entry.updated_at);
            return;
        }
        let contributor = self.contributor(None, source_id);
        self.touch(&entity_id);
        self.set_contribution(&entity_id, &key.into(), contributor, entry);
    }

    /// Set one contribution. The caller touches the entity and records its
    /// ordinal, once for all it sets.
    fn set_contribution(&mut self, entity_id: &EntityId, key: &str, contributor: Contributor, entry: MetadataEntry) {
        let indexed = self.indexed(&contributor);
        let deadline = entry.ttl_ms.map(|ttl| entry.updated_at.saturating_add(ttl));
        let key = self.shapes.get_mut().key(key);
        let mut entry = entry;
        self.intern_text(&mut entry.value);
        if !self.entries.contains_key(entity_id) {
            self.entries.insert(entity_id.clone(), Stored::default());
        }
        let stored = self.entries.get_mut(entity_id).expect("just inserted");
        let previous = stored.entry_or_default(key).insert(contributor, entry);
        if indexed {
            if let Some(previous) = previous {
                self.remove_expiry(&previous);
            }
            if let Some(deadline) = deadline {
                *self.expiry_counts.entry(deadline).or_default() += 1;
            }
        }
    }

    /// Unset a fact the core or host owns (no provider).
    pub fn unset(&mut self, entity_id: &EntityId, key: &str, source_id: &str) -> Option<MetadataEntry> {
        if matches!(entity_id, EntityId::Entity(_)) {
            let removed =
                self.records.get(entity_id)?.facts.fact(key).filter(|fact| fact.source.as_deref() == Some(source_id))?.entry.clone();
            self.apply_record_patch(None, entity_id.clone(), source_id.to_owned(), BTreeMap::new(), vec![key.to_owned()], 0);
            return Some(Rc::unwrap_or_clone(removed));
        }
        let contributor = self.existing_contributor(None, source_id)?;
        self.unset_contribution(entity_id, key, &contributor)
    }

    fn unset_contribution(&mut self, entity_id: &EntityId, key: &str, contributor: &Contributor) -> Option<MetadataEntry> {
        let entity_entries = self.entries.get_mut(entity_id)?;
        let key_entries = entity_entries.get_mut(key)?;
        let removed = key_entries.remove(contributor);
        if key_entries.is_empty() {
            entity_entries.remove(key);
        }
        if entity_entries.is_empty() {
            self.entries.remove(entity_id);
        }
        if let Some(entry) = &removed {
            self.touch(entity_id);
            if self.indexed(contributor) {
                self.remove_expiry(entry);
            }
        }
        removed
    }

    fn remove_expiry(&mut self, entry: &MetadataEntry) {
        if let Some(ttl) = entry.ttl_ms {
            let deadline = entry.updated_at.saturating_add(ttl);
            if let Some(count) = self.expiry_counts.get_mut(&deadline) {
                *count -= 1;
                if *count == 0 {
                    self.expiry_counts.remove(&deadline);
                }
            }
        }
    }

    fn add_expiry(&mut self, entry: &MetadataEntry) {
        if let Some(ttl) = entry.ttl_ms {
            *self.expiry_counts.entry(entry.updated_at.saturating_add(ttl)).or_default() += 1;
        }
    }

    /// Entries are live through their deadline. Storage is bounded by the
    /// number of retained contributions; renewing a lease replaces its deadline.
    pub fn expires_between(&self, before: u64, now: u64) -> bool {
        before < now && self.expiry_counts.range(before..now).next().is_some()
    }

    pub(crate) fn source_contributes(&self, entity: &EntityId, key: &str, provider: Option<&str>, source: &str) -> bool {
        if let Some(record) = self.records.get(entity) {
            let owner = record.provider.map(|ix| self.providers[ix as usize].as_str());
            return owner == provider && record.facts.source(key) == Some(source);
        }
        let Some(contributor) = self.existing_contributor(provider, source) else {
            return false;
        };
        self.entries.get(entity).and_then(|keys| keys.get(key)).is_some_and(|sources| sources.contains_key(&contributor))
    }

    // Raw ownership survives lease expiry: silence is not authoritative removal.
    pub(crate) fn has_contributors(&self, entity: &EntityId, key: &str) -> bool {
        if let Some(record) = self.records.get(entity) {
            return record.facts.contains_key(key);
        }
        self.entries.get(entity).and_then(|keys| keys.get(key)).is_some_and(|sources| !sources.is_empty())
    }

    /// Whether any provider is stale.
    pub fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }

    /// Whether `provider` is stale.
    pub fn is_stale(&self, provider: &str) -> bool {
        self.provider_ixs.get(provider).is_some_and(|p| self.stale.contains_key(p))
    }

    /// Mark a provider stale, or fresh again. A stale provider's facts are
    /// kept as they were when it became stale: those live then stay live and
    /// none expires. Fresh again, each TTL fact that was live renews its lease
    /// from `now`, as though the provider had just reasserted it, so a fact
    /// the provider doesn't reassert expires one TTL later. Returns whether
    /// anything changed.
    pub fn set_stale(&mut self, provider: &str, stale: bool, now: u64) -> bool {
        let ix = self.intern(provider);
        if stale == self.stale.contains_key(&ix) {
            return false;
        }
        let since = if stale {
            self.stale.insert(ix, now);
            None
        } else {
            self.stale.remove(&ix)
        };
        self.change_all();
        let mut entries = std::mem::take(&mut self.entries);
        for keys in entries.values_mut() {
            for contributions in keys.values_mut() {
                for (contributor, entry) in contributions.iter_mut() {
                    if contributor.provider != Some(ix) || entry.ttl_ms.is_none() {
                        continue;
                    }
                    match since {
                        None => self.remove_expiry(entry),
                        Some(since) => {
                            if entry_is_live(entry, since) {
                                Rc::make_mut(entry).updated_at = now;
                            }
                            self.add_expiry(entry);
                        }
                    }
                }
            }
        }
        self.entries = entries;
        true
    }

    /// Remove every fact `provider` contributed. Returns whether any was
    /// removed, and the entities that lost facts.
    pub fn retract(&mut self, provider: &str) -> (bool, BTreeSet<EntityRef>) {
        let Some(ix) = self.provider_ixs.get(provider).copied() else {
            return (false, BTreeSet::new());
        };
        let mut removed = false;
        let mut affected = BTreeSet::new();
        let mut entries = std::mem::take(&mut self.entries);
        entries.retain(|target, keys| {
            let mut lost = false;
            keys.retain(|_, contributions| {
                contributions.retain(|contributor, entry| {
                    if contributor.provider != Some(ix) {
                        return true;
                    }
                    if self.indexed(contributor) {
                        self.remove_expiry(entry);
                    }
                    lost = true;
                    false
                });
                !contributions.is_empty()
            });
            removed |= lost;
            if lost {
                self.touch(target);
            }
            if let (true, EntityId::Entity(entity)) = (lost, target) {
                affected.insert(entity.clone());
            }
            !keys.is_empty()
        });
        self.entries = entries;
        let mut records = std::mem::take(&mut self.records);
        records.retain(|target, record| {
            if record.provider != Some(ix) {
                return true;
            }
            removed = true;
            self.touch(target);
            if let EntityId::Entity(entity) = target {
                affected.insert(entity.clone());
            }
            false
        });
        self.records = records;
        self.stale.remove(&ix);
        (removed, affected)
    }

    pub fn source_entry(&self, entity_id: &EntityId, key: &str, source_id: &str, now: u64) -> Option<&MetadataEntry> {
        if let Some(record) = self.records.get(entity_id) {
            return record.facts.fact(key).filter(|fact| fact.source.as_deref() == Some(source_id)).map(|fact| &*fact.entry);
        }
        self.entries
            .get(entity_id)?
            .get(key)?
            .iter()
            .find(|(contributor, entry)| self.source(contributor) == source_id && self.is_live(contributor, entry, now))
            .map(|(_, entry)| entry)
    }

    /// Apply a patch the core or host owns (no provider).
    #[allow(dead_code)]
    pub fn apply_patch(&mut self, patch: MetadataPatch, now: u64) -> MetadataPatchOutcome {
        self.apply_patch_from(None, patch, now)
    }

    /// Apply a patch, contributed by `provider` when it has one.
    pub fn apply_patch_from(&mut self, provider: Option<&str>, patch: MetadataPatch, now: u64) -> MetadataPatchOutcome {
        let target = EntityId::from(patch.target);
        if matches!(target, EntityId::Entity(_)) {
            return self.apply_record_patch(provider, target, patch.source_id, patch.set, patch.unset, now);
        }
        let mut outcome = MetadataPatchOutcome::default();
        let target_existed = self.entries.contains_key(&target);
        let contributor = self.contributor(provider, patch.source_id);
        for key in patch.unset {
            if let Some(entry) = self.unset_contribution(&target, &key, &contributor) {
                outcome.touched = true;
                outcome.view_changed |= self.is_live(&contributor, &entry, now);
            }
        }
        let mut set_any = false;
        for (key, update) in patch.set {
            let next_entry = MetadataEntry {
                value: update.value,
                updated_at: now,
                ttl_ms: update.ttl_ms,
                precedence: update.precedence.unwrap_or_default(),
                ordinal: update.ordinal.unwrap_or_default(),
            };
            // The same payload, still live, needs setting only to renew a TTL.
            let (set, changes) = match self
                .entries
                .get(&target)
                .and_then(|entity_entries| entity_entries.get(key.as_str()))
                .and_then(|source_entries| source_entries.get(&contributor))
            {
                Some(existing) if metadata_entry_payload_matches(existing, &next_entry) && self.is_live(&contributor, existing, now) => {
                    (existing.ttl_ms.is_some(), false)
                }
                _ => (true, true),
            };
            if set {
                set_any = true;
                self.set_contribution(&target, &key, contributor, next_entry);
                outcome.touched = true;
                outcome.view_changed |= changes;
            }
        }
        if set_any {
            self.touch(&target);
            if let Some(stored) = self.entries.get_mut(&target) {
                stored.shrink();
            }
        }
        // Entity identity can itself participate in placement queries even after all
        // its facts expire. Removing the last expired contribution removes it.
        outcome.view_changed |= target_existed != self.entries.contains_key(&target);
        outcome
    }

    /// Apply a patch to an entity's record: each unset removes a key its own
    /// source set, and each set replaces the key's fact (the same value from
    /// the same source changes nothing). Leases, precedence and ordinals are
    /// not an entity's (`check_entity_patch`), and are ignored here.
    fn apply_record_patch(
        &mut self,
        provider: Option<&str>,
        target: EntityId,
        source_id: String,
        set: BTreeMap<String, MetadataValueUpdate>,
        unset: Vec<String>,
        now: u64,
    ) -> MetadataPatchOutcome {
        let provider = provider.map(|provider| self.intern(provider));
        let source = self.intern_source(source_id);
        let source = self.sources[source as usize].clone();
        let mut facts: Vec<(Rc<str>, Fact)> = match self.records.get(&target) {
            Some(record) => record.facts.named().map(|(key, fact)| (key.clone(), fact.clone())).collect(),
            None => vec![],
        };
        let position = |facts: &[(Rc<str>, Fact)], key: &str| facts.binary_search_by(|(name, _)| (**name).cmp(key));
        let mut changed = false;
        for key in &unset {
            if let Ok(at) = position(&facts, key) {
                if facts[at].1.source.as_deref() == Some(&*source) {
                    facts.remove(at);
                    changed = true;
                }
            }
        }
        for (key, update) in set {
            let mut value = update.value;
            self.intern_text(&mut value);
            match position(&facts, &key) {
                Ok(at) if facts[at].1.entry.value == value && facts[at].1.source.as_deref() == Some(&*source) => {}
                found => {
                    let entry = MetadataEntry { value, updated_at: now, ttl_ms: None, precedence: 0, ordinal: 0 };
                    let fact = Fact { entry: Rc::new(entry), source: Some(source.clone()) };
                    match found {
                        Ok(at) => facts[at].1 = fact,
                        Err(at) => facts.insert(at, (self.shapes.get_mut().key(&key), fact)),
                    }
                    changed = true;
                }
            }
        }
        if !changed {
            return MetadataPatchOutcome::default();
        }
        if facts.is_empty() {
            self.records.remove(&target);
        } else {
            let facts = Rc::new(self.shapes.get_mut().record(facts));
            match self.records.get_mut(&target) {
                Some(record) => record.facts = facts,
                None => {
                    self.records.insert(target.clone(), Record { provider, facts });
                }
            }
        }
        self.touch(&target);
        MetadataPatchOutcome { view_changed: true, touched: true }
    }

    pub fn entries_for(&self, entity_id: &EntityId, key: &str, now: u64) -> Vec<CandidateEntry> {
        if let Some(record) = self.records.get(entity_id) {
            return record
                .facts
                .fact(key)
                .map(|fact| CandidateEntry::new(fact.source.as_deref().unwrap_or_default(), (*fact.entry).clone()))
                .into_iter()
                .collect();
        }
        self.entries
            .get(entity_id)
            .and_then(|entity_entries| entity_entries.get(key))
            .map(|source_entries| {
                source_entries
                    .iter()
                    .filter(|(contributor, entry)| self.is_live(contributor, entry, now))
                    .map(|(contributor, entry)| CandidateEntry::new(self.source(contributor), entry.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `entity_id`'s facts resolved at `now`, shared until they change.
    pub fn resolved(&self, entity_id: &EntityId, now: u64) -> Rc<ResolvedFacts> {
        if let Some(record) = self.records.get(entity_id) {
            return record.facts.clone();
        }
        let Some(stored) = self.entries.get(entity_id) else {
            return Rc::default();
        };
        if let Some(cached) = stored.resolved.borrow().as_ref() {
            if cached.at <= now && cached.until.is_none_or(|until| now <= until) {
                return cached.resolved.clone();
            }
        }
        #[cfg(test)]
        count_resolution();
        let mut facts = vec![];
        let mut until: Option<u64> = None;
        for (key, contributions) in stored.iter() {
            let mut shared = Vec::with_capacity(contributions.len());
            for (contributor, entry) in contributions.shared() {
                if !self.is_live(contributor, entry, now) {
                    continue;
                }
                // A stale provider's facts don't expire while it stays stale.
                if let (Some(ttl), true) = (entry.ttl_ms, self.indexed(contributor)) {
                    let deadline = entry.updated_at.saturating_add(ttl);
                    until = Some(until.map_or(deadline, |until| until.min(deadline)));
                }
                shared.push((contributor, entry));
            }
            // Nearly every key has one contributor, and it wins.
            if let [(contributor, entry)] = shared.as_slice() {
                let source = self.sources[contributor.source as usize].clone();
                facts.push((key.clone(), Fact { entry: Rc::clone(entry), source: Some(source) }));
                continue;
            }
            let candidates: Vec<_> =
                shared.iter().map(|(contributor, entry)| CandidateEntry::new(self.source(contributor), (***entry).clone())).collect();
            if let Some(candidate) = select_primary_entry(&candidates) {
                // The store's own entry, shared, when the winner is one as held.
                let held = shared
                    .iter()
                    .find(|(contributor, entry)| self.source(contributor) == candidate.source_id && ***entry == candidate.entry)
                    .map(|(contributor, entry)| (self.sources[contributor.source as usize].clone(), Rc::clone(entry)));
                let (source, entry) = match held {
                    Some((source, entry)) => (Some(source), entry),
                    None => {
                        let source = self.source_ixs.get_key_value(candidate.source_id.as_str()).map(|(source, _)| source.clone());
                        (source, Rc::new(candidate.entry))
                    }
                };
                facts.push((key.clone(), Fact { entry, source }));
            }
        }
        let resolved = Rc::new(self.shapes.borrow_mut().record(facts));
        *stored.resolved.borrow_mut() = Some(Cached { resolved: resolved.clone(), at: now, until });
        resolved
    }

    /// Preserve the winning producer alongside each selected entry in one pass.
    pub fn resolved_entries_for(&self, entity_id: &EntityId, now: u64) -> BTreeMap<String, MetadataEntry> {
        self.resolved(entity_id, now).to_values()
    }

    pub fn source_entries_for(&self, entity_id: &EntityId, now: u64) -> BTreeMap<String, Vec<MetadataSourceEntry>> {
        if let Some(record) = self.records.get(entity_id) {
            return record
                .facts
                .facts()
                .map(|(key, fact)| {
                    let source_id = fact.source.as_deref().unwrap_or_default().to_owned();
                    (key.to_owned(), vec![MetadataSourceEntry { source_id, entry: (*fact.entry).clone() }])
                })
                .collect();
        }
        self.entries
            .get(entity_id)
            .map(|entity_entries| {
                entity_entries
                    .iter()
                    .filter_map(|(key, _)| {
                        let entries = self
                            .entries_for(entity_id, key, now)
                            .into_iter()
                            .map(|candidate| MetadataSourceEntry { source_id: candidate.source_id, entry: candidate.entry })
                            .collect::<Vec<_>>();
                        (!entries.is_empty()).then_some((key.to_string(), entries))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn snapshot_patches(&self, now: u64) -> Vec<MetadataPatch> {
        let mut patches: BTreeMap<(EntityId, String), BTreeMap<String, MetadataValueUpdate>> = BTreeMap::new();
        for (target, record) in &self.records {
            for (key, fact) in record.facts.facts() {
                let source = fact.source.as_deref().unwrap_or_default().to_owned();
                patches.entry((target.clone(), source)).or_default().insert(
                    key.to_owned(),
                    MetadataValueUpdate { value: fact.entry.value.clone(), ttl_ms: None, precedence: None, ordinal: None },
                );
            }
        }
        for (target, target_entries) in &self.entries {
            for (key, source_entries) in target_entries.iter() {
                for (contributor, entry) in source_entries.iter() {
                    if !self.is_live(contributor, entry, now) {
                        continue;
                    }
                    patches.entry((target.clone(), self.source(contributor).to_owned())).or_default().insert(
                        key.to_string(),
                        MetadataValueUpdate {
                            value: entry.value.clone(),
                            ttl_ms: entry.ttl_ms,
                            precedence: Some(entry.precedence),
                            ordinal: Some(entry.ordinal),
                        },
                    );
                }
            }
        }
        patches
            .into_iter()
            .map(|((target, source_id), set)| {
                let target = match target {
                    EntityId::Root => MetadataTarget::Root,
                    EntityId::Pane(pane) => MetadataTarget::Pane(pane),
                    EntityId::Tab(tab) => MetadataTarget::Tab(tab),
                    EntityId::Entity(entity) => MetadataTarget::Entity(entity),
                    EntityId::Identity(identity) => MetadataTarget::Identity(identity),
                };
                MetadataPatch { target, source_id, set, unset: vec![] }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataPatchOutcome {
    pub view_changed: bool,
    pub touched: bool,
}

fn metadata_entry_payload_matches(left: &MetadataEntry, right: &MetadataEntry) -> bool {
    left.value == right.value && left.ttl_ms == right.ttl_ms && left.precedence == right.precedence && left.ordinal == right.ordinal
}

fn entry_is_live(entry: &MetadataEntry, now: u64) -> bool {
    entry.ttl_ms.map(|ttl| now <= entry.updated_at.saturating_add(ttl)).unwrap_or(true)
}

pub fn select_primary_entry(entries: &[CandidateEntry]) -> Option<CandidateEntry> {
    let max_precedence = entries.iter().map(|candidate| candidate.entry.precedence).max()?;
    let mut grouped: BTreeMap<MetadataValue, (usize, i64, String)> = BTreeMap::new();
    for candidate in entries.iter().filter(|candidate| candidate.entry.precedence == max_precedence) {
        let value = candidate.entry.value.clone();
        let grouped_entry = grouped.entry(value).or_insert((0, candidate.entry.ordinal, candidate.source_id.clone()));
        grouped_entry.0 += 1;
        grouped_entry.1 = grouped_entry.1.min(candidate.entry.ordinal);
        if candidate.source_id < grouped_entry.2 {
            grouped_entry.2 = candidate.source_id.clone();
        }
    }
    grouped
        .into_iter()
        .max_by(|(left_value, left_key), (right_value, right_key)| {
            let left = (left_key.0, std::cmp::Reverse(left_key.1), left_value);
            let right = (right_key.0, std::cmp::Reverse(right_key.1), right_value);
            left.cmp(&right)
        })
        .and_then(|(value, _)| {
            entries
                .iter()
                .filter(|candidate| candidate.entry.precedence == max_precedence)
                .filter(|candidate| candidate.entry.value == value)
                .min_by_key(|candidate| (candidate.entry.ordinal, candidate.source_id.as_str(), candidate.entry.updated_at))
                .cloned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_primary_value(entries: &[CandidateEntry]) -> Option<MetadataValue> {
        select_primary_entry(entries).map(|candidate| candidate.entry.value)
    }
    use crate::{tab_workspace_id, EntityRef, MetadataEntry, MetadataPatch, MetadataValue, MetadataValueUpdate, PaneTarget};

    fn entry(value: &str, precedence: i64, ordinal: i64, updated_at: u64) -> MetadataEntry {
        MetadataEntry { value: MetadataValue::Text(value.into()), updated_at, ttl_ms: None, precedence, ordinal }
    }

    /// Equal text held by many facts is one copy; text no fact holds any
    /// more is forgotten, however much has come and gone.
    #[test]
    fn text_values_are_held_once_while_a_fact_holds_them() {
        let mut store = MetadataStore::default();
        let target = |id: &str| EntityId::Entity(EntityRef::new("p", "convoy", id));
        for id in ["a", "b"] {
            store.set(target(id), "status", "source", entry("running", 0, 0, 0));
        }
        let text = |store: &MetadataStore, id: &str| match &store.resolved(&target(id), 0).get("status").unwrap().value {
            MetadataValue::Text(text) => text.clone(),
            other => panic!("{other:?}"),
        };
        assert!(Arc::ptr_eq(&text(&store, "a"), &text(&store, "b")));
        for n in 0..10_000 {
            store.set(target("a"), "summary", "source", entry(&format!("summary {n}"), 0, 0, 0));
        }
        assert!(store.texts.len() < 1100, "{} texts kept", store.texts.len());
        assert_eq!(&*text(&store, "a"), "running");
    }

    #[test]
    fn expiry_index_replaces_renewed_and_removed_deadlines() {
        let mut store = MetadataStore::default();
        let target = EntityId::Root;
        for now in 0..1000 {
            store.set(
                target.clone(),
                "status",
                "source",
                MetadataEntry {
                    value: MetadataValue::Text("running".into()),
                    updated_at: now,
                    ttl_ms: Some(10),
                    precedence: 0,
                    ordinal: 0,
                },
            );
            assert_eq!(store.expiry_counts.len(), 1);
        }
        assert!(!store.expires_between(0, 1009));
        assert!(store.expires_between(1009, 1010));
        store.unset(&target, "status", "source");
        assert!(store.expiry_counts.is_empty());
        assert!(!store.expires_between(0, u64::MAX));
    }

    #[test]
    fn stale_providers_leave_the_expiry_index_and_rejoin_it_renewed() {
        // A workspace's facts (an entity's carry no lease).
        let mut store = MetadataStore::default();
        let target = EntityId::Tab(tab_workspace_id(1));
        let patch = MetadataPatch {
            target: MetadataTarget::Tab(tab_workspace_id(1)),
            source_id: "flotilla".into(),
            set: BTreeMap::from([(
                "status".to_owned(),
                MetadataValueUpdate { value: MetadataValue::Text("running".into()), ttl_ms: Some(10), precedence: None, ordinal: None },
            )]),
            unset: vec![],
        };
        store.apply_patch_from(Some("p"), patch.clone(), 0);
        // The same source under another provider is another contribution.
        store.apply_patch_from(Some("q"), patch, 0);
        assert_eq!(store.entries_for(&target, "status", 5).len(), 2);
        assert!(store.expires_between(0, 11));
        assert!(store.set_stale("p", true, 5));
        assert_eq!(store.expiry_counts.values().sum::<usize>(), 1);
        assert_eq!(store.entries_for(&target, "status", 1_000).len(), 1);
        assert!(store.set_stale("p", false, 1_000));
        assert!(store.expires_between(1_010, 1_011));
        assert_eq!(store.entries_for(&target, "status", 1_010).len(), 1);
        assert!(store.entries_for(&target, "status", 1_011).is_empty());
        let (removed, affected) = store.retract("p");
        assert!(removed);
        assert!(affected.is_empty(), "a workspace is not an entity");
        assert!(!store.has_stale());
        // q's deadline (10) is the only one left.
        assert_eq!(store.expiry_counts, BTreeMap::from([(10, 1)]));
        assert_eq!(store.retract("p"), (false, Default::default()));
    }

    /// An entity's facts are a record: the last write of a key wins, the
    /// same value from the same source changes nothing, an unset removes a
    /// key only its own source set, and the record goes with its last fact.
    #[test]
    fn an_entitys_facts_are_its_last_writers() {
        let mut store = MetadataStore::default();
        let entity = EntityRef::new("p", "convoy", "c");
        let target = EntityId::Entity(entity.clone());
        let patch = |source: &str, set: &[(&str, &str)], unset: &[&str]| MetadataPatch {
            target: MetadataTarget::Entity(entity.clone()),
            source_id: source.into(),
            set: set
                .iter()
                .map(|(key, value)| {
                    (
                        key.to_string(),
                        MetadataValueUpdate { value: MetadataValue::Text((*value).into()), ttl_ms: None, precedence: None, ordinal: None },
                    )
                })
                .collect(),
            unset: unset.iter().map(|key| key.to_string()).collect(),
        };
        let label = |store: &MetadataStore| store.resolved(&target, 0).get("label").map(|entry| entry.value.clone());
        assert!(store.apply_patch_from(Some("p"), patch("a", &[("label", "one"), ("state", "on")], &[]), 1).touched);
        assert!(!store.apply_patch_from(Some("p"), patch("a", &[("label", "one")], &[]), 2).touched, "same value, same source");
        store.apply_patch_from(Some("p"), patch("b", &[("label", "two")], &[]), 3);
        assert_eq!(label(&store), Some(MetadataValue::Text("two".into())), "the last write wins");
        assert_eq!(store.resolved(&target, 0).source("label"), Some("b"));
        assert!(!store.apply_patch_from(Some("p"), patch("a", &[], &["label"]), 4).touched, "a's unset can't remove b's");
        store.apply_patch_from(Some("p"), patch("b", &[], &["label"]), 5);
        assert_eq!(label(&store), None);
        assert!(store.source_contributes(&target, "state", Some("p"), "a"));
        store.apply_patch_from(Some("p"), patch("a", &[], &["state"]), 6);
        assert!(store.targets().next().is_none(), "the record goes with its last fact");
    }

    #[test]
    fn unset_removes_only_that_source_key() {
        let mut store = MetadataStore::default();
        store.set(EntityId::Pane(PaneTarget::Terminal(1)), "zellij.pane.cwd", "zellij", entry("/a", 0, 0, 1));
        store.set(EntityId::Pane(PaneTarget::Terminal(1)), "zellij.pane.cwd", "shell", entry("/b", 0, 0, 2));

        store.unset(&EntityId::Pane(PaneTarget::Terminal(1)), "zellij.pane.cwd", "zellij");

        let entries = store.entries_for(&EntityId::Pane(PaneTarget::Terminal(1)), "zellij.pane.cwd", 3);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "shell");
    }

    #[test]
    fn selector_prefers_precedence_before_count() {
        let entries = vec![
            CandidateEntry::new("a", entry("/same", 0, 0, 1)),
            CandidateEntry::new("b", entry("/same", 0, 1, 1)),
            CandidateEntry::new("c", entry("/focused", 10, 2, 1)),
        ];

        assert_eq!(select_primary_value(&entries), Some(MetadataValue::Text("/focused".into())));
    }

    #[test]
    fn selector_uses_count_inside_precedence_bucket() {
        let entries = vec![
            CandidateEntry::new("a", entry("/one", 0, 0, 1)),
            CandidateEntry::new("b", entry("/two", 0, 1, 1)),
            CandidateEntry::new("c", entry("/two", 0, 2, 1)),
        ];

        assert_eq!(select_primary_value(&entries), Some(MetadataValue::Text("/two".into())));
    }

    #[test]
    fn selector_uses_ordinal_after_count() {
        let entries = vec![CandidateEntry::new("a", entry("/later", 0, 10, 1)), CandidateEntry::new("b", entry("/earlier", 0, 2, 1))];

        assert_eq!(select_primary_value(&entries), Some(MetadataValue::Text("/earlier".into())));
    }

    #[test]
    fn expired_entries_are_not_returned() {
        let mut store = MetadataStore::default();
        let mut value = entry("/a", 0, 0, 10);
        value.ttl_ms = Some(5);
        store.set(EntityId::Pane(PaneTarget::Terminal(1)), "zellij.pane.cwd", "zellij", value);

        assert!(store.entries_for(&EntityId::Pane(PaneTarget::Terminal(1)), "zellij.pane.cwd", 16,).is_empty());
    }

    #[test]
    fn metadata_patch_sets_values_with_controller_timestamp() {
        // A workspace's facts keep a lease, precedence and ordinal each.
        let mut store = MetadataStore::default();
        let target = EntityId::Tab(tab_workspace_id(1));
        store.apply_patch(
            MetadataPatch {
                target: MetadataTarget::Tab(tab_workspace_id(1)),
                source_id: "flotilla".to_owned(),
                set: BTreeMap::from([(
                    "summary.local_llm".to_owned(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text("running tests".into()),
                        ttl_ms: Some(30_000),
                        precedence: Some(10),
                        ordinal: Some(2),
                    },
                )]),
                unset: vec![],
            },
            42,
        );

        let entries = store.entries_for(&target, "summary.local_llm", 42);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "flotilla");
        assert_eq!(entries[0].entry.updated_at, 42);
        assert_eq!(entries[0].entry.ttl_ms, Some(30_000));
        assert_eq!(entries[0].entry.precedence, 10);
        assert_eq!(entries[0].entry.ordinal, 2);
    }

    #[test]
    fn duplicate_metadata_patch_without_ttl_is_noop() {
        let mut store = MetadataStore::default();
        let target = EntityId::Pane(PaneTarget::Terminal(1));
        let patch = MetadataPatch {
            target: MetadataTarget::Pane(PaneTarget::Terminal(1)),
            source_id: "watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                MetadataValueUpdate {
                    value: MetadataValue::Text("zellij-org/zellij".into()),
                    ttl_ms: None,
                    precedence: Some(10),
                    ordinal: Some(2),
                },
            )]),
            unset: vec![],
        };

        let first = store.apply_patch(patch.clone(), 10);
        let second = store.apply_patch(patch, 11);

        let entries = store.entries_for(&target, "git.repo", 11);
        assert_eq!(first.view_changed, true);
        assert_eq!(first.touched, true);
        assert_eq!(second.view_changed, false);
        assert_eq!(second.touched, false);
        assert_eq!(entries[0].entry.updated_at, 10);
    }

    #[test]
    fn duplicate_metadata_patch_with_ttl_refreshes_without_view_change() {
        let mut store = MetadataStore::default();
        let target = EntityId::Pane(PaneTarget::Terminal(1));
        let patch = MetadataPatch {
            target: MetadataTarget::Pane(PaneTarget::Terminal(1)),
            source_id: "watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                MetadataValueUpdate {
                    value: MetadataValue::Text("zellij-org/zellij".into()),
                    ttl_ms: Some(10_000),
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        };

        let first = store.apply_patch(patch.clone(), 10);
        let second = store.apply_patch(patch, 11);

        let entries = store.entries_for(&target, "git.repo", 11);
        assert_eq!(first.view_changed, true);
        assert_eq!(first.touched, true);
        assert_eq!(second.view_changed, false);
        assert_eq!(second.touched, true);
        assert_eq!(entries[0].entry.updated_at, 11);
    }

    #[test]
    fn metadata_patch_omitted_keys_are_unchanged() {
        let mut store = MetadataStore::default();
        let target = EntityId::Pane(PaneTarget::Terminal(1));
        store.set(target.clone(), "zellij.pane.cwd", "zellij", entry("/repo", 0, 0, 1));

        store.apply_patch(
            MetadataPatch {
                target: MetadataTarget::Pane(PaneTarget::Terminal(1)),
                source_id: "zellij".to_owned(),
                set: BTreeMap::from([(
                    "zellij.pane.title".to_owned(),
                    MetadataValueUpdate { value: MetadataValue::Text("server".into()), ttl_ms: None, precedence: None, ordinal: None },
                )]),
                unset: vec![],
            },
            2,
        );

        assert_eq!(store.entries_for(&target, "zellij.pane.cwd", 2).len(), 1);
        let title = store.entries_for(&target, "zellij.pane.title", 2);
        assert_eq!(title.len(), 1);
        assert_eq!(title[0].entry.precedence, 0);
        assert_eq!(title[0].entry.ordinal, 0);
    }

    #[test]
    fn metadata_patch_unset_removes_only_patch_source_key() {
        let mut store = MetadataStore::default();
        let target = EntityId::Pane(PaneTarget::Terminal(1));
        store.set(target.clone(), "zellij.pane.cwd", "zellij", entry("/repo", 0, 0, 1));
        store.set(target.clone(), "zellij.pane.cwd", "shell", entry("/shell", 0, 0, 1));

        store.apply_patch(
            MetadataPatch {
                target: MetadataTarget::Pane(PaneTarget::Terminal(1)),
                source_id: "zellij".to_owned(),
                set: BTreeMap::new(),
                unset: vec!["zellij.pane.cwd".to_owned()],
            },
            2,
        );

        let entries = store.entries_for(&target, "zellij.pane.cwd", 2);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "shell");
    }
}

#[cfg(test)]
impl MetadataStore {
    /// Count what the store holds: facts by entity, and the resolutions kept.
    pub(crate) fn census(&self, census: &mut Census) {
        for (target, keys) in &self.entries {
            if let EntityId::Entity(entity) = target {
                census.entity("store: entity ids", entity);
            }
            census.shape("store: facts per entity", keys.keys());
            for key in keys.keys() {
                if census.first(key) {
                    census.string("store: fact keys (interned)", key);
                }
            }
            for contributions in keys.values() {
                for (_, entry) in contributions.shared() {
                    if census.first(entry) {
                        census.value("store: fact values", &entry.value);
                    }
                }
            }
        }
        for source in &self.sources {
            census.string("store: sources (interned)", source);
        }
        for stored in self.entries.values() {
            if let Some(cached) = stored.resolved.borrow().as_ref().filter(|cached| census.first(&cached.resolved)) {
                cached.resolved.census(census, "resolved facts");
            }
        }
        for (target, record) in &self.records {
            if let EntityId::Entity(entity) = target {
                census.entity("records: entity ids", entity);
            }
            if census.first(&record.facts) {
                record.facts.census(census, "records");
            }
        }
    }
}

#[cfg(test)]
impl ResolvedFacts {
    pub(crate) fn census(&self, census: &mut Census, prefix: &str) {
        // Keys and sources are shared (a spine, interned sources); count
        // each spine once, and the facts each record holds.
        if census.first(self.shape()) {
            census.shape(&format!("{prefix}: spines"), self.keys());
            for key in self.shape().keys() {
                if census.first(key) {
                    census.string(&format!("{prefix}: spine keys"), key);
                }
            }
        }
        for (_, fact) in self.facts() {
            if census.first(&fact.entry) {
                census.value(&format!("{prefix}: values (not shared)"), &fact.entry.value);
            }
        }
    }
}
