//! Reconciliation of each Slot's content: Andamento plans changes with
//! tokens, and the host prepares the runtime instance, validates the token,
//! commits and acknowledges. Bindings are keyed by workspace and slot key;
//! managed primary content is the `primary` slot, and its ABI 2 calls
//! ([`ManagedContent::plan`] and friends) are wrappers over the slot calls.
//! Missing facts suspend updates; they never mean delete the existing content.
use crate::{
    suggested_layout::{CommandLine, LocalRecipe, RebindPolicy, PRIMARY_SLOT},
    EntityRef, WorkspaceId,
};
use std::collections::{BTreeMap, BTreeSet};

/// The `primary` slot's resolution as managed primary content describes it:
/// a shell command, its working directory, and the target it resolves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalContent {
    pub target: String,
    pub command: String,
    pub cwd: Option<String>,
}

/// What a slot's content resolves to now: the recipe a frontend runs, and
/// the identity of the backing instance, which changes whenever it is
/// replaced, even if the recipe does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub target: Option<String>,
    pub recipe: LocalRecipe,
}

impl Resolved {
    /// An opaque, stable identity for this resolution. A host records the
    /// identity of what it applied and passes it back to plan, which reports
    /// Current when it matches the desired resolution.
    pub fn id(&self) -> String {
        let mut out = String::new();
        let mut field = |value: &str| {
            out.push_str(&value.len().to_string());
            out.push(':');
            out.push_str(value);
        };
        field(self.target.as_deref().unwrap_or(""));
        match &self.recipe {
            LocalRecipe::Command { line, cwd } => {
                match line {
                    CommandLine::Shell(shell) => {
                        field("shell");
                        field(shell);
                    }
                    CommandLine::Argv(argv) => {
                        field("argv");
                        field(&argv.len().to_string());
                        argv.iter().for_each(|arg| field(arg));
                    }
                }
                match cwd {
                    Some(cwd) => {
                        field("cwd");
                        field(cwd);
                    }
                    None => field("no-cwd"),
                }
            }
            LocalRecipe::File { path } => {
                field("file");
                field(path);
            }
            LocalRecipe::Url { url } => {
                field("url");
                field(url);
            }
            LocalRecipe::Jackstay { launcher, endpoint } => {
                field("jackstay");
                field(launcher);
                field(endpoint);
            }
        }
        out
    }

    /// The target, shell command and cwd of a command resolution with a
    /// target, as managed primary content describes it.
    pub fn as_terminal(&self) -> Option<(&str, &str, Option<&str>)> {
        match (&self.target, &self.recipe) {
            (
                Some(target),
                LocalRecipe::Command {
                    line: CommandLine::Shell(command),
                    cwd,
                },
            ) => Some((target, command, cwd.as_deref())),
            _ => None,
        }
    }
}

impl From<TerminalContent> for Resolved {
    fn from(content: TerminalContent) -> Self {
        Self {
            target: Some(content.target),
            recipe: LocalRecipe::Command {
                line: CommandLine::Shell(content.command),
                cwd: content.cwd,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesiredContent {
    Ready(Resolved),
    Held,
}

/// Where a slot's desired resolution comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotSource {
    /// Published by a provider: `slot` of the Suggested Layout `entity`
    /// suggests (`primary` for the `workspace.primary.*` facts).
    Provider { entity: EntityRef, slot: String },
    /// A local recipe, which is its own resolution.
    Local(LocalRecipe),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentUpdate {
    pub token: u64,
    pub content: TerminalContent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotUpdate {
    pub token: u64,
    pub resolution: Resolved,
    /// `resolution.id()`: what the host records once it has applied it.
    pub id: String,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotPlan {
    pub state: ContentState,
    /// Present only while Updating.
    pub update: Option<SlotUpdate>,
    /// What the host does with the instance it replaces: `Replace` closes
    /// it, `KeepPrevious` keeps it reachable (see `previous`), and `Ask`
    /// asks the user first; declining is completing the update unsuccessfully.
    pub rebind: RebindPolicy,
    /// Under `KeepPrevious`, the identity of the resolution a committed
    /// rebind replaced, until the host releases that instance.
    pub previous: Option<String>,
    /// The portable Target Resolution saved with the workspace for this
    /// slot, if any. It was made for the resolution whose identity is its
    /// `against`: while Updating to that resolution (a host starting with
    /// no instance), try connecting through it before resolving the target
    /// afresh. A saved resolution the slot no longer resolves to is cleared,
    /// so it is never one for other content.
    pub saved: Option<crate::target_resolution::SavedResolution>,
}

struct Binding {
    source: SlotSource,
    rebind: RebindPolicy,
    applied: String,
    pending: Option<SlotUpdate>,
    failed: bool,
    previous: Option<String>,
}

type BindingKey = (WorkspaceId, String);

#[derive(Default)]
pub struct ManagedContent {
    desired: BTreeMap<(EntityRef, String), DesiredContent>,
    bindings: BTreeMap<BindingKey, Binding>,
    next_token: u64,
}

impl ManagedContent {
    /// Replace the provider-published resolutions. A binding whose desired
    /// resolution changed loses its pending token and its failure. Returns
    /// whether any resolution changed.
    pub fn publish(&mut self, desired: BTreeMap<(EntityRef, String), DesiredContent>) -> bool {
        if desired == self.desired {
            return false;
        }
        for binding in self.bindings.values_mut() {
            if let SlotSource::Provider { entity, slot } = &binding.source {
                let key = (entity.clone(), slot.clone());
                if self.desired.get(&key) != desired.get(&key) {
                    binding.pending = None;
                    binding.failed = false;
                }
            }
        }
        self.desired = desired;
        true
    }

    /// The resolution `source` has now; `None` is Unavailable.
    pub fn desired(&self, source: &SlotSource) -> Option<DesiredContent> {
        match source {
            SlotSource::Provider { entity, slot } => {
                self.desired.get(&(entity.clone(), slot.clone())).cloned()
            }
            SlotSource::Local(recipe) => Some(DesiredContent::Ready(Resolved {
                target: None,
                recipe: recipe.clone(),
            })),
        }
    }

    pub fn retain_workspaces(&mut self, ids: &[WorkspaceId]) {
        let live: BTreeSet<_> = ids.iter().copied().collect();
        self.bindings.retain(|(id, _), _| live.contains(id));
    }

    /// Forget one slot's binding, as when the slot is removed.
    pub fn forget_slot(&mut self, workspace: WorkspaceId, slot: &str) {
        self.bindings.remove(&(workspace, slot.to_owned()));
    }

    /// Plan `slot` of `workspace`, whose content comes from `source`, given
    /// the identity of the resolution the host applied (empty for none).
    pub fn plan_slot(
        &mut self,
        workspace: WorkspaceId,
        slot: &str,
        source: SlotSource,
        rebind: RebindPolicy,
        applied: &str,
    ) -> SlotPlan {
        let desired = self.desired(&source);
        let binding = self
            .bindings
            .entry((workspace, slot.to_owned()))
            .or_insert_with(|| Binding {
                source: source.clone(),
                rebind,
                applied: applied.to_owned(),
                pending: None,
                failed: false,
                previous: None,
            });
        if binding.source != source || binding.applied != applied {
            let previous = binding.previous.take().filter(|_| binding.source == source);
            *binding = Binding {
                source,
                rebind,
                applied: applied.to_owned(),
                pending: None,
                failed: false,
                previous,
            };
        }
        binding.rebind = rebind;
        let state = match &desired {
            None => ContentState::Unavailable,
            Some(DesiredContent::Held) => ContentState::Held,
            Some(DesiredContent::Ready(content)) if content.id() == binding.applied => {
                ContentState::Current
            }
            Some(DesiredContent::Ready(_)) if binding.failed => ContentState::Failed,
            Some(DesiredContent::Ready(content)) => {
                if binding
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.resolution != *content)
                {
                    self.next_token += 1;
                    binding.pending = Some(SlotUpdate {
                        token: self.next_token,
                        id: content.id(),
                        resolution: content.clone(),
                    });
                }
                ContentState::Updating
            }
        };
        SlotPlan {
            state,
            update: binding
                .pending
                .clone()
                .filter(|_| state == ContentState::Updating),
            rebind: binding.rebind,
            previous: binding.previous.clone(),
            saved: None,
        }
    }

    /// Host calls immediately before mutation, on the same owner thread as publication.
    pub fn slot_valid(&self, workspace: WorkspaceId, slot: &str, token: u64) -> bool {
        self.bindings
            .get(&(workspace, slot.to_owned()))
            .is_some_and(|b| {
                b.pending.as_ref().is_some_and(|u| {
                    u.token == token
                        && self.desired(&b.source)
                            == Some(DesiredContent::Ready(u.resolution.clone()))
                })
            })
    }

    pub fn complete_slot(
        &mut self,
        workspace: WorkspaceId,
        slot: &str,
        token: u64,
        success: bool,
    ) -> bool {
        if !self.slot_valid(workspace, slot, token) {
            return false;
        }
        let binding = self
            .bindings
            .get_mut(&(workspace, slot.to_owned()))
            .unwrap();
        let update = binding.pending.take().unwrap();
        if success {
            let replaced = std::mem::replace(&mut binding.applied, update.id);
            if binding.rebind == RebindPolicy::KeepPrevious && !replaced.is_empty() {
                binding.previous = Some(replaced);
            }
        } else {
            binding.failed = true;
        }
        true
    }

    pub fn retry_slot(&mut self, workspace: WorkspaceId, slot: &str) {
        if let Some(binding) = self.bindings.get_mut(&(workspace, slot.to_owned())) {
            binding.failed = false;
        }
    }

    /// The host closed the instance a `KeepPrevious` rebind kept. Returns
    /// whether there was one.
    pub fn release_previous(&mut self, workspace: WorkspaceId, slot: &str) -> bool {
        self.bindings
            .get_mut(&(workspace, slot.to_owned()))
            .and_then(|binding| binding.previous.take())
            .is_some()
    }

    /// ABI 2: plan the `primary` slot, whose resolution `entity` publishes.
    pub fn plan(
        &mut self,
        workspace: WorkspaceId,
        entity: EntityRef,
        applied: TerminalContent,
    ) -> ContentPlan {
        let source = SlotSource::Provider {
            entity,
            slot: PRIMARY_SLOT.to_owned(),
        };
        let applied = Resolved::from(applied).id();
        let plan = self.plan_slot(
            workspace,
            PRIMARY_SLOT,
            source,
            RebindPolicy::Replace,
            &applied,
        );
        ContentPlan {
            state: plan.state,
            update: plan.update.and_then(|update| {
                let (target, command, cwd) = update.resolution.as_terminal()?;
                Some(ContentUpdate {
                    token: update.token,
                    content: TerminalContent {
                        target: target.to_owned(),
                        command: command.to_owned(),
                        cwd: cwd.map(str::to_owned),
                    },
                })
            }),
        }
    }

    pub fn valid(&self, workspace: WorkspaceId, token: u64) -> bool {
        self.slot_valid(workspace, PRIMARY_SLOT, token)
    }

    pub fn complete(&mut self, workspace: WorkspaceId, token: u64, success: bool) -> bool {
        self.complete_slot(workspace, PRIMARY_SLOT, token, success)
    }

    pub fn retry(&mut self, workspace: WorkspaceId) {
        self.retry_slot(workspace, PRIMARY_SLOT)
    }
}
