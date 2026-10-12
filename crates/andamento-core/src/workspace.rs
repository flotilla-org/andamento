//! One workspace's logical state: its subject, its slots (the Workspace
//! Overlay over a cached baseline), its arrangement and its saved portable
//! Target Resolutions. It is also the `workspace/<id>` record: each part is
//! persisted without its generation.
//!
//! Each part is [`Versioned`] on the sidebar's one clock, so every change
//! moves a generation, and the record's generation is the latest of its
//! parts': it can't change without moving.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    managed::{DesiredContent, ManagedContent, SlotSource},
    records,
    records::SubjectRecord,
    slots::{
        Arrangement, ArrangementCommit, ArrangementDoc, ArrangementView, Baseline, OverlayView, Resolve, SlotInfo, StoredArrangement,
        WorkspaceSlots,
    },
    suggested_layout::{Content, RebindPolicy, ViewSpec},
    target_resolution::{PortableResolution, SavedResolution, SavedResolutions},
    versioned::{Clock, CommitError, Versioned},
    EntityRef,
};

/// The entity a workspace was opened for, and the entities on its path as
/// last seen, so its row can be drawn where it was when no producer
/// publishes them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub entity: Option<EntityRef>,
    #[serde(with = "crate::records::pairs")]
    pub retained: BTreeMap<EntityRef, SubjectRecord>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub subject: Versioned<Subject>,
    pub slots: Versioned<WorkspaceSlots>,
    pub arrangement: StoredArrangement,
    pub resolutions: Versioned<SavedResolutions>,
}

impl WorkspaceRecord {
    /// The record's generation: it changes whenever any part does, and is
    /// never reused.
    pub fn generation(&self) -> u64 {
        self.subject.generation().max(self.content_generation())
    }

    /// The generation of what a host reads as workspace content: slots,
    /// arrangement and resolutions.
    pub fn content_generation(&self) -> u64 {
        self.slots.generation().max(self.arrangement.generation()).max(self.resolutions.generation())
    }

    pub fn encode(&self) -> String {
        records::encode(self)
    }

    /// Read a record [`encode`](Self::encode) wrote, giving its parts
    /// generations no earlier state had (an arrangement only once stored).
    pub fn decode(text: &str, clock: &mut Clock) -> Result<Self, String> {
        let mut workspace: Self = records::decode(text, "workspace")?;
        if let Some(arrangement) = workspace.arrangement.get() {
            arrangement.doc.check()?;
            if arrangement.owned && !arrangement.soft.is_empty() {
                return Err("an owned arrangement has no soft overrides".into());
            }
            workspace.arrangement.restamp(clock);
        }
        for saved in workspace.resolutions.get().by_slot.values() {
            saved.resolution.check()?;
        }
        workspace.subject.restamp(clock);
        workspace.slots.restamp(clock);
        let stamp = clock.next();
        if !workspace.resolutions.follow(clock, |r| r.restamp(stamp)) {
            workspace.resolutions.restamp(clock);
        }
        Ok(workspace)
    }

    pub fn subject(&self) -> Option<&EntityRef> {
        self.subject.get().entity.as_ref()
    }

    pub fn slot(&self, key: &str) -> Result<SlotInfo, String> {
        self.slots.get().slot(key).ok_or_else(|| format!("no slot {key:?}"))
    }

    /// Where a slot's resolution comes from: a baseline slot that follows
    /// its provider resolves from the subject's layout; any other slot from
    /// its spec, as a local recipe or a provider facet (`facet` of the
    /// entity's own layout, `primary` for the `workspace.primary.*` facts).
    pub fn slot_source(&self, slot: &SlotInfo) -> SlotSource {
        match (self.subject(), slot.in_baseline && !slot.detached, &slot.spec.content) {
            (Some(subject), true, _) => SlotSource::Provider { entity: subject.clone(), slot: slot.key.clone() },
            (_, _, Content::ProviderFacet { entity, facet }) => SlotSource::Provider { entity: entity.clone(), slot: facet.clone() },
            (_, _, Content::Local(recipe)) => SlotSource::Local(recipe.clone()),
        }
    }

    /// Record the subject and its path as retained now. Returns whether
    /// they changed.
    pub fn follow_subject(&mut self, clock: &mut Clock, subject: Subject) -> bool {
        self.subject.follow(clock, |current| *current = subject)
    }

    /// Adopt the subject's current Suggested Layout as the baseline, or
    /// none (a malformed or expired layout keeps the cached one), and bring
    /// the arrangement up to date. Returns whether anything changed.
    pub fn follow_baseline(&mut self, clock: &mut Clock, baseline: Option<Baseline>) -> bool {
        let mut changed = false;
        if let Some(baseline) = baseline {
            let hint_changed = self.slots.get().baseline.as_ref().is_some_and(|old| old.hint != baseline.hint);
            changed = self.slots.follow(clock, |slots| {
                slots.set_baseline(baseline);
            });
            if hint_changed {
                changed |= self.arrangement.follow(clock, Arrangement::note_hint_change);
            }
        }
        if changed || (self.arrangement.get().is_none() && !self.slots.get().is_empty()) {
            changed |= self.follow_slots(clock);
        }
        changed
    }

    /// Add the user's own slot, or edit a baseline slot (see
    /// [`WorkspaceSlots::set`]). The arrangement is unchanged, except that an
    /// unowned one shows a tombstoned slot this restores. A slot whose
    /// content changed loses its saved resolution. Returns whether anything
    /// changed.
    pub fn set_slot(&mut self, clock: &mut Clock, key: &str, spec: ViewSpec, rebind: RebindPolicy) -> Result<bool, String> {
        if let Content::ProviderFacet { entity, .. } = &spec.content {
            entity.check_provider()?;
        }
        let before = self.slots.get().clone();
        let changed = self.slots.edit(clock, |slots| slots.set(key, spec, rebind))?;
        if self.slots.get().hidden() != before.hidden() {
            self.follow_unowned(clock);
        }
        self.forget_resolution_if_edited(clock, key, &before);
        Ok(changed)
    }

    /// Remove a slot (see [`WorkspaceSlots::remove`]). An owned arrangement
    /// keeps its tab, reported gone; one derived from the provider's hint
    /// loses it. Returns whether anything changed.
    pub fn remove_slot(&mut self, clock: &mut Clock, key: &str) -> Result<bool, String> {
        let removed = self.slots.edit(clock, |slots| slots.remove(key))?;
        if removed {
            self.follow_unowned(clock);
            if self.slots.get().slot(key).is_none() {
                self.resolutions.follow(clock, |r| r.forget(key));
            }
        }
        Ok(removed)
    }

    /// Drop a baseline slot's override or tombstone. Returns whether there
    /// was one.
    pub fn reattach_slot(&mut self, clock: &mut Clock, key: &str) -> bool {
        let before = self.slots.get().clone();
        let reattached = self.slots.follow(clock, |slots| {
            slots.reattach(key);
        });
        if reattached {
            self.follow_unowned(clock);
            self.forget_resolution_if_edited(clock, key, &before);
        }
        reattached
    }

    /// Set the user's name over the provider's, or drop it with `None`.
    /// Returns whether it changed.
    pub fn set_name(&mut self, clock: &mut Clock, name: Option<String>) -> bool {
        self.slots.follow(clock, |slots| slots.name = name)
    }

    /// Set the user's mood over the provider's, or drop it with `None`.
    pub fn set_mood(&mut self, clock: &mut Clock, mood: Option<String>) -> bool {
        self.slots.follow(clock, |slots| slots.mood = mood)
    }

    /// Forget a departed slot once the host has dealt with its instance.
    pub fn release_departed(&mut self, clock: &mut Clock, key: &str) -> bool {
        self.slots.follow(clock, |slots| {
            slots.release_departed(key);
        })
    }

    /// Commit the host's whole arrangement at `expected`, validated against
    /// the slot set and reconciled (see [`Arrangement::commit`]).
    pub fn commit_arrangement(&mut self, clock: &mut Clock, doc: ArrangementDoc, expected: u64) -> Result<ArrangementCommit, CommitError> {
        let slots = self.slots.get().slots();
        let baseline = self.slots.get().baseline.as_ref();
        let (generation, placed) =
            self.arrangement.update(Some(expected), clock, |stored| Ok(Arrangement::commit(stored, doc, baseline, &slots)?))?;
        let stored = self.arrangement.get().as_ref().expect("committed");
        Ok(ArrangementCommit { generation, placed, gone: stored.gone(&slots).into_iter().collect(), owned: stored.owned })
    }

    /// Settle a provider change to an owned arrangement at `expected`.
    /// Returns the generation after.
    pub fn resolve_arrangement(&mut self, clock: &mut Clock, choice: Resolve, expected: u64) -> Result<u64, CommitError> {
        let slots = self.slots.get().slots();
        let baseline = self.slots.get().baseline.as_ref();
        let (generation, ()) = self.arrangement.update(Some(expected), clock, |stored| {
            Arrangement::resolve(stored, choice, baseline, &slots);
            Ok(())
        })?;
        Ok(generation)
    }

    pub fn arrangement_view(&self) -> ArrangementView {
        Arrangement::view(&self.arrangement, &self.slots.get().slots())
    }

    pub fn overlay(&self) -> OverlayView {
        OverlayView::new(self.slots.get(), self.arrangement.get().as_ref())
    }

    /// The saved resolution for a slot, if any.
    pub fn resolution(&self, key: &str) -> Result<Option<SavedResolution>, String> {
        self.slot(key)?;
        Ok(self.resolutions.get().get(key).cloned())
    }

    /// Save a slot's portable resolution at the slot's `expected` generation
    /// (0 for none). `applied` is the identity of the slot resolution it
    /// resolves: when the slot is known to resolve to another, it is
    /// invalid. Returns the slot's generation after.
    pub fn set_resolution(
        &mut self,
        clock: &mut Clock,
        managed: &ManagedContent,
        key: &str,
        resolution: PortableResolution,
        applied: &str,
        expected: u64,
    ) -> Result<u64, CommitError> {
        let slot = self.slot(key)?;
        if let Some(DesiredContent::Ready(desired)) = managed.desired(&self.slot_source(&slot)) {
            if desired.id() != applied {
                return Err(format!("slot {key:?} no longer resolves to what was applied: plan it again").into());
            }
        }
        let stamp = clock.next();
        let (_, generation) = self.resolutions.update(None, clock, |saved| saved.set(key, resolution, applied, expected, stamp))?;
        Ok(generation)
    }

    /// Clear a slot's saved resolution at its `expected` generation.
    /// Returns whether there was one.
    pub fn clear_resolution(&mut self, clock: &mut Clock, key: &str, expected: u64) -> Result<bool, CommitError> {
        let (_, cleared) = self.resolutions.update(None, clock, |saved| saved.clear(key, expected))?;
        Ok(cleared)
    }

    /// Clear saved resolutions the slot no longer resolves to: the slot has
    /// gone, or its resolution is known (Ready) and is not the one the
    /// resolution was saved against. Unavailable and Held keep them: expiry
    /// is silence, not a new target. Returns whether any were cleared.
    pub fn invalidate_resolutions(&mut self, clock: &mut Clock, managed: &ManagedContent) -> bool {
        if self.resolutions.get().by_slot.is_empty() {
            return false;
        }
        let sources: BTreeMap<String, SlotSource> =
            self.slots.get().slots().iter().map(|slot| (slot.key.clone(), self.slot_source(slot))).collect();
        self.resolutions.follow(clock, |saved| {
            saved.retain(|key, saved| {
                let Some(source) = sources.get(key) else {
                    return false;
                };
                match managed.desired(source) {
                    Some(DesiredContent::Ready(desired)) => desired.id() == saved.against,
                    _ => true,
                }
            })
        })
    }

    /// Slot keys, for checking what a commit or a pin names.
    pub fn slot_keys(&self) -> BTreeSet<String> {
        self.slots.get().keys()
    }

    /// A slot whose content the user edited, or which went, loses its saved
    /// portable resolution: it was made for other content.
    fn forget_resolution_if_edited(&mut self, clock: &mut Clock, key: &str, before: &WorkspaceSlots) {
        let spec = |slots: &WorkspaceSlots| slots.slot(key).map(|slot| slot.spec);
        if spec(self.slots.get()) != spec(before) {
            self.resolutions.follow(clock, |r| r.forget(key));
        }
    }

    /// Re-derive an arrangement the user doesn't own after a slot edit that
    /// hides or shows a baseline slot. An owned one is left to the host.
    fn follow_unowned(&mut self, clock: &mut Clock) {
        if self.arrangement.get().as_ref().is_some_and(|a| !a.owned) {
            self.follow_slots(clock);
        }
    }

    /// Bring the arrangement up to date with the slots, storing one if none
    /// is stored yet. Returns whether it changed.
    fn follow_slots(&mut self, clock: &mut Clock) -> bool {
        let slots = self.slots.get().slots();
        let baseline = self.slots.get().baseline.as_ref();
        self.arrangement.follow(clock, |stored| Arrangement::follow(stored, baseline, &slots))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetadataValue;

    #[test]
    fn records_round_trip_at_generations_no_earlier_state_had() {
        let mut clock = Clock::default();
        let mut record = WorkspaceRecord::default();
        let vessel = EntityRef::new("sub-1", "vessel", "v");
        record.follow_subject(
            &mut clock,
            Subject {
                entity: Some(vessel.clone()),
                retained: BTreeMap::from([(
                    vessel,
                    SubjectRecord {
                        label: Some("Worker".into()),
                        ended: true,
                        last_seen_ms: Some(102),
                        facts: BTreeMap::from([("flotilla.project".into(), MetadataValue::Text("p".into()))]),
                    },
                )]),
            },
        );
        let read = WorkspaceRecord::decode(&record.encode(), &mut clock).unwrap();
        assert_eq!(read.subject.get(), record.subject.get());
        assert_eq!(read.encode(), record.encode());
        assert!(read.generation() > record.generation());
        let error = WorkspaceRecord::decode("[]", &mut clock).unwrap_err();
        assert!(error.contains("not a workspace record"), "{error}");
    }
}
