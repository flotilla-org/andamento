//! Reconciliation of one host-owned primary slot per workspace intent.
//! Missing facts suspend updates; they never mean delete the existing content.
use crate::EntityRef;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalContent {
    pub target: String,
    pub command: String,
    pub cwd: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesiredContent {
    Ready(TerminalContent),
    Held,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentUpdate {
    pub token: u64,
    pub content: TerminalContent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentState {
    Unavailable,
    Held,
    Current,
    Updating,
    Failed,
}
#[derive(Clone, Debug)]
pub struct ContentPlan {
    pub state: ContentState,
    pub update: Option<ContentUpdate>,
}
struct Binding {
    entity: EntityRef,
    applied: TerminalContent,
    pending: Option<ContentUpdate>,
    failed: bool,
}
#[derive(Default)]
pub struct ManagedContent {
    desired: BTreeMap<EntityRef, DesiredContent>,
    bindings: BTreeMap<u64, Binding>,
    next_token: u64,
}
impl ManagedContent {
    pub fn publish(&mut self, desired: BTreeMap<EntityRef, DesiredContent>) {
        for binding in self.bindings.values_mut() {
            if self.desired.get(&binding.entity) != desired.get(&binding.entity) {
                binding.pending = None;
                binding.failed = false;
            }
        }
        self.desired = desired;
    }
    pub fn retain_workspaces(&mut self, ids: &[u64]) {
        let live: BTreeSet<_> = ids.iter().copied().collect();
        self.bindings.retain(|id, _| live.contains(id));
    }
    pub fn plan(
        &mut self,
        workspace: u64,
        entity: EntityRef,
        applied: TerminalContent,
    ) -> ContentPlan {
        let binding = self.bindings.entry(workspace).or_insert_with(|| Binding {
            entity: entity.clone(),
            applied: applied.clone(),
            pending: None,
            failed: false,
        });
        if binding.entity != entity || binding.applied != applied {
            *binding = Binding {
                entity,
                applied,
                pending: None,
                failed: false,
            };
        }
        let state = match self.desired.get(&binding.entity) {
            None => ContentState::Unavailable,
            Some(DesiredContent::Held) => ContentState::Held,
            Some(DesiredContent::Ready(content)) if *content == binding.applied => {
                ContentState::Current
            }
            Some(DesiredContent::Ready(_)) if binding.failed => ContentState::Failed,
            Some(DesiredContent::Ready(content)) => {
                if binding.pending.is_none() {
                    self.next_token += 1;
                    binding.pending = Some(ContentUpdate {
                        token: self.next_token,
                        content: content.clone(),
                    });
                }
                ContentState::Updating
            }
        };
        ContentPlan {
            state,
            update: binding
                .pending
                .clone()
                .filter(|_| state == ContentState::Updating),
        }
    }
    /// Host calls immediately before mutation, on the same owner thread as publication.
    pub fn valid(&self, workspace: u64, token: u64) -> bool {
        self.bindings.get(&workspace).is_some_and(|b| {
            b.pending.as_ref().is_some_and(|u| {
                u.token == token
                    && self.desired.get(&b.entity)
                        == Some(&DesiredContent::Ready(u.content.clone()))
            })
        })
    }
    pub fn complete(&mut self, workspace: u64, token: u64, success: bool) -> bool {
        if !self.valid(workspace, token) {
            return false;
        }
        let binding = self.bindings.get_mut(&workspace).unwrap();
        let update = binding.pending.take().unwrap();
        if success {
            binding.applied = update.content;
        } else {
            binding.failed = true;
        }
        true
    }
    pub fn retry(&mut self, workspace: u64) {
        if let Some(binding) = self.bindings.get_mut(&workspace) {
            binding.failed = false;
        }
    }
}
