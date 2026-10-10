//! One presentation client's runtime. Hosts supply time and observations and
//! execute returned effects. Transport and rendering do not run inside it.
use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, Serialize};

use crate::{
    host::PaneObservation,
    presentation::SurfaceSnapshot,
    records::{DashboardRecord, RecordName, SubjectRecord, WorkspaceRecord},
    state::{ControllerState, ControllerTab, EntityActivation},
    EntityRef, MaterializeLatentRequest, MetadataPatch, NodeKey, PlacementKey, RailUiAction,
    WorkspaceId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub position: usize,
    pub name: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    CopySubjectUrl {
        entity: EntityRef,
    },
    /// Activate this appearance, focusing its exact workspace when already live.
    ActivatePlacement {
        key: PlacementKey,
    },
    Activate {
        entity: EntityRef,
    },
    TogglePlacement {
        key: PlacementKey,
    },
    SetVariable {
        key: PlacementKey,
        name: String,
        value: Option<String>,
    },
    ToggleDisplayVariable {
        name: String,
    },
}

/// In-process/FFI messages for a presentation client. This is separate from
/// the producer metadata-patch protocol and its HTTP/UDS transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub enum Request {
    Configure {
        kdl: String,
    },
    /// Apply patches from `provider` (a subscription ID), or from the
    /// default provider when it is absent.
    Apply {
        now_ms: u64,
        patches: Vec<MetadataPatch>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    SetDefaultProvider {
        provider: String,
    },
    RetractProvider {
        provider: String,
    },
    SetProviderStale {
        provider: String,
        stale: bool,
    },
    Observe {
        workspaces: Vec<Workspace>,
        panes: Vec<PaneObservation>,
    },
    Dispatch {
        action: Action,
    },
    Complete {
        request_id: u64,
        workspace_id: Option<WorkspaceId>,
        error: Option<String>,
    },
    RegisterWorkspace {
        workspace_id: WorkspaceId,
    },
    ForgetWorkspace {
        workspace_id: WorkspaceId,
    },
    SetDisplayVariable {
        name: String,
        value: Option<crate::DisplayVariableValue>,
    },
    SetLocal {
        entity: EntityRef,
        facts: BTreeMap<String, crate::MetadataValue>,
    },
    RemoveLocal {
        entity: EntityRef,
    },
    ImportRecord {
        name: String,
        kdl: String,
    },
    Snapshot,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub snapshot: Snapshot,
    pub effects: Vec<HostEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "kebab-case")]
pub enum HostEffect {
    OpenUrl {
        url: String,
    },
    CopyUrl {
        url: String,
    },
    Focus {
        request_id: u64,
        workspace_id: WorkspaceId,
    },
    Materialize {
        request_id: u64,
        entity: EntityRef,
        name: String,
        recipe: String,
        cwd: Option<String>,
        /// Managed-content target this recipe resolves, when the entity opts
        /// into managed primary content and is ready. Hosts record it as the
        /// applied target so the first reconciliation sees current content.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        primary_target: Option<String>,
    },
    /// No materialization is available; the frontend can show its inspector.
    Inspect {
        entity: EntityRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationError {
    pub entity: EntityRef,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub surface: SurfaceSnapshot,
    pub errors: Vec<ActivationError>,
}

enum Pending {
    Focus(EntityRef),
    Materialize(EntityRef, MaterializeLatentRequest),
}

#[derive(Default)]
pub struct Sidebar {
    state: ControllerState,
    revision: u64,
    snapshot: OnceCell<Snapshot>,
    evaluation: OnceCell<crate::state::RevisionEvaluation>,
    details: RefCell<BTreeMap<EntityRef, Option<crate::detail::DetailCard>>>,
    next_request: u64,
    pending: BTreeMap<u64, Pending>,
    errors: BTreeMap<EntityRef, String>,
    retained_paths: BTreeMap<WorkspaceId, BTreeSet<EntityRef>>,
    retained_paths_revision: Option<u64>,
    registered: BTreeSet<WorkspaceId>,
    /// Each registered workspace's record (`workspace/<id>`).
    workspaces: BTreeMap<WorkspaceId, WorkspaceEntry>,
    /// Local sections, groups and refs, with their facts.
    local: BTreeMap<EntityRef, BTreeMap<String, crate::MetadataValue>>,
    /// Nodes of the imported dashboard record this version doesn't know.
    dashboard_unknown: Vec<String>,
    /// Each record's generation and the text it was assigned for.
    generations: BTreeMap<RecordName, (u64, String)>,
    last_generation: u64,
    /// Fact keys the configured placement reads, kept in subject records.
    placement_keys: BTreeSet<String>,
    pub managed: crate::managed::ManagedContent,
    /// Revision of workspace content: slots, their resolutions and
    /// arrangements. Separate from `revision`, so committing an arrangement
    /// leaves the sidebar's snapshot and evaluation current.
    content_revision: u64,
}

#[derive(Default)]
struct WorkspaceEntry {
    record: WorkspaceRecord,
    /// Retained subjects a producer published at the last refresh.
    observed: BTreeSet<EntityRef>,
}

/// The kinds of local entities: sections, groups and refs people make, which
/// Andamento owns and keeps in the dashboard record.
pub const LOCAL_KINDS: [&str; 3] = [
    crate::presentation::system::SECTION,
    crate::presentation::system::GROUP,
    crate::presentation::system::REF,
];
const SOURCE_LOCAL: &str = "andamento-local";

impl Sidebar {
    pub fn handle(&mut self, request: Request) -> Result<Response, String> {
        let effects = match request {
            Request::Configure { kdl } => {
                self.configure(&kdl)?;
                vec![]
            }
            Request::Apply {
                now_ms,
                patches,
                provider,
            } => {
                match provider {
                    Some(provider) => self.apply_from(now_ms, &provider, patches)?,
                    None => self.apply(now_ms, patches),
                }
                vec![]
            }
            Request::SetDefaultProvider { provider } => {
                self.set_default_provider(&provider)?;
                vec![]
            }
            Request::RetractProvider { provider } => {
                self.retract_provider(&provider);
                vec![]
            }
            Request::SetProviderStale { provider, stale } => {
                self.set_provider_stale(&provider, stale);
                vec![]
            }
            Request::Observe { workspaces, panes } => {
                self.observe(workspaces, panes);
                vec![]
            }
            Request::Dispatch { action } => self.dispatch(action)?,
            Request::Complete {
                request_id,
                workspace_id,
                error,
            } => {
                self.complete(
                    request_id,
                    match error {
                        Some(error) => Err(error),
                        None => Ok(workspace_id),
                    },
                );
                vec![]
            }
            Request::RegisterWorkspace { workspace_id } => {
                self.register_workspace(workspace_id);
                vec![]
            }
            Request::ForgetWorkspace { workspace_id } => {
                self.forget_workspace(workspace_id);
                vec![]
            }
            Request::SetDisplayVariable { name, value } => {
                self.set_display_variable(&name, value)?;
                vec![]
            }
            Request::SetLocal { entity, facts } => {
                self.set_local(entity, facts)?;
                vec![]
            }
            Request::RemoveLocal { entity } => {
                self.remove_local(&entity);
                vec![]
            }
            Request::ImportRecord { name, kdl } => {
                self.import_record(&name, &kdl)?;
                vec![]
            }
            Request::Snapshot => vec![],
        };
        Ok(Response {
            snapshot: self.snapshot(),
            effects,
        })
    }

    pub fn new(config_kdl: &str) -> Result<Self, String> {
        let mut sidebar = Self::default();
        sidebar.configure(config_kdl)?;
        Ok(sidebar)
    }

    /// Validate the template catalog before replacing the active configuration.
    pub fn configure(&mut self, config_kdl: &str) -> Result<(), String> {
        let templates = crate::template_config::parse_template_config_kdl(config_kdl)
            .map_err(|e| e.to_string())?;
        self.state.set_template_catalog(Some(
            crate::template_config::TemplateConfigCatalog::with_bundled_defaults(templates),
        ));
        self.placement_keys = self.state.placement_fact_keys();
        self.invalidate();
        Ok(())
    }

    /// The provider of entities that name none: the one [`apply`](Self::apply)
    /// stamps, and the one entities named by kind and ID alone belong to.
    /// It is [`LOCAL_PROVIDER`](crate::LOCAL_PROVIDER) until the host sets it.
    pub fn default_provider(&self) -> &str {
        self.state.default_provider()
    }

    /// Set the default provider, so a host can move to providers gradually.
    /// Set it before applying facts or importing records.
    pub fn set_default_provider(&mut self, provider: &str) -> Result<(), String> {
        self.state.set_default_provider(provider)?;
        self.invalidate();
        Ok(())
    }

    /// Apply a drained batch before requesting a snapshot. Time is monotonic
    /// milliseconds in this instance, supplied by the host; an empty batch is a tick.
    /// The patches come from the default provider.
    pub fn apply(&mut self, now_ms: u64, patches: impl IntoIterator<Item = MetadataPatch>) {
        let provider = self.state.default_provider().to_owned();
        self.apply_patches(now_ms, &provider, patches);
    }

    /// Apply a drained batch from `provider`, the subscription ID the host
    /// received it on. The provider is stamped over every entity the patches
    /// name; a patch never names its own.
    pub fn apply_from(
        &mut self,
        now_ms: u64,
        provider: &str,
        patches: impl IntoIterator<Item = MetadataPatch>,
    ) -> Result<(), String> {
        if provider.is_empty() {
            return Err("a provider needs a name".into());
        }
        self.apply_patches(now_ms, provider, patches);
        Ok(())
    }

    /// Remove all of a provider's facts at once, as when its subscription is
    /// removed. What open workspaces and refs showed of its entities is
    /// retained, as when facts expire: a workspace keeps its subject and its
    /// row stays where it was, and a ref keeps presenting its target.
    /// Returns whether anything changed.
    pub fn retract_provider(&mut self, provider: &str) -> bool {
        let subjects = self.state.workspace_subjects();
        if !subjects.is_empty() {
            // Paths come from the current snapshot; make sure there is one.
            self.snapshot_shared();
        }
        let mut changed = self.retain_workspace_paths(&subjects);
        // A workspace bound to its subject by the provider's own facts keeps
        // the binding.
        for (id, subject) in &subjects {
            if subject.provider == provider {
                changed |= self.state.bind_workspace_subject(*id, Some(subject));
            }
        }
        changed |= self.state.retract_provider(provider);
        self.maintain(changed);
        changed
    }

    /// Mark a provider stale, when its connection drops, or fresh again when
    /// it returns. A stale provider's facts are kept as they were and none
    /// expires while it stays stale; snapshot nodes presenting its entities
    /// are flagged stale. Fresh again, each TTL fact that was live then
    /// renews its lease from now, as though the provider had reasserted it,
    /// so one it doesn't reassert expires a TTL later. Returns whether the
    /// provider's state changed.
    pub fn set_provider_stale(&mut self, provider: &str, stale: bool) -> bool {
        let changed = self.state.set_provider_stale(provider, stale);
        if changed {
            self.invalidate();
        }
        changed
    }

    pub fn provider_is_stale(&self, provider: &str) -> bool {
        self.state.provider_is_stale(provider)
    }

    fn apply_patches(
        &mut self,
        now_ms: u64,
        provider: &str,
        patches: impl IntoIterator<Item = MetadataPatch>,
    ) {
        let _phase = crate::profile::span("apply");
        let maintenance = crate::profile::span("workspace-maintenance");
        let subjects = self.state.workspace_subjects();
        let mut changed = self.retain_workspace_paths(&subjects);
        drop(maintenance);
        changed |= self.state.advance_time(now_ms);
        let patch_phase = crate::profile::span("patch-application");
        for patch in patches {
            changed |= self.state.apply_provider_patch(provider, patch);
        }
        drop(patch_phase);
        let maintenance = crate::profile::span("workspace-maintenance");
        changed |= self.state.refresh_retained_subjects();
        changed |= self
            .state
            .mark_ended_workspace_paths(&self.retained_paths, &subjects);
        drop(maintenance);
        if changed {
            self.invalidate();
        }
        self.refresh_workspace_records();
        if changed {
            self.refresh_content();
        }
    }

    fn retain_workspace_paths(&mut self, subjects: &BTreeMap<WorkspaceId, EntityRef>) -> bool {
        fn collect(
            nodes: &[crate::presentation::PlacementNode],
            ancestors: &mut Vec<EntityRef>,
            paths: &mut BTreeMap<WorkspaceId, BTreeSet<EntityRef>>,
        ) {
            for node in nodes {
                // Andamento's own kinds (`.`-prefixed: workspaces, sections,
                // groups, refs) are never retained: the host retracts them, and
                // a ghost must not outlive its removal because its target is
                // open.
                let subject = !crate::presentation::system::is_system(&node.entity.kind);
                if subject {
                    ancestors.push(node.entity.clone());
                }
                if let crate::presentation::PresentationState::Live { workspace_id, .. } =
                    node.state
                {
                    paths
                        .entry(workspace_id)
                        .or_default()
                        .extend(ancestors.iter().cloned());
                }
                collect(&node.children, ancestors, paths);
                if subject {
                    ancestors.pop();
                }
            }
        }
        if self.retained_paths_revision != Some(self.revision) && !subjects.is_empty() {
            if let Some(snapshot) = self.snapshot.get() {
                let mut paths = BTreeMap::new();
                for section in &snapshot.surface.sections {
                    collect(&section.nodes, &mut Vec::new(), &mut paths);
                }
                self.retained_paths
                    .extend(paths.into_iter().filter(|(id, path)| {
                        subjects
                            .get(id)
                            .is_some_and(|subject| path.contains(subject))
                    }));
                self.retained_paths_revision = Some(self.revision);
            }
        }
        // A workspace with no path yet, still open for the subject its record
        // names, takes its recorded path: so a restored row is drawn where it
        // was before any producer publishes it. A path from a snapshot
        // replaces it.
        for (id, entry) in &self.workspaces {
            let record = &entry.record;
            if record.subject.is_some()
                && subjects.get(id) == record.subject.as_ref()
                && !record.retained.is_empty()
                && !self.retained_paths.contains_key(id)
            {
                self.retained_paths
                    .insert(*id, record.retained.keys().cloned().collect());
            }
        }
        self.retained_paths.retain(|id, subjects_on_path| {
            subjects.contains_key(id) && !retained_workspace_expired(&self.state, subjects_on_path)
        });
        let saved = self.saved_subjects();
        let mut retained: BTreeSet<EntityRef> =
            self.retained_paths.values().flatten().cloned().collect();
        retained.extend(self.ref_targets());
        self.state.retain_workspace_paths(retained, &saved)
    }

    /// The entities local refs (pins) present. They are retained like the
    /// subjects on open workspace paths, so a ref keeps presenting its target
    /// when the target's facts go away.
    fn ref_targets(&self) -> impl Iterator<Item = EntityRef> + '_ {
        use crate::presentation::system;
        self.local
            .iter()
            .filter(|(entity, _)| entity.kind == system::REF)
            .filter_map(|(_, facts)| match facts.get(system::TARGET) {
                Some(crate::MetadataValue::EntityRefs(refs)) if refs.len() == 1 => {
                    Some(refs[0].clone())
                }
                _ => None,
            })
            .filter(|target| !system::is_system(&target.kind))
    }

    /// Subjects recorded by every workspace record. Where records disagree, an
    /// end wins, then the latest sighting.
    fn saved_subjects(&self) -> BTreeMap<EntityRef, SubjectRecord> {
        let mut saved = BTreeMap::<EntityRef, SubjectRecord>::new();
        for entry in self.workspaces.values() {
            for (entity, record) in &entry.record.retained {
                let newer = saved.get(entity).is_none_or(|current| {
                    (record.ended, record.last_seen_ms) > (current.ended, current.last_seen_ms)
                });
                if newer {
                    saved.insert(entity.clone(), record.clone());
                }
            }
        }
        saved
    }

    /// Full host topology, including selected workspace. Closing a workspace
    /// removes its local presentation, not the separately published entity.
    pub fn observe(&mut self, workspaces: Vec<Workspace>, panes: Vec<PaneObservation>) {
        self.managed
            .retain_workspaces(&workspaces.iter().map(|w| w.id).collect::<Vec<_>>());
        let mut changed = self.state.observe_workspaces(
            workspaces
                .into_iter()
                .map(|w| ControllerTab {
                    tab_id: w.id,
                    position: w.position,
                    name: w.name,
                    active: w.selected,
                })
                .collect(),
        );
        changed |= self.state.observe_panes(panes);
        let subjects = self.state.workspace_subjects();
        if changed {
            // The first host topology observation establishes an open workspace
            // and captures its subject before any later producer-removal drain.
            // Rebuild paths from the new topology before publishing one revision.
            self.snapshot.take();
            self.evaluation.take();
            self.details.get_mut().clear();
            self.retained_paths_revision = None;
            if !subjects.is_empty() {
                self.snapshot_shared();
            }
        }
        changed |= self.retain_workspace_paths(&subjects);
        changed |= self
            .state
            .mark_ended_workspace_paths(&self.retained_paths, &subjects);
        if changed {
            self.invalidate();
        }
        self.refresh_workspace_records();
        if changed {
            self.refresh_content();
        }
    }

    /// Replace terminal directory observations after supplying workspace topology.
    /// Matching is exact and host-normalized; explicit workspace identity wins.
    /// Several terminal directories may belong to one workspace. Associations
    /// only affect focus/presentation and never enroll content for replacement.
    pub fn observe_workdirs(&mut self, workdirs: Vec<(WorkspaceId, String)>) {
        if self.state.observe_workdirs(workdirs) {
            self.invalidate();
        }
    }

    /// Declare a workspace the host created itself, such as one the user made,
    /// that never went through a materialize effect. A successful materialize
    /// completion registers its workspace too. The host supplies the ID;
    /// Andamento never generates one. Registration is independent of topology:
    /// either may come first, and closing a workspace does not forget it.
    /// It does not change the snapshot or its revision. The workspace gets a
    /// record, `workspace/<id>`. Returns whether the ID was new.
    pub fn register_workspace(&mut self, id: WorkspaceId) -> bool {
        self.workspaces.entry(id).or_default();
        let new = self.registered.insert(id);
        self.refresh_workspace_records();
        self.refresh_content();
        new
    }

    /// The host deleted the workspace rather than keeping it. Returns whether
    /// it was registered.
    pub fn forget_workspace(&mut self, id: WorkspaceId) -> bool {
        self.workspaces.remove(&id);
        self.generations.remove(&RecordName::Workspace(id));
        self.registered.remove(&id)
    }

    /// Workspaces the host registered or materialized and has not forgotten.
    pub fn registered_workspaces(&self) -> &BTreeSet<WorkspaceId> {
        &self.registered
    }

    /// Set the host-owned order of one sibling run, identified by the loop key
    /// every node in it carries. An empty order returns the run to data order.
    pub fn set_sibling_order(
        &mut self,
        mut loop_key: crate::PlacementLoopKey,
        mut order: Vec<EntityRef>,
    ) {
        let provider = self.state.default_provider().to_owned();
        loop_key.fill_provider(&provider);
        for entity in &mut order {
            entity.fill_provider(&provider);
        }
        self.state
            .apply_rail_ui_action(RailUiAction::SetSiblingOrder { loop_key, order });
        self.invalidate();
    }

    /// Set a declared display variable, or return it to its default with
    /// None, without a snapshot action. Unknown variables and values the
    /// declaration doesn't allow are rejected.
    pub fn set_display_variable(
        &mut self,
        name: &str,
        value: Option<crate::DisplayVariableValue>,
    ) -> Result<(), String> {
        if self.state.set_display_variable(name, value)? {
            self.invalidate();
        }
        Ok(())
    }

    /// Set a local section, group or ref (see [`LOCAL_KINDS`]), replacing all
    /// its facts. Andamento owns local entities and keeps them in the
    /// dashboard record; their provider is [`LOCAL_PROVIDER`](crate::LOCAL_PROVIDER).
    /// Lists of text and group paths are not allowed. Entity references that
    /// name no provider get the default provider.
    pub fn set_local(
        &mut self,
        mut entity: EntityRef,
        mut facts: BTreeMap<String, crate::MetadataValue>,
    ) -> Result<(), String> {
        entity.fill_provider(crate::LOCAL_PROVIDER);
        if entity.provider != crate::LOCAL_PROVIDER {
            return Err(format!(
                "local entities have the {:?} provider, not {:?}",
                crate::LOCAL_PROVIDER,
                entity.provider
            ));
        }
        for value in facts.values_mut() {
            value.fill_provider(self.state.default_provider());
        }
        if !LOCAL_KINDS.contains(&entity.kind.as_str()) {
            return Err(format!(
                "local entities are {}, not {:?}",
                LOCAL_KINDS.join(", "),
                entity.kind
            ));
        }
        if entity.id.is_empty() {
            return Err("a local entity needs an ID".into());
        }
        if facts.keys().any(String::is_empty) {
            return Err("a fact needs a key".into());
        }
        if !facts.values().all(crate::records::recordable) {
            return Err("local facts are text, booleans, integers or entity references".into());
        }
        let patch = self.local_patch(&entity, Some(&facts));
        self.local.insert(entity, facts);
        self.apply_local([patch]);
        Ok(())
    }

    /// Remove a local entity. Returns whether there was one.
    pub fn remove_local(&mut self, entity: &EntityRef) -> bool {
        let mut entity = entity.clone();
        entity.fill_provider(crate::LOCAL_PROVIDER);
        let entity = &entity;
        if !self.local.contains_key(entity) {
            return false;
        }
        let patch = self.local_patch(entity, None);
        self.local.remove(entity);
        self.apply_local([patch]);
        true
    }

    /// Local entities, with their facts.
    pub fn local_entities(&self) -> &BTreeMap<EntityRef, BTreeMap<String, crate::MetadataValue>> {
        &self.local
    }

    /// The patch that makes the catalog hold `facts` for a local entity,
    /// unsetting the keys it held before.
    fn local_patch(
        &self,
        entity: &EntityRef,
        facts: Option<&BTreeMap<String, crate::MetadataValue>>,
    ) -> MetadataPatch {
        let empty = BTreeMap::new();
        let facts = facts.unwrap_or(&empty);
        MetadataPatch {
            target: crate::MetadataTarget::Entity(entity.clone()),
            source_id: SOURCE_LOCAL.into(),
            set: facts
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        crate::MetadataValueUpdate {
                            value: value.clone(),
                            ttl_ms: None,
                            precedence: None,
                            ordinal: None,
                        },
                    )
                })
                .collect(),
            unset: self
                .local
                .get(entity)
                .into_iter()
                .flat_map(BTreeMap::keys)
                .filter(|key| !facts.contains_key(*key))
                .cloned()
                .collect(),
        }
    }

    /// Apply local patches at the current time, without advancing it.
    fn apply_local(&mut self, patches: impl IntoIterator<Item = MetadataPatch>) {
        let mut changed = false;
        for patch in patches {
            changed |= self.state.apply_metadata_patch(patch);
        }
        self.maintain(changed);
    }

    /// The upkeep `apply` does after its patches, for changes that come from
    /// elsewhere (local entities, imported records). It never advances time.
    fn maintain(&mut self, mut changed: bool) {
        let subjects = self.state.workspace_subjects();
        changed |= self.retain_workspace_paths(&subjects);
        changed |= self.state.refresh_retained_subjects();
        changed |= self
            .state
            .mark_ended_workspace_paths(&self.retained_paths, &subjects);
        if changed {
            self.invalidate();
        }
        self.refresh_workspace_records();
        if changed {
            self.refresh_content();
        }
    }

    /// Bring the records of open, registered workspaces up to date: their
    /// subject and its path, as retained now. A closed workspace keeps the
    /// record it had when it closed.
    fn refresh_workspace_records(&mut self) {
        let open = self
            .state
            .workspaces()
            .iter()
            .map(|tab| tab.tab_id)
            .filter(|id| self.workspaces.contains_key(id))
            .collect::<Vec<_>>();
        if open.is_empty() {
            return;
        }
        let subjects = self.state.workspace_subjects();
        let keys = &self.placement_keys;
        let now = self.state.evaluation_time();
        for id in open {
            let path = self.retained_paths.get(&id).cloned().unwrap_or_default();
            let entry = self
                .workspaces
                .get_mut(&id)
                .expect("open IDs are registered");
            let mut retained = BTreeMap::new();
            let mut observed = BTreeSet::new();
            for entity in path {
                let Some((mut record, seen)) = self.state.retained_subject_record(&entity, keys)
                else {
                    if let Some(old) = entry.record.retained.get(&entity) {
                        retained.insert(entity, old.clone());
                    }
                    continue;
                };
                let old = entry.record.retained.get(&entity);
                // Last seen moves when the subject appears, changes or goes
                // away, not on every heartbeat.
                record.last_seen_ms = match old {
                    Some(old)
                        if (&old.label, old.ended, &old.facts)
                            == (&record.label, record.ended, &record.facts)
                            && entry.observed.contains(&entity) == seen =>
                    {
                        old.last_seen_ms
                    }
                    _ => Some(now),
                };
                if seen {
                    observed.insert(entity.clone());
                }
                retained.insert(entity, record);
            }
            entry.record.subject = subjects.get(&id).cloned();
            entry.record.retained = retained;
            entry.observed = observed;
        }
    }

    /// Names of the records Andamento holds: `dashboard`, and
    /// `workspace/<id>` for each registered workspace.
    pub fn record_names(&self) -> Vec<String> {
        std::iter::once(RecordName::Dashboard)
            .chain(self.registered.iter().copied().map(RecordName::Workspace))
            .map(|name| name.to_string())
            .collect()
    }

    fn record_text(&self, name: RecordName) -> Result<String, String> {
        match name {
            RecordName::Dashboard => Ok(DashboardRecord {
                display: self.state.recorded_display_variables(),
                collapsed: self.state.collapsed_placements().clone(),
                orders: self.state.sibling_orders().clone(),
                variables: self.state.placement_variables(),
                local: self.local.clone(),
                unknown: self.dashboard_unknown.clone(),
            }
            .encode()),
            RecordName::Workspace(id) => self
                .workspaces
                .get(&id)
                .map(|entry| entry.record.encode(id))
                .ok_or_else(|| format!("no record {name}: the workspace is not registered")),
        }
    }

    /// Export a record as KDL in a versioned envelope.
    pub fn export_record(&mut self, name: &str) -> Result<String, String> {
        let name = RecordName::parse(name)?;
        let text = self.record_text(name)?;
        self.note_generation(name, &text);
        Ok(text)
    }

    /// A record's generation, nonzero. It changes when, and only when, the
    /// record's content changes; generations are never reused, even by a
    /// workspace forgotten and registered again. Write the record when it
    /// differs from the one last written.
    pub fn record_generation(&mut self, name: &str) -> Result<u64, String> {
        let name = RecordName::parse(name)?;
        let text = self.record_text(name)?;
        Ok(self.note_generation(name, &text))
    }

    fn note_generation(&mut self, name: RecordName, text: &str) -> u64 {
        if let Some((generation, known)) = self.generations.get(&name) {
            if known == text {
                return *generation;
            }
        }
        self.last_generation += 1;
        self.generations
            .insert(name, (self.last_generation, text.to_owned()));
        self.last_generation
    }

    /// Import a record exported by this or an earlier Andamento. It may come
    /// before the first observation and needs no facts. A record that doesn't
    /// parse, has another version, or names another record is rejected
    /// without changing anything. Importing `workspace/<id>` registers the
    /// workspace and binds it to its recorded subject.
    pub fn import_record(&mut self, name: &str, kdl: &str) -> Result<(), String> {
        match RecordName::parse(name)? {
            RecordName::Dashboard => {
                let record = DashboardRecord::decode(kdl, self.state.default_provider())?;
                if let Some(entity) = record
                    .local
                    .keys()
                    .find(|entity| !LOCAL_KINDS.contains(&entity.kind.as_str()))
                {
                    return Err(format!("{:?} is not a local entity kind", entity.kind));
                }
                if let Some(entity) = record
                    .local
                    .keys()
                    .find(|entity| entity.provider != crate::LOCAL_PROVIDER)
                {
                    return Err(format!(
                        "local entity {:?} has provider {:?}",
                        entity.id, entity.provider
                    ));
                }
                self.state.restore_dashboard(&record);
                let mut patches = Vec::new();
                for entity in self.local.keys() {
                    if !record.local.contains_key(entity) {
                        patches.push(self.local_patch(entity, None));
                    }
                }
                for (entity, facts) in &record.local {
                    if self.local.get(entity) != Some(facts) {
                        patches.push(self.local_patch(entity, Some(facts)));
                    }
                }
                self.local = record.local;
                self.dashboard_unknown = record.unknown;
                for patch in patches {
                    self.state.apply_metadata_patch(patch);
                }
                self.maintain(true);
            }
            RecordName::Workspace(id) => {
                let record = WorkspaceRecord::decode(kdl, id, self.state.default_provider())?;
                self.state
                    .bind_workspace_subject(id, record.subject.as_ref());
                self.registered.insert(id);
                // A path from this session is no longer the recorded one.
                self.retained_paths.remove(&id);
                self.workspaces.insert(
                    id,
                    WorkspaceEntry {
                        record,
                        observed: BTreeSet::new(),
                    },
                );
                self.maintain(true);
            }
        }
        Ok(())
    }

    /// Publish slot resolutions to managed content, and bring open
    /// workspaces' baselines and arrangements up to date with their
    /// subjects' Suggested Layouts. A layout that is malformed or gone keeps
    /// the cached baseline: expiry is silence, not removal. This never
    /// invalidates the sidebar's snapshot; it bumps the content revision.
    fn refresh_content(&mut self) {
        let mut changed = self.managed.publish(self.state.slot_resolutions());
        let open: BTreeSet<WorkspaceId> = self
            .state
            .workspaces()
            .iter()
            .map(|tab| tab.tab_id)
            .collect();
        let state = &self.state;
        for (id, entry) in self.workspaces.iter_mut() {
            if !open.contains(id) {
                continue;
            }
            let record = &mut entry.record;
            let mut slots_changed = false;
            if let Some(subject) = &record.subject {
                if let Ok(Some(layout)) = state.suggested_layout(subject) {
                    slots_changed = record
                        .slots
                        .set_baseline(crate::slots::Baseline::from_layout(&layout));
                }
            }
            if slots_changed || (record.arrangement.is_none() && !record.slots.is_empty()) {
                changed |= follow_slots(record);
            }
            changed |= slots_changed;
        }
        if changed {
            self.content_revision += 1;
        }
    }

    /// Revision of workspace content: slots, their resolutions and
    /// arrangements. It changes when any of them may have; the sidebar's
    /// [`revision`](Self::revision) does not change with them.
    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    fn workspace_record(&self, id: WorkspaceId) -> Result<&WorkspaceRecord, String> {
        self.workspaces
            .get(&id)
            .map(|entry| &entry.record)
            .ok_or_else(|| format!("workspace {id} is not registered"))
    }

    fn workspace_record_mut(&mut self, id: WorkspaceId) -> Result<&mut WorkspaceRecord, String> {
        self.workspaces
            .get_mut(&id)
            .map(|entry| &mut entry.record)
            .ok_or_else(|| format!("workspace {id} is not registered"))
    }

    /// A registered workspace's slots, in default placement order.
    pub fn slots(&self, workspace: WorkspaceId) -> Result<Vec<crate::slots::SlotInfo>, String> {
        Ok(self.workspace_record(workspace)?.slots.slots())
    }

    /// Add the user's own slot (`u:<id>`), or override a baseline slot,
    /// detaching it. The arrangement is unchanged: the host gives the slot
    /// a tab in its next commit, which otherwise places it. Entity
    /// references that name no provider get the default provider.
    pub fn set_slot(
        &mut self,
        workspace: WorkspaceId,
        key: &str,
        mut spec: crate::suggested_layout::ViewSpec,
        rebind: crate::suggested_layout::RebindPolicy,
    ) -> Result<bool, String> {
        if let crate::suggested_layout::Content::ProviderFacet { entity, .. } = &mut spec.content {
            entity.fill_provider(self.state.default_provider());
        }
        let changed = self
            .workspace_record_mut(workspace)?
            .slots
            .set(key, spec, rebind)?;
        if changed {
            self.content_revision += 1;
        }
        Ok(changed)
    }

    /// Remove the user's own slot, or a detached slot its baseline dropped.
    /// Its tab, if any, is reported gone until the host removes it.
    pub fn remove_slot(&mut self, workspace: WorkspaceId, key: &str) -> Result<bool, String> {
        let removed = self.workspace_record_mut(workspace)?.slots.remove(key)?;
        if removed {
            self.managed.forget_slot(workspace, key);
            self.content_revision += 1;
        }
        Ok(removed)
    }

    /// Drop a baseline slot's override, so it follows the provider again.
    pub fn reattach_slot(&mut self, workspace: WorkspaceId, key: &str) -> Result<bool, String> {
        let reattached = self.workspace_record_mut(workspace)?.slots.reattach(key);
        if reattached {
            self.content_revision += 1;
        }
        Ok(reattached)
    }

    /// Plan one slot, given the identity of the resolution the host applied
    /// to it (empty for none). See [`crate::managed`].
    pub fn plan_slot(
        &mut self,
        workspace: WorkspaceId,
        key: &str,
        applied: &str,
    ) -> Result<crate::managed::SlotPlan, String> {
        let record = self.workspace_record(workspace)?;
        let slot = record
            .slots
            .slot(key)
            .ok_or_else(|| format!("workspace {workspace} has no slot {key:?}"))?;
        let source = slot_source(record.subject.as_ref(), &slot);
        Ok(self
            .managed
            .plan_slot(workspace, key, source, slot.rebind, applied))
    }

    /// Commit a workspace's whole arrangement, if `expected` is its current
    /// generation (0 before any). It is validated against the slot set: a
    /// tab names a slot, or a slot the stored arrangement already tabbed
    /// that has since gone. Slots with no tab are then placed by the default
    /// rule. A stale or invalid commit changes nothing. This never
    /// invalidates the sidebar's snapshot.
    pub fn set_arrangement(
        &mut self,
        workspace: WorkspaceId,
        doc: crate::slots::ArrangementDoc,
        expected: u64,
    ) -> Result<crate::slots::ArrangementCommit, crate::slots::ArrangementError> {
        let record = self
            .workspace_record_mut(workspace)
            .map_err(crate::slots::ArrangementError::Invalid)?;
        let slots = record.slots.slots();
        let stored = record.arrangement.get_or_insert_with(Default::default);
        let before = stored.generation;
        let mut next = || before + 1;
        let result = stored.commit(doc, expected, &slots, &mut next);
        if stored.generation == 0 {
            record.arrangement = None;
        }
        if result
            .as_ref()
            .is_ok_and(|commit| commit.generation != before)
        {
            self.content_revision += 1;
        }
        result
    }

    /// A registered workspace's arrangement. Before anything is stored it is
    /// empty, at generation 0.
    pub fn arrangement(
        &self,
        workspace: WorkspaceId,
    ) -> Result<crate::slots::ArrangementView, String> {
        let record = self.workspace_record(workspace)?;
        let slots = record.slots.slots();
        Ok(record.arrangement.clone().unwrap_or_default().view(&slots))
    }

    /// Conservative revision of presentation and action dependencies. Unchanged
    /// heartbeats and ticks preserve it; recipe changes invalidate it even when
    /// the rendered content is unchanged.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn invalidate(&mut self) {
        self.revision += 1;
        self.snapshot.take();
        self.evaluation.take();
        self.details.get_mut().clear();
    }

    fn evaluation(&self) -> &crate::state::RevisionEvaluation {
        self.evaluation
            .get_or_init(|| self.state.evaluate_revision())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_shared().clone()
    }

    /// Borrow the retained immutable snapshot, rebuilding only after a change.
    pub fn snapshot_shared(&self) -> &Snapshot {
        self.snapshot.get_or_init(|| Snapshot {
            revision: self.revision,
            surface: self.state.presentation_in(self.evaluation()),
            errors: self
                .errors
                .iter()
                .map(|(entity, message)| ActivationError {
                    entity: entity.clone(),
                    message: message.clone(),
                })
                .collect(),
        })
    }

    pub fn detail_cards(&self) -> (u64, Vec<crate::detail::DetailCard>) {
        (
            self.state.evaluation_time(),
            self.state.detail_cards_in(self.evaluation(), None),
        )
    }

    /// Exact catalog identity, including hidden and retained ended subjects.
    /// Cache at most 64 cards (including misses), and discard on revision change.
    pub fn detail_card(&self, entity: &EntityRef) -> Option<crate::detail::DetailCard> {
        let entity = &self.with_provider(entity);
        if let Some(card) = self.details.borrow().get(entity) {
            return card.clone();
        }
        let card = self
            .state
            .detail_cards_in(self.evaluation(), Some(entity))
            .pop();
        let mut cache = self.details.borrow_mut();
        // Whole-cache eviction is intentional: snapshot-owned cards survive it,
        // and a miss reuses the shared evaluation. Keep policy simple until a
        // measured working set warrants LRU bookkeeping.
        if cache.len() == 64 {
            cache.clear();
        }
        cache.insert(entity.clone(), card.clone());
        card
    }

    pub fn now_ms(&self) -> u64 {
        self.state.evaluation_time()
    }

    pub fn subject_url(&self, entity: &EntityRef) -> Option<String> {
        self.state.subject_url(&self.with_provider(entity))
    }

    /// `entity`, with the default provider if it names none.
    fn with_provider(&self, entity: &EntityRef) -> EntityRef {
        let mut entity = entity.clone();
        entity.fill_provider(self.state.default_provider());
        entity
    }

    pub fn dispatch(&mut self, mut action: Action) -> Result<Vec<HostEffect>, String> {
        let provider = self.state.default_provider().to_owned();
        match &mut action {
            Action::CopySubjectUrl { entity } | Action::Activate { entity } => {
                entity.fill_provider(&provider)
            }
            Action::ActivatePlacement { key }
            | Action::TogglePlacement { key }
            | Action::SetVariable { key, .. } => key.fill_provider(&provider),
            Action::ToggleDisplayVariable { .. } => {}
        }
        match action {
            Action::CopySubjectUrl { entity } => {
                let url = self
                    .state
                    .subject_url(&entity)
                    .ok_or("subject URL unavailable")?;
                Ok(vec![HostEffect::CopyUrl { url }])
            }
            Action::ActivatePlacement { key } => {
                let node = self
                    .snapshot_shared()
                    .surface
                    .node(&key)
                    .ok_or("unknown placement")?;
                // A reference activates what it presents.
                let entity = self.state.presented_entity(&node.entity);
                if let Some(url) = self.state.subject_url(&entity) {
                    return Ok(vec![HostEffect::OpenUrl { url }]);
                }
                if self.entity_is_pending(&entity) {
                    return Ok(vec![]);
                }
                if let crate::presentation::PresentationState::Live { workspace_id, .. } =
                    node.state
                {
                    if !self
                        .state
                        .workspaces()
                        .iter()
                        .any(|w| w.tab_id == workspace_id)
                    {
                        return Err("workspace disappeared".into());
                    }
                    self.errors.remove(&entity);
                    let request_id = self.request_id();
                    self.pending.insert(request_id, Pending::Focus(entity));
                    self.invalidate();
                    Ok(vec![HostEffect::Focus {
                        request_id,
                        workspace_id,
                    }])
                } else {
                    self.dispatch(Action::Activate { entity })
                }
            }
            Action::Activate { entity } => {
                if let Some(url) = self.state.subject_url(&entity) {
                    return Ok(vec![HostEffect::OpenUrl { url }]);
                }
                if self.entity_is_pending(&entity) {
                    return Ok(vec![]);
                }
                if self.errors.remove(&entity).is_some() {
                    self.invalidate();
                }
                let activation = self.state.activation_for_entity(&entity);
                let effect = match activation {
                    Some(EntityActivation::FocusTab { position }) => {
                        let workspace_id = self
                            .state
                            .workspaces()
                            .iter()
                            .find(|w| w.position == position)
                            .ok_or("workspace disappeared")?
                            .tab_id;
                        let request_id = self.request_id();
                        self.pending.insert(request_id, Pending::Focus(entity));
                        HostEffect::Focus {
                            request_id,
                            workspace_id,
                        }
                    }
                    Some(EntityActivation::Materialize(request)) => {
                        if !self.state.begin_latent_materialization(&request) {
                            return Ok(vec![]);
                        }
                        let request_id = self.request_id();
                        let primary_target = self.state.managed_primary_target(
                            &entity,
                            &request.recipe,
                            request.checkout_path.as_deref(),
                        );
                        let effect = HostEffect::Materialize {
                            request_id,
                            entity: entity.clone(),
                            name: request.name.clone(),
                            recipe: request.recipe.clone(),
                            cwd: request.checkout_path.clone(),
                            primary_target,
                        };
                        self.pending
                            .insert(request_id, Pending::Materialize(entity, request));
                        effect
                    }
                    None => {
                        // An opening presentation may be awaiting its first observation.
                        if self.state.entity_is_opening(&entity) {
                            return Ok(vec![]);
                        }
                        HostEffect::Inspect { entity }
                    }
                };
                self.invalidate();
                Ok(vec![effect])
            }
            Action::TogglePlacement { key } => {
                if self.snapshot_shared().surface.node(&key).is_none() {
                    return Err("unknown placement".into());
                }
                self.state
                    .apply_rail_ui_action(RailUiAction::TogglePlacement { key });
                self.invalidate();
                Ok(vec![])
            }
            Action::SetVariable { key, name, value } => {
                let snapshot = self.snapshot_shared();
                let node = snapshot.surface.node(&key).ok_or("unknown placement")?;
                let definition = node
                    .variables
                    .as_ref()
                    .and_then(|v| v.declarations.iter().find(|d| d.name == name))
                    .ok_or("unknown variable")?;
                if value
                    .as_ref()
                    .is_some_and(|value| !definition.accepts(value))
                {
                    return Err("value is not allowed by the variable declaration".into());
                }
                self.state
                    .set_node_variable(NodeKey::Placement(key), name, value);
                self.invalidate();
                Ok(vec![])
            }
            Action::ToggleDisplayVariable { name } => {
                if !self
                    .state
                    .apply_rail_ui_action(RailUiAction::ToggleVariable { name })
                {
                    return Err("unknown display variable".into());
                }
                self.invalidate();
                Ok(vec![])
            }
        }
    }

    /// Materialize success supplies the newly created workspace ID. Focus
    /// success supplies None. Duplicate/stale results are ignored. The host
    /// must eventually complete each effect, including timeout or cancellation.
    pub fn complete(
        &mut self,
        request_id: u64,
        result: Result<Option<WorkspaceId>, String>,
    ) -> bool {
        let Some(pending) = self.pending.remove(&request_id) else {
            return false;
        };
        match pending {
            Pending::Focus(entity) => {
                if let Err(error) = result {
                    self.errors.insert(entity, error);
                }
            }
            Pending::Materialize(entity, request) => match result {
                Ok(Some(id)) => {
                    self.registered.insert(id);
                    self.workspaces.entry(id).or_default();
                    self.state
                        .bind_materializing_subject(&request, id, entity.clone());
                    // If observation arrived before acknowledgement, claim now.
                    let current = self.state.workspaces().to_vec();
                    self.state.observe_workspaces(current);
                }
                other => {
                    self.state.abort_latent_materialization(&request);
                    self.errors.insert(
                        entity,
                        other
                            .err()
                            .unwrap_or_else(|| "materialize returned no workspace ID".into()),
                    );
                }
            },
        }
        self.invalidate();
        self.refresh_workspace_records();
        self.refresh_content();
        true
    }

    fn entity_is_pending(&self, entity: &EntityRef) -> bool {
        self.pending.values().any(|pending| match pending {
            Pending::Focus(e) | Pending::Materialize(e, _) => e == entity,
        })
    }

    fn request_id(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }
}

/// Ended presentation expiry policy is intentionally undecided. Keep the seam
/// shared by all retained workspace paths; user close is the only expiry today.
// TODO: apply the policy decided in flotilla-org/wheelhouse#159 here.
/// Where a slot's resolution comes from: a baseline slot that follows its
/// provider resolves from the subject's layout; any other slot from its spec,
/// as a local recipe or a provider facet (`facet` of the entity's own layout,
/// `primary` for the `workspace.primary.*` facts).
fn slot_source(
    subject: Option<&EntityRef>,
    slot: &crate::slots::SlotInfo,
) -> crate::managed::SlotSource {
    use crate::{managed::SlotSource, suggested_layout::Content};
    match (
        subject,
        slot.in_baseline && !slot.detached,
        &slot.spec.content,
    ) {
        (Some(subject), true, _) => SlotSource::Provider {
            entity: subject.clone(),
            slot: slot.key.clone(),
        },
        (_, _, Content::ProviderFacet { entity, facet }) => SlotSource::Provider {
            entity: entity.clone(),
            slot: facet.clone(),
        },
        (_, _, Content::Local(recipe)) => SlotSource::Local(recipe.clone()),
    }
}

/// Bring a record's arrangement up to date with its slots. Returns whether
/// it changed.
fn follow_slots(record: &mut WorkspaceRecord) -> bool {
    let slots = record.slots.slots();
    let stored = record.arrangement.get_or_insert_with(Default::default);
    let next = stored.generation + 1;
    stored.follow(record.slots.baseline.as_ref(), &slots, &mut || next)
}

fn retained_workspace_expired(_state: &ControllerState, _subjects: &BTreeSet<EntityRef>) -> bool {
    false
}

#[cfg(test)]
mod evaluation_tests {
    use super::*;
    use crate::{MetadataTarget, MetadataValue, MetadataValueUpdate};

    const CONFIG: &str = include_str!("../tests/fixtures/sidebar.kdl");
    fn entity() -> EntityRef {
        EntityRef::local("vessel", "v")
    }
    fn patch(source: &str, label: &str, ttl: Option<u64>, precedence: i64) -> MetadataPatch {
        MetadataPatch {
            target: MetadataTarget::Entity(entity()),
            source_id: source.into(),
            unset: vec![],
            set: [
                ("display.label", label),
                ("flotilla.project", "p"),
                ("flotilla.vessel", "v"),
                ("action.primary.recipe", "true"),
                ("status.state", "waiting"),
            ]
            .into_iter()
            .map(|(key, value)| {
                (
                    key.into(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text(value.into()),
                        ttl_ms: ttl,
                        precedence: Some(precedence),
                        ordinal: None,
                    },
                )
            })
            .collect(),
        }
    }
    fn add_project(sidebar: &mut Sidebar) {
        sidebar.apply(
            100,
            [MetadataPatch {
                target: MetadataTarget::Entity(EntityRef::local("project", "p")),
                source_id: "base".into(),
                unset: vec![],
                set: [(
                    "flotilla.project".into(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text("p".into()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                )]
                .into(),
            }],
        );
    }
    fn check(sidebar: &Sidebar) {
        // Issue #96: cached presentation and exact details must equal an uncached
        // controller evaluation after every mutation, including identities without rows.
        assert_eq!(
            sidebar.snapshot_shared().surface,
            sidebar.state.view_model().presentation.unwrap()
        );
        let (_, all) = sidebar.state.detail_cards();
        for card in &all {
            assert_eq!(sidebar.detail_card(&card.entity).as_ref(), Some(card));
        }
        assert_eq!(sidebar.detail_cards().1, all);
    }

    #[test]
    fn revision_queries_share_evaluation_and_validation() {
        let mut sidebar = Sidebar::new(CONFIG).unwrap();
        sidebar.apply(100, [patch("a", "first", None, 0)]);
        add_project(&mut sidebar);
        crate::profile::start();
        let snapshot = sidebar.snapshot();
        let key = snapshot.surface.sections[0].nodes[0].children[0]
            .key
            .clone();
        sidebar.detail_card(&entity()).unwrap();
        sidebar.detail_cards();
        sidebar.detail_card(&entity()).unwrap();
        assert_eq!(crate::profile::take()["catalog"].calls, 1);
        sidebar.dispatch(Action::TogglePlacement { key }).unwrap();
        // Validation used the rendered snapshot; only the new output evaluates.
        assert!(!crate::profile::take().contains_key("catalog"));
        assert_ne!(sidebar.snapshot().revision, snapshot.revision);
        assert_eq!(crate::profile::take()["catalog"].calls, 1);
        check(&sidebar);
    }

    #[test]
    fn generated_revision_lifecycles_match_uncached_output() {
        // Deterministic operation generator spans duplicate observations, source
        // precedence, expiry boundaries, unsets, config variables, topology,
        // dispatch and completions. Real controller/store/template collaborators.
        for seed in 0..24u64 {
            let mut sidebar = Sidebar::new(CONFIG).unwrap();
            sidebar.apply(100, [patch("base", "original", None, 0)]);
            add_project(&mut sidebar);
            let held = sidebar.snapshot();
            let mut now = 100;
            for step in 0..18 {
                match (seed + step * 7) % 9 {
                    0 => sidebar.apply(
                        now,
                        [patch("update", &format!("{seed}/{step}"), Some(5), 10)],
                    ),
                    1 => {
                        now += 5;
                        sidebar.apply(now, []);
                    }
                    2 => {
                        now += 1;
                        sidebar.apply(now, []);
                    }
                    3 => {
                        let mut p = patch("update", "", None, 0);
                        p.set.clear();
                        p.unset = vec!["display.label".into()];
                        sidebar.apply(now, [p]);
                    }
                    4 => sidebar
                        .configure(&CONFIG.replace("default=\"roomy\"", "default=\"compact\""))
                        .unwrap(),
                    5 => sidebar.observe(
                        vec![Workspace {
                            id: WorkspaceId::from(1),
                            position: 0,
                            name: "vessel:v".into(),
                            selected: step % 2 == 0,
                        }],
                        vec![],
                    ),
                    6 => sidebar.observe(vec![], vec![]),
                    7 => {
                        if let Some(key) = sidebar
                            .snapshot()
                            .surface
                            .sections
                            .iter()
                            .flat_map(|s| &s.nodes)
                            .flat_map(|n| &n.children)
                            .find(|n| n.entity == entity())
                            .map(|n| n.key.clone())
                        {
                            sidebar
                                .dispatch(Action::SetVariable {
                                    key,
                                    name: "density".into(),
                                    value: Some("compact".into()),
                                })
                                .unwrap();
                        }
                    }
                    _ => {
                        for effect in sidebar
                            .dispatch(Action::Activate { entity: entity() })
                            .unwrap()
                        {
                            match effect {
                                HostEffect::Materialize { request_id, .. }
                                | HostEffect::Focus { request_id, .. } => {
                                    sidebar.complete(request_id, Err("test refusal".into()));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                check(&sidebar);
            }
            // Old snapshots are owned immutable values through every revision.
            assert_eq!(
                held.surface.sections[0].nodes[0].children[0].label,
                "original"
            );
        }
    }

    // Semantic output must not depend on whether tab metadata was resolved in
    // inventory or controller sort order, including a pin-driven reorder.
    #[test]
    fn presentation_matches_controller_for_every_sort_mode() {
        for mode in [
            crate::SortMode::Controller,
            crate::SortMode::Position,
            crate::SortMode::PinnedFirst,
            crate::SortMode::LatestStatus,
        ] {
            let mut sidebar = Sidebar::new(CONFIG).unwrap();
            sidebar.apply(100, [patch("base", "original", None, 0)]);
            add_project(&mut sidebar);
            sidebar.observe(
                vec![
                    Workspace {
                        id: WorkspaceId::from(1),
                        position: 1,
                        name: "vessel:v".into(),
                        selected: true,
                    },
                    Workspace {
                        id: WorkspaceId::from(2),
                        position: 0,
                        name: "Other".into(),
                        selected: false,
                    },
                ],
                vec![],
            );
            sidebar.state.set_sort_mode(mode);
            sidebar.state.toggle_pin(WorkspaceId::from(1));
            sidebar.invalidate();
            check(&sidebar);
            let tabs = sidebar.state.view_model().tabs;
            assert_eq!(
                tabs[0].tab_id,
                WorkspaceId::from(if mode == crate::SortMode::PinnedFirst {
                    1
                } else {
                    2
                })
            );
        }
    }

    #[test]
    fn exact_detail_cache_is_bounded_and_invalidates_misses() {
        let mut sidebar = Sidebar::new(CONFIG).unwrap();
        // Missing, empty and invalid identities are cached without growing forever.
        for i in 0..200 {
            assert!(sidebar
                .detail_card(&EntityRef::local("missing", i.to_string()))
                .is_none());
            assert!(sidebar.details.borrow().len() <= 64);
        }
        assert!(sidebar.detail_card(&entity()).is_none());
        sidebar.apply(100, [patch("base", "appeared", None, 0)]);
        assert_eq!(sidebar.detail_card(&entity()).unwrap().label, "appeared");
    }
}
