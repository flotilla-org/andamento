use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{
    MetadataEntry, MetadataPatch, MetadataSourceEntry, MetadataTarget, MetadataValue,
    MetadataValueUpdate, ResolvedMetadataTarget,
};

pub type EntityId = ResolvedMetadataTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEntry {
    pub source_id: String,
    pub entry: MetadataEntry,
}

impl CandidateEntry {
    pub fn new(source_id: impl Into<String>, entry: MetadataEntry) -> Self {
        Self {
            source_id: source_id.into(),
            entry,
        }
    }
}

/// An interned provider (subscription) name.
type ProviderIx = u32;

/// Who contributed a fact: the provider whose patch set it, and the
/// producer's source ID. Facts with no provider are the core's or the host's
/// own; neither retraction nor staleness touches them. The same source ID
/// under two providers is two contributors.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Contributor {
    provider: Option<ProviderIx>,
    source: String,
}

#[derive(Debug, Default)]
pub struct MetadataStore {
    entries: HashMap<EntityId, HashMap<String, BTreeMap<Contributor, MetadataEntry>>>,
    target_ordinals: HashMap<EntityId, i64>,
    /// Deadlines of the TTL facts that can expire: every one whose provider
    /// is not stale.
    expiry_counts: BTreeMap<u64, usize>,
    providers: Vec<String>,
    provider_ixs: HashMap<String, ProviderIx>,
    /// Stale providers, with the time each became stale. Their facts are
    /// judged live as of that time, so none expires while it stays stale.
    stale: BTreeMap<ProviderIx, u64>,
}

impl MetadataStore {
    pub fn targets(&self) -> impl Iterator<Item = &EntityId> {
        self.entries.keys()
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

    fn contributor(&mut self, provider: Option<&str>, source: impl Into<String>) -> Contributor {
        Contributor {
            provider: provider.map(|p| self.intern(p)),
            source: source.into(),
        }
    }

    /// The contributor if its provider is known; an unknown provider has
    /// contributed nothing.
    fn existing_contributor(&self, provider: Option<&str>, source: &str) -> Option<Contributor> {
        Some(Contributor {
            provider: match provider {
                Some(p) => Some(*self.provider_ixs.get(p)?),
                None => None,
            },
            source: source.to_owned(),
        })
    }

    /// The time `contributor`'s facts are judged live at: now, or when its
    /// provider became stale.
    fn liveness_time(&self, contributor: &Contributor, now: u64) -> u64 {
        contributor
            .provider
            .and_then(|p| self.stale.get(&p))
            .map_or(now, |since| (*since).min(now))
    }

    fn is_live(&self, contributor: &Contributor, entry: &MetadataEntry, now: u64) -> bool {
        entry_is_live(entry, self.liveness_time(contributor, now))
    }

    fn indexed(&self, contributor: &Contributor) -> bool {
        contributor
            .provider
            .is_none_or(|p| !self.stale.contains_key(&p))
    }

    /// Set a fact the core or host owns (no provider).
    pub fn set(
        &mut self,
        entity_id: EntityId,
        key: impl Into<String>,
        source_id: impl Into<String>,
        entry: MetadataEntry,
    ) {
        let contributor = self.contributor(None, source_id);
        self.set_contribution(entity_id, key.into(), contributor, entry);
    }

    fn set_contribution(
        &mut self,
        entity_id: EntityId,
        key: String,
        contributor: Contributor,
        entry: MetadataEntry,
    ) {
        self.target_ordinals
            .entry(entity_id.clone())
            .or_insert(entry.ordinal);
        let indexed = self.indexed(&contributor);
        let deadline = entry.ttl_ms.map(|ttl| entry.updated_at.saturating_add(ttl));
        let previous = self
            .entries
            .entry(entity_id)
            .or_default()
            .entry(key)
            .or_default()
            .insert(contributor, entry);
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
    pub fn unset(
        &mut self,
        entity_id: &EntityId,
        key: &str,
        source_id: &str,
    ) -> Option<MetadataEntry> {
        let contributor = self.existing_contributor(None, source_id)?;
        self.unset_contribution(entity_id, key, &contributor)
    }

    fn unset_contribution(
        &mut self,
        entity_id: &EntityId,
        key: &str,
        contributor: &Contributor,
    ) -> Option<MetadataEntry> {
        let entity_entries = self.entries.get_mut(entity_id)?;
        let key_entries = entity_entries.get_mut(key)?;
        let removed = key_entries.remove(contributor);
        if key_entries.is_empty() {
            entity_entries.remove(key);
        }
        if entity_entries.is_empty() {
            self.entries.remove(entity_id);
            self.target_ordinals.remove(entity_id);
        }
        if let Some(entry) = &removed {
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
            *self
                .expiry_counts
                .entry(entry.updated_at.saturating_add(ttl))
                .or_default() += 1;
        }
    }

    /// Entries are live through their deadline. Storage is bounded by the
    /// number of retained contributions; renewing a lease replaces its deadline.
    pub fn expires_between(&self, before: u64, now: u64) -> bool {
        before < now && self.expiry_counts.range(before..now).next().is_some()
    }

    pub fn target_ordinal(&self, entity_id: &EntityId) -> Option<i64> {
        self.target_ordinals.get(entity_id).copied()
    }

    pub(crate) fn source_contributes(
        &self,
        entity: &EntityId,
        key: &str,
        provider: Option<&str>,
        source: &str,
    ) -> bool {
        let Some(contributor) = self.existing_contributor(provider, source) else {
            return false;
        };
        self.entries
            .get(entity)
            .and_then(|keys| keys.get(key))
            .is_some_and(|sources| sources.contains_key(&contributor))
    }

    // Raw ownership survives lease expiry: silence is not authoritative removal.
    pub(crate) fn has_contributors(&self, entity: &EntityId, key: &str) -> bool {
        self.entries
            .get(entity)
            .and_then(|keys| keys.get(key))
            .is_some_and(|sources| !sources.is_empty())
    }

    /// Whether any provider is stale.
    pub fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }

    /// Whether `provider` is stale.
    pub fn is_stale(&self, provider: &str) -> bool {
        self.provider_ixs
            .get(provider)
            .is_some_and(|p| self.stale.contains_key(p))
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
                                entry.updated_at = now;
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
    pub fn retract(&mut self, provider: &str) -> (bool, BTreeSet<crate::EntityRef>) {
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
            if let (true, EntityId::Entity(entity)) = (lost, target) {
                affected.insert(entity.clone());
            }
            if keys.is_empty() {
                self.target_ordinals.remove(target);
                false
            } else {
                true
            }
        });
        self.entries = entries;
        self.stale.remove(&ix);
        (removed, affected)
    }

    pub fn source_entry(
        &self,
        entity_id: &EntityId,
        key: &str,
        source_id: &str,
        now: u64,
    ) -> Option<&MetadataEntry> {
        self.entries
            .get(entity_id)?
            .get(key)?
            .iter()
            .find(|(contributor, entry)| {
                contributor.source == source_id && self.is_live(contributor, entry, now)
            })
            .map(|(_, entry)| entry)
    }

    /// Apply a patch the core or host owns (no provider).
    #[allow(dead_code)]
    pub fn apply_patch(&mut self, patch: MetadataPatch, now: u64) -> MetadataPatchOutcome {
        self.apply_patch_from(None, patch, now)
    }

    /// Apply a patch, contributed by `provider` when it has one.
    pub fn apply_patch_from(
        &mut self,
        provider: Option<&str>,
        patch: MetadataPatch,
        now: u64,
    ) -> MetadataPatchOutcome {
        let mut outcome = MetadataPatchOutcome::default();
        let target = EntityId::from(patch.target);
        let target_existed = self.entries.contains_key(&target);
        let contributor = self.contributor(provider, patch.source_id);
        for key in patch.unset {
            if let Some(entry) = self.unset_contribution(&target, &key, &contributor) {
                outcome.touched = true;
                outcome.view_changed |= self.is_live(&contributor, &entry, now);
            }
        }
        if !self.entries.contains_key(&target) {
            if let Some(ordinal) = patch
                .set
                .values()
                .map(|update| update.ordinal.unwrap_or_default())
                .min()
            {
                self.target_ordinals.insert(target.clone(), ordinal);
            }
        }
        for (key, update) in patch.set {
            let next_entry = MetadataEntry {
                value: update.value,
                updated_at: now,
                ttl_ms: update.ttl_ms,
                precedence: update.precedence.unwrap_or_default(),
                ordinal: update.ordinal.unwrap_or_default(),
            };
            let existing_entry = self
                .entries
                .get(&target)
                .and_then(|entity_entries| entity_entries.get(&key))
                .and_then(|source_entries| source_entries.get(&contributor))
                .cloned();

            match existing_entry {
                Some(existing)
                    if metadata_entry_payload_matches(&existing, &next_entry)
                        && self.is_live(&contributor, &existing, now) =>
                {
                    if existing.ttl_ms.is_some() {
                        self.set_contribution(target.clone(), key, contributor.clone(), next_entry);
                        outcome.touched = true;
                    }
                }
                _ => {
                    self.set_contribution(target.clone(), key, contributor.clone(), next_entry);
                    outcome.touched = true;
                    outcome.view_changed = true;
                }
            }
        }
        // Entity identity can itself participate in placement queries even after all
        // its facts expire. Removing the last expired contribution removes it.
        outcome.view_changed |= target_existed != self.entries.contains_key(&target);
        outcome
    }

    pub fn entries_for(&self, entity_id: &EntityId, key: &str, now: u64) -> Vec<CandidateEntry> {
        self.entries
            .get(entity_id)
            .and_then(|entity_entries| entity_entries.get(key))
            .map(|source_entries| {
                source_entries
                    .iter()
                    .filter(|(contributor, entry)| self.is_live(contributor, entry, now))
                    .map(|(contributor, entry)| {
                        CandidateEntry::new(contributor.source.clone(), entry.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Preserve the winning producer alongside each selected entry in one pass.
    pub fn resolved_entries_with_sources_for(
        &self,
        entity_id: &EntityId,
        now: u64,
    ) -> (BTreeMap<String, MetadataEntry>, BTreeMap<String, String>) {
        let mut values = BTreeMap::new();
        let mut sources = BTreeMap::new();
        if let Some(entries) = self.entries.get(entity_id) {
            for key in entries.keys() {
                if let Some(candidate) =
                    select_primary_entry(&self.entries_for(entity_id, key, now))
                {
                    sources.insert(key.clone(), candidate.source_id);
                    values.insert(key.clone(), candidate.entry);
                }
            }
        }
        (values, sources)
    }

    pub fn resolved_entries_for(
        &self,
        entity_id: &EntityId,
        now: u64,
    ) -> BTreeMap<String, MetadataEntry> {
        self.entries
            .get(entity_id)
            .map(|entity_entries| {
                entity_entries
                    .iter()
                    .filter_map(|(key, _)| {
                        select_primary_entry(&self.entries_for(entity_id, key, now))
                            .map(|candidate| (key.clone(), candidate.entry))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn source_entries_for(
        &self,
        entity_id: &EntityId,
        now: u64,
    ) -> BTreeMap<String, Vec<MetadataSourceEntry>> {
        self.entries
            .get(entity_id)
            .map(|entity_entries| {
                entity_entries
                    .iter()
                    .filter_map(|(key, _)| {
                        let entries = self
                            .entries_for(entity_id, key, now)
                            .into_iter()
                            .map(|candidate| MetadataSourceEntry {
                                source_id: candidate.source_id,
                                entry: candidate.entry,
                            })
                            .collect::<Vec<_>>();
                        (!entries.is_empty()).then_some((key.clone(), entries))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn snapshot_patches(&self, now: u64) -> Vec<MetadataPatch> {
        let mut patches: BTreeMap<(EntityId, String), BTreeMap<String, MetadataValueUpdate>> =
            BTreeMap::new();
        for (target, target_entries) in &self.entries {
            for (key, source_entries) in target_entries {
                for (contributor, entry) in source_entries {
                    if !self.is_live(contributor, entry, now) {
                        continue;
                    }
                    patches
                        .entry((target.clone(), contributor.source.clone()))
                        .or_default()
                        .insert(
                            key.clone(),
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
            .filter_map(|((target, source_id), set)| {
                let target = match target {
                    EntityId::Root => MetadataTarget::Root,
                    EntityId::Pane(pane) => MetadataTarget::Pane(pane),
                    EntityId::Tab(tab) => MetadataTarget::Tab(tab),
                    EntityId::Entity(entity) => MetadataTarget::Entity(entity),
                    EntityId::Identity(identity) => MetadataTarget::Identity(identity),
                };
                Some(MetadataPatch {
                    target,
                    source_id,
                    set,
                    unset: vec![],
                })
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
    left.value == right.value
        && left.ttl_ms == right.ttl_ms
        && left.precedence == right.precedence
        && left.ordinal == right.ordinal
}

fn entry_is_live(entry: &MetadataEntry, now: u64) -> bool {
    entry
        .ttl_ms
        .map(|ttl| now <= entry.updated_at.saturating_add(ttl))
        .unwrap_or(true)
}

pub fn select_primary_entry(entries: &[CandidateEntry]) -> Option<CandidateEntry> {
    let max_precedence = entries
        .iter()
        .map(|candidate| candidate.entry.precedence)
        .max()?;
    let mut grouped: BTreeMap<MetadataValue, (usize, i64, String)> = BTreeMap::new();
    for candidate in entries
        .iter()
        .filter(|candidate| candidate.entry.precedence == max_precedence)
    {
        let value = candidate.entry.value.clone();
        let grouped_entry = grouped.entry(value).or_insert((
            0,
            candidate.entry.ordinal,
            candidate.source_id.clone(),
        ));
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
                .min_by_key(|candidate| {
                    (
                        candidate.entry.ordinal,
                        candidate.source_id.as_str(),
                        candidate.entry.updated_at,
                    )
                })
                .cloned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_primary_value(entries: &[CandidateEntry]) -> Option<MetadataValue> {
        select_primary_entry(entries).map(|candidate| candidate.entry.value)
    }
    use crate::{MetadataEntry, MetadataPatch, MetadataValue, MetadataValueUpdate, PaneTarget};

    fn entry(value: &str, precedence: i64, ordinal: i64, updated_at: u64) -> MetadataEntry {
        MetadataEntry {
            value: MetadataValue::Text(value.to_owned()),
            updated_at,
            ttl_ms: None,
            precedence,
            ordinal,
        }
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
        let mut store = MetadataStore::default();
        let target = EntityId::Entity(crate::EntityRef::new("p", "vessel", "v"));
        let patch = MetadataPatch {
            target: MetadataTarget::Entity(crate::EntityRef::new("p", "vessel", "v")),
            source_id: "flotilla".into(),
            set: BTreeMap::from([(
                "status".to_owned(),
                MetadataValueUpdate {
                    value: MetadataValue::Text("running".into()),
                    ttl_ms: Some(10),
                    precedence: None,
                    ordinal: None,
                },
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
        assert_eq!(affected.len(), 1);
        assert!(!store.has_stale());
        // q's deadline (10) is the only one left.
        assert_eq!(store.expiry_counts, BTreeMap::from([(10, 1)]));
        assert_eq!(store.retract("p"), (false, Default::default()));
    }

    #[test]
    fn unset_removes_only_that_source_key() {
        let mut store = MetadataStore::default();
        store.set(
            EntityId::Pane(PaneTarget::Terminal(1)),
            "zellij.pane.cwd",
            "zellij",
            entry("/a", 0, 0, 1),
        );
        store.set(
            EntityId::Pane(PaneTarget::Terminal(1)),
            "zellij.pane.cwd",
            "shell",
            entry("/b", 0, 0, 2),
        );

        store.unset(
            &EntityId::Pane(PaneTarget::Terminal(1)),
            "zellij.pane.cwd",
            "zellij",
        );

        let entries = store.entries_for(
            &EntityId::Pane(PaneTarget::Terminal(1)),
            "zellij.pane.cwd",
            3,
        );
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

        assert_eq!(
            select_primary_value(&entries),
            Some(MetadataValue::Text("/focused".to_owned()))
        );
    }

    #[test]
    fn selector_uses_count_inside_precedence_bucket() {
        let entries = vec![
            CandidateEntry::new("a", entry("/one", 0, 0, 1)),
            CandidateEntry::new("b", entry("/two", 0, 1, 1)),
            CandidateEntry::new("c", entry("/two", 0, 2, 1)),
        ];

        assert_eq!(
            select_primary_value(&entries),
            Some(MetadataValue::Text("/two".to_owned()))
        );
    }

    #[test]
    fn selector_uses_ordinal_after_count() {
        let entries = vec![
            CandidateEntry::new("a", entry("/later", 0, 10, 1)),
            CandidateEntry::new("b", entry("/earlier", 0, 2, 1)),
        ];

        assert_eq!(
            select_primary_value(&entries),
            Some(MetadataValue::Text("/earlier".to_owned()))
        );
    }

    #[test]
    fn expired_entries_are_not_returned() {
        let mut store = MetadataStore::default();
        let mut value = entry("/a", 0, 0, 10);
        value.ttl_ms = Some(5);
        store.set(
            EntityId::Pane(PaneTarget::Terminal(1)),
            "zellij.pane.cwd",
            "zellij",
            value,
        );

        assert!(store
            .entries_for(
                &EntityId::Pane(PaneTarget::Terminal(1)),
                "zellij.pane.cwd",
                16,
            )
            .is_empty());
    }

    #[test]
    fn metadata_patch_sets_values_with_controller_timestamp() {
        let mut store = MetadataStore::default();
        let entity = crate::EntityRef::local("project".to_owned(), "zellij".to_owned());
        let target = EntityId::Entity(entity.clone());
        store.apply_patch(
            MetadataPatch {
                target: MetadataTarget::Entity(entity),
                source_id: "flotilla".to_owned(),
                set: BTreeMap::from([(
                    "summary.local_llm".to_owned(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text("running tests".to_owned()),
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
    fn target_ordinal_comes_from_the_patch_without_an_identity_value() {
        let mut store = MetadataStore::default();
        let entity = crate::EntityRef::local(
            "issue".to_owned(),
            "github/flotilla-org/andamento#37".to_owned(),
        );
        let target = EntityId::Entity(entity.clone());

        store.apply_patch(
            MetadataPatch {
                target: MetadataTarget::Entity(entity),
                source_id: "flotilla".to_owned(),
                set: BTreeMap::from([(
                    "display.label".to_owned(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text("#37".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: Some(7),
                    },
                )]),
                unset: vec![],
            },
            1,
        );

        assert_eq!(store.target_ordinal(&target), Some(7));
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
                    value: MetadataValue::Text("zellij-org/zellij".to_owned()),
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
                    value: MetadataValue::Text("zellij-org/zellij".to_owned()),
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
        store.set(
            target.clone(),
            "zellij.pane.cwd",
            "zellij",
            entry("/repo", 0, 0, 1),
        );

        store.apply_patch(
            MetadataPatch {
                target: MetadataTarget::Pane(PaneTarget::Terminal(1)),
                source_id: "zellij".to_owned(),
                set: BTreeMap::from([(
                    "zellij.pane.title".to_owned(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text("server".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
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
        store.set(
            target.clone(),
            "zellij.pane.cwd",
            "zellij",
            entry("/repo", 0, 0, 1),
        );
        store.set(
            target.clone(),
            "zellij.pane.cwd",
            "shell",
            entry("/shell", 0, 0, 1),
        );

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
