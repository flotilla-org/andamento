use std::collections::{BTreeMap, HashMap};

use tabs_shared::{MetadataEntry, MetadataTarget, MetadataValue};

pub type EntityId = MetadataTarget;

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
}

impl MetadataStore {
    pub fn set(
        &mut self,
        entity_id: EntityId,
        key: impl Into<String>,
        source_id: impl Into<String>,
        entry: MetadataEntry,
    ) {
        self.entries
            .entry(entity_id)
            .or_default()
            .entry(key.into())
            .or_default()
            .insert(source_id.into(), entry);
    }

    pub fn unset(&mut self, entity_id: &EntityId, key: &str, source_id: &str) {
        let Some(entity_entries) = self.entries.get_mut(entity_id) else {
            return;
        };
        let Some(key_entries) = entity_entries.get_mut(key) else {
            return;
        };
        key_entries.remove(source_id);
        if key_entries.is_empty() {
            entity_entries.remove(key);
        }
        if entity_entries.is_empty() {
            self.entries.remove(entity_id);
        }
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
}

fn entry_is_live(entry: &MetadataEntry, now: u64) -> bool {
    entry
        .ttl_ms
        .map(|ttl| now <= entry.updated_at.saturating_add(ttl))
        .unwrap_or(true)
}

pub fn select_primary_value(entries: &[CandidateEntry]) -> Option<MetadataValue> {
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
        .map(|(value, _)| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tabs_shared::{GroupPath, GroupSegment, MetadataEntry, MetadataValue, PaneTarget};

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
    fn group_targets_are_distinct_metadata_entities() {
        let mut store = MetadataStore::default();
        let group = GroupPath(vec![GroupSegment {
            key: "project.name".to_owned(),
            value: MetadataValue::Text("zellij".to_owned()),
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
}
