use std::collections::{BTreeMap, HashMap};

use andamento_shared::{
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

#[derive(Debug, Default)]
pub struct MetadataStore {
    entries: HashMap<EntityId, HashMap<String, BTreeMap<String, MetadataEntry>>>,
    target_ordinals: HashMap<EntityId, i64>,
}

impl MetadataStore {
    pub fn targets(&self) -> impl Iterator<Item = &EntityId> {
        self.entries.keys()
    }

    pub fn set(
        &mut self,
        entity_id: EntityId,
        key: impl Into<String>,
        source_id: impl Into<String>,
        entry: MetadataEntry,
    ) {
        self.target_ordinals
            .entry(entity_id.clone())
            .or_insert(entry.ordinal);
        self.entries
            .entry(entity_id)
            .or_default()
            .entry(key.into())
            .or_default()
            .insert(source_id.into(), entry);
    }

    pub fn unset(
        &mut self,
        entity_id: &EntityId,
        key: &str,
        source_id: &str,
    ) -> Option<MetadataEntry> {
        let Some(entity_entries) = self.entries.get_mut(entity_id) else {
            return None;
        };
        let Some(key_entries) = entity_entries.get_mut(key) else {
            return None;
        };
        let removed = key_entries.remove(source_id);
        if key_entries.is_empty() {
            entity_entries.remove(key);
        }
        if entity_entries.is_empty() {
            self.entries.remove(entity_id);
            self.target_ordinals.remove(entity_id);
        }
        removed
    }

    pub fn target_ordinal(&self, entity_id: &EntityId) -> Option<i64> {
        self.target_ordinals.get(entity_id).copied()
    }

    pub fn source_entry(
        &self,
        entity_id: &EntityId,
        key: &str,
        source_id: &str,
        now: u64,
    ) -> Option<&MetadataEntry> {
        let entry = self.entries.get(entity_id)?.get(key)?.get(source_id)?;
        entry_is_live(entry, now).then_some(entry)
    }

    #[allow(dead_code)]
    pub fn apply_patch(&mut self, patch: MetadataPatch, now: u64) -> MetadataPatchOutcome {
        let mut outcome = MetadataPatchOutcome::default();
        let target = EntityId::from(patch.target);
        for key in patch.unset {
            if let Some(entry) = self.unset(&target, &key, &patch.source_id) {
                outcome.touched = true;
                outcome.view_changed |= entry_is_live(&entry, now);
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
                .and_then(|source_entries| source_entries.get(&patch.source_id))
                .cloned();

            match existing_entry {
                Some(existing)
                    if metadata_entry_payload_matches(&existing, &next_entry)
                        && entry_is_live(&existing, now) =>
                {
                    if existing.ttl_ms.is_some() {
                        self.set(target.clone(), key, patch.source_id.clone(), next_entry);
                        outcome.touched = true;
                    }
                }
                Some(existing) if metadata_entry_payload_matches(&existing, &next_entry) => {
                    self.set(target.clone(), key, patch.source_id.clone(), next_entry);
                    outcome.touched = true;
                    outcome.view_changed = true;
                }
                _ => {
                    self.set(target.clone(), key, patch.source_id.clone(), next_entry);
                    outcome.touched = true;
                    outcome.view_changed = true;
                }
            }
        }
        outcome
    }

    pub fn entries_for(&self, entity_id: &EntityId, key: &str, now: u64) -> Vec<CandidateEntry> {
        self.entries
            .get(entity_id)
            .and_then(|entity_entries| entity_entries.get(key))
            .map(|source_entries| {
                source_entries
                    .iter()
                    .filter(|(_, entry)| entry_is_live(entry, now))
                    .map(|(source_id, entry)| CandidateEntry::new(source_id, entry.clone()))
                    .collect()
            })
            .unwrap_or_default()
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
                for (source_id, entry) in source_entries {
                    if !entry_is_live(entry, now) {
                        continue;
                    }
                    patches
                        .entry((target.clone(), source_id.clone()))
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
                    EntityId::Group(_) => return None,
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

pub fn select_primary_value(entries: &[CandidateEntry]) -> Option<MetadataValue> {
    select_primary_entry(entries).map(|candidate| candidate.entry.value)
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
    use andamento_shared::{
        GroupPath, GroupSegment, MetadataEntry, MetadataPatch, MetadataValue, MetadataValueUpdate,
        PaneTarget,
    };

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
    fn derived_group_targets_are_distinct_internal_metadata_entities() {
        let mut store = MetadataStore::default();
        let group = GroupPath(vec![GroupSegment {
            key: "project.name".to_owned(),
            value: MetadataValue::Text("zellij".to_owned()),
            label: None,
        }]);
        store.set(
            EntityId::Group(group.clone()),
            "summary.local_llm",
            "flotilla",
            entry("running tests", 0, 0, 1),
        );

        let entries = store.entries_for(&EntityId::Group(group), "summary.local_llm", 1);

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].entry.value,
            MetadataValue::Text("running tests".to_owned())
        );
    }

    #[test]
    fn metadata_patch_sets_values_with_controller_timestamp() {
        let mut store = MetadataStore::default();
        let entity = andamento_shared::EntityRef {
            kind: "project".to_owned(),
            id: "zellij".to_owned(),
        };
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
        let entity = andamento_shared::EntityRef {
            kind: "issue".to_owned(),
            id: "github/flotilla-org/andamento#37".to_owned(),
        };
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
