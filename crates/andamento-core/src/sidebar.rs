//! One presentation client's runtime. Hosts supply time and observations and
//! execute returned effects. Transport and rendering do not run inside it.
#[cfg(test)]
use crate::census::Census;
use std::rc::Rc;
use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, Serialize};

use crate::{
    detail::DetailCard,
    host::PaneObservation,
    managed::{ManagedContent, SlotPlan},
    metadata::EntityId,
    presentation::{system, PlacementNode, PresentationState, SurfaceSnapshot, UNPLACED_WORKSPACES_SECTION},
    profile,
    records::{recordable, DashboardRecord, DashboardState, RecordName, SubjectRecord},
    sidebar_arrangement::{self, Declared, LocalSection, Report, SidebarArrangement, SidebarDoc, SidebarView},
    slots::{ArrangementCommit, ArrangementDoc, ArrangementView, Baseline, OverlayView, Resolve, SlotInfo},
    state::{ControllerState, ControllerTab, EntityActivation, RevisionEvaluation},
    suggested_layout::{RebindPolicy, ViewSpec},
    target_resolution::{PortableResolution, SavedResolution},
    template_config::{parse_template_config_kdl, TemplateConfigCatalog},
    versioned::{Clock, CommitError, Versioned},
    workspace::{Subject, WorkspaceRecord},
    DisplayVariableValue, EntityRef, MaterializeLatentRequest, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, NodeKey,
    PlacementKey, PlacementLoopKey, RailUiAction, WorkspaceId, LOCAL_PROVIDER,
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
    /// Apply patches from `provider` (a subscription ID).
    Apply {
        now_ms: u64,
        provider: String,
        patches: Vec<MetadataPatch>,
    },
    Tick {
        now_ms: u64,
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
        value: Option<DisplayVariableValue>,
    },
    SetLocal {
        entity: EntityRef,
        facts: BTreeMap<String, MetadataValue>,
    },
    RemoveLocal {
        entity: EntityRef,
    },
    ImportRecord {
        name: String,
        record: String,
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
    evaluation: OnceCell<RevisionEvaluation>,
    details: RefCell<BTreeMap<EntityRef, Option<DetailCard>>>,
    next_request: u64,
    pending: BTreeMap<u64, Pending>,
    errors: BTreeMap<EntityRef, String>,
    retained_paths: BTreeMap<WorkspaceId, BTreeSet<EntityRef>>,
    retained_paths_revision: Option<u64>,
    /// Each registered workspace's record (`workspace/<id>`).
    workspaces: BTreeMap<WorkspaceId, WorkspaceEntry>,
    /// Local sections, groups and refs, with their facts.
    local: BTreeMap<EntityRef, BTreeMap<String, MetadataValue>>,
    /// Every revision and generation comes from this clock.
    clock: Clock,
    /// The Dashboard's state as of the last change (see
    /// [`invalidate`](Self::invalidate)), for the dashboard record.
    dashboard: Versioned<DashboardState>,
    /// Fact keys the configured placement reads, kept in subject records.
    placement_keys: BTreeSet<String>,
    pub managed: ManagedContent,
    /// When workspace content outside the records last changed: managed
    /// content's resolutions, or a workspace forgotten.
    content_changed: u64,
    /// The Dashboard's sidebar arrangement, and what its last
    /// reconciliation did.
    sidebar_arrangement: Versioned<SidebarArrangement>,
    sidebar_report: Report,
}

#[derive(Default)]
struct WorkspaceEntry {
    record: WorkspaceRecord,
    /// Retained subjects a producer published at the last refresh.
    observed: BTreeSet<EntityRef>,
}

/// The kinds of local entities: sections, groups and refs people make, which
/// Andamento owns and keeps in the dashboard record.
pub const LOCAL_KINDS: [&str; 3] = [system::SECTION, system::GROUP, system::REF];
const SOURCE_LOCAL: &str = "andamento-local";

impl Sidebar {
    pub fn handle(&mut self, request: Request) -> Result<Response, String> {
        let effects = match request {
            Request::Configure { kdl } => {
                self.configure(&kdl)?;
                vec![]
            }
            Request::Apply { now_ms, provider, patches } => {
                self.apply(now_ms, &provider, patches)?;
                vec![]
            }
            Request::Tick { now_ms } => {
                self.tick(now_ms);
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
            Request::Complete { request_id, workspace_id, error } => {
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
            Request::ImportRecord { name, record } => {
                self.import_record(&name, &record)?;
                vec![]
            }
            Request::Snapshot => vec![],
        };
        Ok(Response { snapshot: self.snapshot(), effects })
    }

    pub fn new(config_kdl: &str) -> Result<Self, String> {
        let mut sidebar = Self::default();
        sidebar.configure(config_kdl)?;
        Ok(sidebar)
    }

    /// Validate the template catalog before replacing the active configuration.
    pub fn configure(&mut self, config_kdl: &str) -> Result<(), String> {
        let templates = parse_template_config_kdl(config_kdl).map_err(|e| e.to_string())?;
        self.state.set_template_catalog(Some(TemplateConfigCatalog::with_bundled_defaults(templates)));
        self.placement_keys = self.state.placement_fact_keys();
        self.invalidate();
        self.follow_sidebar_declarations();
        Ok(())
    }

    /// Apply a drained batch from `provider`, the subscription ID the host
    /// received it on, before requesting a snapshot. The provider is stamped
    /// over every entity the patches name; a patch never names its own. Time
    /// is monotonic milliseconds in this instance, supplied by the host.
    pub fn apply(&mut self, now_ms: u64, provider: &str, patches: impl IntoIterator<Item = MetadataPatch>) -> Result<(), String> {
        if provider.is_empty() || provider == LOCAL_PROVIDER {
            return Err(format!("a provider needs a name other than {:?}", LOCAL_PROVIDER));
        }
        self.apply_patches(now_ms, Some(provider), patches);
        Ok(())
    }

    /// Advance time without facts, expiring what has lapsed.
    pub fn tick(&mut self, now_ms: u64) {
        self.apply_patches(now_ms, None, []);
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
    /// it returns. A stale provider's facts are kept, and snapshot nodes and
    /// detail fields presenting its entities are flagged stale. Its leased
    /// non-entity facts don't expire while it is stale; fresh again, each
    /// renews its lease from now, so one it doesn't reassert expires a TTL
    /// later. Returns whether the provider's state changed.
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

    fn apply_patches(&mut self, now_ms: u64, provider: Option<&str>, patches: impl IntoIterator<Item = MetadataPatch>) {
        let _phase = profile::span("apply");
        let maintenance = profile::span("workspace-maintenance");
        let subjects = self.state.workspace_subjects();
        let mut changed = self.retain_workspace_paths(&subjects);
        drop(maintenance);
        changed |= self.state.advance_time(now_ms);
        let patch_phase = profile::span("patch-application");
        if let Some(provider) = provider {
            for patch in patches {
                changed |= self.state.apply_provider_patch(provider, patch);
            }
        }
        drop(patch_phase);
        let maintenance = profile::span("workspace-maintenance");
        changed |= self.state.refresh_retained_subjects();
        changed |= self.state.mark_ended_workspace_paths(&self.retained_paths, &subjects);
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
        fn collect(nodes: &[Rc<PlacementNode>], ancestors: &mut Vec<EntityRef>, paths: &mut BTreeMap<WorkspaceId, BTreeSet<EntityRef>>) {
            for node in nodes {
                // Andamento's own kinds (`.`-prefixed: workspaces, sections,
                // groups, refs) are never retained: the host retracts them, and
                // a ghost must not outlive its removal because its target is
                // open.
                let subject = !system::is_system(&node.entity.kind);
                if subject {
                    ancestors.push(node.entity.clone());
                }
                if let PresentationState::Live { workspace_id, .. } = node.state {
                    paths.entry(workspace_id).or_default().extend(ancestors.iter().cloned());
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
                    .extend(paths.into_iter().filter(|(id, path)| subjects.get(id).is_some_and(|subject| path.contains(subject))));
                self.retained_paths_revision = Some(self.revision);
            }
        }
        // A workspace with no path yet, still open for the subject its record
        // names, takes its recorded path: so a restored row is drawn where it
        // was before any producer publishes it. A path from a snapshot
        // replaces it.
        for (id, entry) in &self.workspaces {
            let subject = entry.record.subject.get();
            if subject.entity.is_some()
                && subjects.get(id) == subject.entity.as_ref()
                && !subject.retained.is_empty()
                && !self.retained_paths.contains_key(id)
            {
                self.retained_paths.insert(*id, subject.retained.keys().cloned().collect());
            }
        }
        self.retained_paths
            .retain(|id, subjects_on_path| subjects.contains_key(id) && !retained_workspace_expired(&self.state, subjects_on_path));
        let saved = self.saved_subjects();
        let mut retained: BTreeSet<EntityRef> = self.retained_paths.values().flatten().cloned().collect();
        retained.extend(self.ref_targets());
        self.state.retain_workspace_paths(retained, &saved)
    }

    /// The entities local refs (pins) present. They are retained like the
    /// subjects on open workspace paths, so a ref keeps presenting its target
    /// when the target's facts go away.
    fn ref_targets(&self) -> impl Iterator<Item = EntityRef> + '_ {
        self.local
            .iter()
            .filter(|(entity, _)| entity.kind == system::REF)
            .filter_map(|(_, facts)| match facts.get(system::TARGET) {
                Some(MetadataValue::EntityRefs(refs)) if refs.len() == 1 => Some(refs[0].clone()),
                _ => None,
            })
            .filter(|target| !system::is_system(&target.kind))
    }

    /// Subjects recorded by every workspace record. Where records disagree, an
    /// end wins, then the latest sighting.
    fn saved_subjects(&self) -> BTreeMap<EntityRef, SubjectRecord> {
        let mut saved = BTreeMap::<EntityRef, SubjectRecord>::new();
        for entry in self.workspaces.values() {
            for (entity, record) in &entry.record.subject.get().retained {
                let newer =
                    saved.get(entity).is_none_or(|current| (record.ended, record.last_seen_ms) > (current.ended, current.last_seen_ms));
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
        self.managed.retain_workspaces(&workspaces.iter().map(|w| w.id).collect::<Vec<_>>());
        let mut changed = self.state.observe_workspaces(
            workspaces
                .into_iter()
                .map(|w| ControllerTab { tab_id: w.id, position: w.position, name: w.name, active: w.selected })
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
        changed |= self.state.mark_ended_workspace_paths(&self.retained_paths, &subjects);
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
        let new = self.ensure_workspace(id);
        self.refresh_workspace_records();
        self.refresh_content();
        new
    }

    /// Give a workspace a record if it has none. Returns whether it was new.
    fn ensure_workspace(&mut self, id: WorkspaceId) -> bool {
        if self.workspaces.contains_key(&id) {
            return false;
        }
        let mut entry = WorkspaceEntry::default();
        // A record's generation is nonzero from the start.
        entry.record.subject.restamp(&mut self.clock);
        self.workspaces.insert(id, entry);
        true
    }

    /// The host deleted the workspace rather than keeping it. Returns whether
    /// it was registered.
    pub fn forget_workspace(&mut self, id: WorkspaceId) -> bool {
        let forgotten = self.workspaces.remove(&id).is_some();
        if forgotten {
            self.content_changed = self.clock.tick();
        }
        forgotten
    }

    /// Workspaces the host registered or materialized and has not forgotten.
    pub fn registered_workspaces(&self) -> BTreeSet<WorkspaceId> {
        self.workspaces.keys().copied().collect()
    }

    /// Set the host-owned order of one sibling run, identified by the loop key
    /// every node in it carries. An empty order returns the run to data order.
    pub fn set_sibling_order(&mut self, loop_key: PlacementLoopKey, order: Vec<EntityRef>) -> Result<(), String> {
        for entity in &order {
            entity.check_provider()?;
        }
        self.state.apply_rail_ui_action(RailUiAction::SetSiblingOrder { loop_key, order });
        self.invalidate();
        Ok(())
    }

    /// Set a declared display variable, or return it to its default with
    /// None, without a snapshot action. Unknown variables and values the
    /// declaration doesn't allow are rejected.
    pub fn set_display_variable(&mut self, name: &str, value: Option<DisplayVariableValue>) -> Result<(), String> {
        if self.state.set_display_variable(name, value)? {
            self.invalidate();
        }
        Ok(())
    }

    /// Set a local section, group or ref (see [`LOCAL_KINDS`]), replacing all
    /// its facts. Andamento owns local entities and keeps them in the
    /// dashboard record; their provider is [`LOCAL_PROVIDER`](crate::LOCAL_PROVIDER).
    /// Lists of text and group paths are not allowed; entity references name
    /// their providers.
    pub fn set_local(&mut self, entity: EntityRef, facts: BTreeMap<String, MetadataValue>) -> Result<(), String> {
        if entity.provider != LOCAL_PROVIDER {
            return Err(format!("local entities have the {:?} provider, not {:?}", LOCAL_PROVIDER, entity.provider));
        }
        for value in facts.values() {
            value.check_providers()?;
        }
        if !LOCAL_KINDS.contains(&entity.kind.as_str()) {
            return Err(format!("local entities are {}, not {:?}", LOCAL_KINDS.join(", "), entity.kind));
        }
        if entity.id.is_empty() {
            return Err("a local entity needs an ID".into());
        }
        if facts.keys().any(String::is_empty) {
            return Err("a fact needs a key".into());
        }
        if !facts.values().all(recordable) {
            return Err("local facts are text, booleans, integers or entity references".into());
        }
        let patch = self.local_patch(&entity, Some(&facts));
        let section = entity.kind != system::REF;
        let changed = self.local.get(&entity) != Some(&facts);
        self.local.insert(entity, facts);
        self.apply_local([patch], changed);
        if section {
            self.follow_sidebar_declarations();
        }
        Ok(())
    }

    /// Remove a local entity. Returns whether there was one.
    pub fn remove_local(&mut self, entity: &EntityRef) -> bool {
        if !self.local.contains_key(entity) {
            return false;
        }
        let patch = self.local_patch(entity, None);
        self.local.remove(entity);
        self.apply_local([patch], true);
        if entity.kind != system::REF {
            self.follow_sidebar_declarations();
        }
        true
    }

    /// Saved sibling orders, by the loop key of the run each orders.
    pub fn sibling_orders(&self) -> &BTreeMap<PlacementLoopKey, Vec<EntityRef>> {
        self.state.sibling_orders()
    }

    /// Local entities, with their facts.
    pub fn local_entities(&self) -> &BTreeMap<EntityRef, BTreeMap<String, MetadataValue>> {
        &self.local
    }

    /// The patch that makes the catalog hold `facts` for a local entity,
    /// unsetting the keys it held before.
    fn local_patch(&self, entity: &EntityRef, facts: Option<&BTreeMap<String, MetadataValue>>) -> MetadataPatch {
        let empty = BTreeMap::new();
        let facts = facts.unwrap_or(&empty);
        MetadataPatch {
            target: MetadataTarget::Entity(entity.clone()),
            source_id: SOURCE_LOCAL.into(),
            set: facts
                .iter()
                .map(|(key, value)| {
                    (key.clone(), MetadataValueUpdate { value: value.clone(), ttl_ms: None, precedence: None, ordinal: None })
                })
                .collect(),
            unset: self.local.get(entity).into_iter().flat_map(BTreeMap::keys).filter(|key| !facts.contains_key(*key)).cloned().collect(),
        }
    }

    /// Apply local patches at the current time, without advancing it.
    /// `changed`: the local entities changed, whatever their facts did.
    fn apply_local(&mut self, patches: impl IntoIterator<Item = MetadataPatch>, mut changed: bool) {
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
        changed |= self.state.mark_ended_workspace_paths(&self.retained_paths, &subjects);
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
        let open = self.state.workspaces().iter().map(|tab| tab.tab_id).filter(|id| self.workspaces.contains_key(id)).collect::<Vec<_>>();
        if open.is_empty() {
            return;
        }
        let subjects = self.state.workspace_subjects();
        let keys = &self.placement_keys;
        let now = self.state.evaluation_time();
        for id in open {
            let path = self.retained_paths.get(&id).cloned().unwrap_or_default();
            let entry = self.workspaces.get_mut(&id).expect("open IDs are registered");
            let mut retained = BTreeMap::new();
            let mut observed = BTreeSet::new();
            for entity in path {
                let Some((mut record, seen)) = self.state.retained_subject_record(&entity, keys) else {
                    if let Some(old) = entry.record.subject.get().retained.get(&entity) {
                        retained.insert(entity, old.clone());
                    }
                    continue;
                };
                let old = entry.record.subject.get().retained.get(&entity);
                // Last seen moves when the subject appears, changes or goes
                // away, not on every heartbeat.
                record.last_seen_ms = match old {
                    Some(old)
                        if (&old.label, old.ended, &old.facts) == (&record.label, record.ended, &record.facts)
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
            let subject = Subject { entity: subjects.get(&id).cloned(), retained };
            entry.record.follow_subject(&mut self.clock, subject);
            entry.observed = observed;
        }
    }

    /// Names of the records Andamento holds: `dashboard`, and
    /// `workspace/<id>` for each registered workspace.
    pub fn record_names(&self) -> Vec<String> {
        std::iter::once(RecordName::Dashboard)
            .chain(self.workspaces.keys().copied().map(RecordName::Workspace))
            .map(|name| name.to_string())
            .collect()
    }

    /// A record as text to store. Its format is private: only this
    /// Andamento reads it.
    pub fn export_record(&self, name: &str) -> Result<String, String> {
        match RecordName::parse(name)? {
            RecordName::Dashboard => Ok(DashboardRecord {
                state: self.dashboard.get().clone(),
                sidebar: (self.sidebar_arrangement.generation() != 0).then(|| self.sidebar_arrangement.get().clone()),
            }
            .encode()),
            RecordName::Workspace(id) => Ok(self.workspace(id)?.encode()),
        }
    }

    /// A record's generation, nonzero. It changes whenever the record does,
    /// and is never reused. Store the record when it differs from the one
    /// last stored.
    pub fn record_generation(&self, name: &str) -> Result<u64, String> {
        match RecordName::parse(name)? {
            RecordName::Dashboard => Ok(self.dashboard.generation().max(self.sidebar_arrangement.generation())),
            RecordName::Workspace(id) => Ok(self.workspace(id)?.generation()),
        }
    }

    /// The Dashboard's state other than its sidebar arrangement, as it is
    /// now.
    fn dashboard_state(&self) -> DashboardState {
        DashboardState {
            display: self.state.recorded_display_variables(),
            collapsed: self.state.collapsed_placements().clone(),
            orders: self.state.sibling_orders().clone(),
            variables: self.state.placement_variables(),
            local: self.local.clone(),
        }
    }

    /// Import a record exported by this Andamento. It may come before the
    /// first observation and needs no facts. A record that doesn't read, or
    /// names another record, is rejected without changing anything.
    /// Importing `workspace/<id>` registers the workspace and binds it to
    /// its recorded subject.
    pub fn import_record(&mut self, name: &str, text: &str) -> Result<(), String> {
        match RecordName::parse(name)? {
            RecordName::Dashboard => {
                let DashboardRecord { state, sidebar } = DashboardRecord::decode(text)?;
                if let Some(entity) = state.local.keys().find(|entity| !LOCAL_KINDS.contains(&entity.kind.as_str())) {
                    return Err(format!("{:?} is not a local entity kind", entity.kind));
                }
                if let Some(entity) = state.local.keys().find(|entity| entity.provider != LOCAL_PROVIDER) {
                    return Err(format!("local entity {:?} has provider {:?}", entity.id, entity.provider));
                }
                self.state.restore_dashboard(&state);
                let mut patches = Vec::new();
                for entity in self.local.keys() {
                    if !state.local.contains_key(entity) {
                        patches.push(self.local_patch(entity, None));
                    }
                }
                for (entity, facts) in &state.local {
                    if self.local.get(entity) != Some(facts) {
                        patches.push(self.local_patch(entity, Some(facts)));
                    }
                }
                self.local = state.local;
                for patch in patches {
                    self.state.apply_metadata_patch(patch);
                }
                self.maintain(true);
                self.import_sidebar_arrangement(sidebar.unwrap_or_default());
            }
            RecordName::Workspace(id) => {
                let record = WorkspaceRecord::decode(text, &mut self.clock)?;
                self.state.bind_workspace_subject(id, record.subject());
                // A path from this session is no longer the recorded one.
                self.retained_paths.remove(&id);
                self.workspaces.insert(id, WorkspaceEntry { record, observed: BTreeSet::new() });
                self.maintain(true);
            }
        }
        Ok(())
    }

    /// Publish slot resolutions to managed content, and bring open
    /// workspaces' baselines and arrangements up to date with their
    /// subjects' Suggested Layouts. A layout that is malformed or gone keeps
    /// the cached baseline: expiry is silence, not removal. This never
    /// invalidates the sidebar's snapshot; it moves the content revision.
    fn refresh_content(&mut self) {
        // Only the entities whose facts changed resolve differently.
        let changed = self.state.take_metadata_changes();
        let entities: BTreeSet<EntityRef> = if changed.all {
            self.state.metadata_entities().into_iter().chain(self.managed.published().cloned()).collect()
        } else {
            changed
                .targets
                .into_iter()
                .filter_map(|target| match target {
                    EntityId::Entity(entity) => Some(entity),
                    _ => None,
                })
                .collect()
        };
        let mut published = false;
        for entity in &entities {
            published |= self.managed.publish(entity, self.state.entity_slot_resolutions(entity));
        }
        if published {
            self.content_changed = self.clock.tick();
        }
        let open: BTreeSet<WorkspaceId> = self.state.workspaces().iter().map(|tab| tab.tab_id).collect();
        for (id, entry) in self.workspaces.iter_mut() {
            let record = &mut entry.record;
            if open.contains(id) {
                let baseline = record
                    .subject()
                    .and_then(|subject| self.state.suggested_layout(subject).ok().flatten())
                    .map(|layout| Baseline::from_layout(&layout));
                record.follow_baseline(&mut self.clock, baseline);
            }
            record.invalidate_resolutions(&mut self.clock, &self.managed);
        }
    }

    /// Revision of workspace content: slots, their resolutions and plans,
    /// and arrangements. It changes when any of them may have; the
    /// sidebar's [`revision`](Self::revision) does not change with them.
    pub fn content_revision(&self) -> u64 {
        self.workspaces.values().map(|entry| entry.record.content_generation()).fold(self.content_changed, u64::max)
    }

    fn workspace(&self, id: WorkspaceId) -> Result<&WorkspaceRecord, String> {
        self.workspaces.get(&id).map(|entry| &entry.record).ok_or_else(|| format!("workspace {id} is not registered"))
    }

    /// Edit a registered workspace's record, with the clock its parts take
    /// their generations from.
    fn with_workspace<R, E: From<String>>(
        &mut self,
        id: WorkspaceId,
        edit: impl FnOnce(&mut WorkspaceRecord, &mut Clock, &ManagedContent) -> Result<R, E>,
    ) -> Result<R, E> {
        let entry = self.workspaces.get_mut(&id).ok_or_else(|| E::from(format!("workspace {id} is not registered")))?;
        edit(&mut entry.record, &mut self.clock, &self.managed)
    }

    /// A registered workspace's slots, in default placement order.
    pub fn slots(&self, workspace: WorkspaceId) -> Result<Vec<SlotInfo>, String> {
        Ok(self.workspace(workspace)?.slots.get().slots())
    }

    /// Add the user's own slot (`u:<id>`), or edit a baseline slot: other
    /// content overrides it, detaching it, and another policy is a rebind
    /// edit (see [`WorkspaceSlots::set`](crate::slots::WorkspaceSlots::set)).
    /// The host gives a new slot a tab in its next commit, which otherwise
    /// places it. A facet's entity names its provider.
    pub fn set_slot(&mut self, workspace: WorkspaceId, key: &str, spec: ViewSpec, rebind: RebindPolicy) -> Result<bool, String> {
        self.with_workspace(workspace, |record, clock, _| record.set_slot(clock, key, spec, rebind))
    }

    /// Remove a slot: the user's own slot or a detached slot its baseline
    /// dropped goes; a baseline slot gets a tombstone (see
    /// [`WorkspaceSlots::remove`](crate::slots::WorkspaceSlots::remove)).
    pub fn remove_slot(&mut self, workspace: WorkspaceId, key: &str) -> Result<bool, String> {
        let removed = self.with_workspace(workspace, |record, clock, _| record.remove_slot(clock, key))?;
        if removed {
            self.managed.forget_slot(workspace, key);
        }
        Ok(removed)
    }

    /// Drop a baseline slot's override or tombstone, so it follows the
    /// provider again.
    pub fn reattach_slot(&mut self, workspace: WorkspaceId, key: &str) -> Result<bool, String> {
        self.with_workspace(workspace, |record, clock, _| Ok(record.reattach_slot(clock, key)))
    }

    /// Set the user's name for a workspace over the provider's, or drop it
    /// with `None`. Returns whether it changed.
    pub fn set_workspace_name(&mut self, workspace: WorkspaceId, name: Option<String>) -> Result<bool, String> {
        self.with_workspace(workspace, |record, clock, _| Ok(record.set_name(clock, name)))
    }

    /// Set the user's mood for a workspace over the provider's, or drop it
    /// with `None`. Returns whether it changed.
    pub fn set_workspace_mood(&mut self, workspace: WorkspaceId, mood: Option<String>) -> Result<bool, String> {
        self.with_workspace(workspace, |record, clock, _| Ok(record.set_mood(clock, mood)))
    }

    /// The host has dealt with the live instance a rebind replaced: a
    /// `keep-previous` slot's previous instance, or the instance of a slot
    /// the provider removed. Returns whether there was one.
    pub fn release_slot_previous(&mut self, workspace: WorkspaceId, key: &str) -> Result<bool, String> {
        let released = self.with_workspace(workspace, |record, clock, _| Ok::<_, String>(record.release_departed(clock, key)))?;
        Ok(self.managed.release_previous(workspace, key) || released)
    }

    /// A workspace's overlay: its baseline version, names and mood, and what
    /// the host should know about it (removed slots, departed instances).
    pub fn overlay(&self, workspace: WorkspaceId) -> Result<OverlayView, String> {
        let record = self.workspace(workspace)?;
        // The subject's label now, else as the record kept it: the record
        // follows the label while the workspace is open, so it is the one
        // fallback, after a restart before the provider publishes, or once
        // the subject ends.
        let saved = |subject: &EntityRef| record.subject.get().retained.get(subject).and_then(|saved| saved.label.clone());
        let subject_name = record.subject().and_then(|subject| self.state.subject_label(subject).or_else(|| saved(subject)));
        Ok(record.overlay(subject_name))
    }

    /// Plan one slot, given the identity of the resolution the host applied
    /// to it (empty for none). See [`crate::managed`].
    pub fn plan_slot(&mut self, workspace: WorkspaceId, key: &str, applied: &str) -> Result<SlotPlan, String> {
        let record = self.workspace(workspace)?;
        let slot = record.slot(key)?;
        let source = record.slot_source(&slot);
        let saved = record.resolutions.get().get(key).cloned();
        let mut plan = self.managed.plan_slot(workspace, key, source, slot.rebind, applied);
        plan.saved = saved;
        Ok(plan)
    }

    /// The portable Target Resolution saved for a slot, if any (see
    /// [`crate::target_resolution`]).
    pub fn slot_resolution(&self, workspace: WorkspaceId, key: &str) -> Result<Option<SavedResolution>, String> {
        self.workspace(workspace)?.resolution(key)
    }

    /// Save a slot's portable Target Resolution in the workspace record, if
    /// `expected` is the slot's saved generation (0 for none). `applied` is
    /// the identity of the slot resolution it resolves, as the host passes
    /// it to plan: when the slot's resolution is known to be another, the
    /// save is invalid. Returns the generation after; saving what is saved
    /// keeps it. Only the record and the content revision change.
    pub fn set_slot_resolution(
        &mut self,
        workspace: WorkspaceId,
        key: &str,
        resolution: PortableResolution,
        applied: &str,
        expected: u64,
    ) -> Result<u64, CommitError> {
        self.with_workspace(workspace, |record, clock, managed| record.set_resolution(clock, managed, key, resolution, applied, expected))
    }

    /// Clear a slot's saved portable Target Resolution, if `expected` is its
    /// saved generation. Returns whether there was one.
    pub fn clear_slot_resolution(&mut self, workspace: WorkspaceId, key: &str, expected: u64) -> Result<bool, CommitError> {
        self.with_workspace(workspace, |record, clock, _| record.clear_resolution(clock, key, expected))
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
        doc: ArrangementDoc,
        expected: u64,
    ) -> Result<ArrangementCommit, CommitError> {
        self.with_workspace(workspace, |record, clock, _| record.commit_arrangement(clock, doc, expected))
    }

    /// Settle a provider change to an arrangement the user owns: keep the
    /// user's, or follow the provider's again (which also drops soft
    /// overrides). Returns the generation after the call.
    pub fn resolve_arrangement(&mut self, workspace: WorkspaceId, choice: Resolve, expected: u64) -> Result<u64, CommitError> {
        self.with_workspace(workspace, |record, clock, _| record.resolve_arrangement(clock, choice, expected))
    }

    /// A registered workspace's arrangement. Before anything is stored it is
    /// empty, at generation 0.
    pub fn arrangement(&self, workspace: WorkspaceId) -> Result<ArrangementView, String> {
        Ok(self.workspace(workspace)?.arrangement_view())
    }

    /// What the template and local sections declare, for the sidebar
    /// arrangement.
    fn sidebar_declared(&self) -> Declared {
        let sections_with_default: BTreeSet<&str> = self
            .local
            .iter()
            .filter(|(entity, facts)| entity.kind == system::GROUP && matches!(facts.get(system::DEFAULT), Some(MetadataValue::Bool(true))))
            .filter_map(|(_, facts)| match facts.get(system::SECTION) {
                Some(MetadataValue::EntityRefs(refs)) if refs.len() == 1 => Some(refs[0].id.as_str()),
                _ => None,
            })
            .collect();
        let local: Vec<LocalSection> = self
            .local
            .iter()
            .filter(|(entity, _)| entity.kind == system::SECTION)
            .map(|(entity, facts)| LocalSection {
                id: entity.id.clone(),
                position: match facts.get(".position") {
                    Some(MetadataValue::Integer(n)) => Some(*n),
                    Some(MetadataValue::Text(text)) => text.trim().parse().ok(),
                    _ => None,
                },
                default: sections_with_default.contains(entity.id.as_str()),
            })
            .collect();
        Declared::new(&self.state.sidebar_regions(), &local)
    }

    /// Reconcile the sidebar arrangement after the declarations may have
    /// changed: on configure, and when local sections or groups change.
    fn follow_sidebar_declarations(&mut self) {
        let declared = self.sidebar_declared();
        let mut report = Report::default();
        self.sidebar_arrangement.follow(&mut self.clock, |stored| report = stored.follow(&declared));
        self.sidebar_report = report;
    }

    /// Adopt an imported arrangement, reconciled, at a new generation.
    fn import_sidebar_arrangement(&mut self, imported: SidebarArrangement) {
        let (next, report) = sidebar_arrangement::reconcile(&self.sidebar_declared(), &imported);
        self.sidebar_arrangement.follow(&mut self.clock, |stored| *stored = next);
        self.sidebar_report = report;
    }

    /// The Dashboard's sidebar arrangement: the dock and floating panels,
    /// closed sections, flags, and what its last reconciliation did. Its
    /// unresolved keys include saved dashboard keys the template no longer
    /// resolves.
    pub fn sidebar_arrangement(&self) -> (SidebarView, &Report) {
        let declared = self.sidebar_declared();
        let mut view = self.sidebar_arrangement.get().view(self.sidebar_arrangement.generation(), &declared);
        view.unresolved.extend(self.unresolved_dashboard_keys(&declared));
        (view, &self.sidebar_report)
    }

    /// Saved dashboard keys the template no longer resolves, as
    /// `display:<name>`, `collapse:<key>`, `order:<loop key>` and
    /// `variable:<key>`. They are kept, never dropped, and resolve again if
    /// the template brings their names back.
    fn unresolved_dashboard_keys(&self, declared: &Declared) -> Vec<String> {
        let (loops, display) = self.state.template_names();
        let loop_resolves = |name: &str| loops.contains(name) || name == UNPLACED_WORKSPACES_SECTION;
        let key_resolves = |key: &PlacementKey| key.0.iter().all(|s| loop_resolves(&s.loop_name));
        let key_text = |key: &PlacementKey| {
            key.0.iter().map(|s| format!("{}={}", s.loop_name, s.entity.action_target())).collect::<Vec<_>>().join("/")
        };
        let mut unresolved = Vec::new();
        for name in self.state.recorded_display_variables().keys() {
            if !display.contains(name) {
                unresolved.push(format!("display:{name}"));
            }
        }
        for key in self.state.collapsed_placements() {
            if !key_resolves(key) {
                unresolved.push(format!("collapse:{}", key_text(key)));
            }
        }
        for key in self.state.sibling_orders().keys() {
            if !(declared.resolves(&key.region) && loop_resolves(&key.binding) && key_resolves(&key.parent)) {
                unresolved.push(format!("order:{}", key.encode()));
            }
        }
        for key in self.state.placement_variables().keys() {
            if !key_resolves(key) {
                unresolved.push(format!("variable:{}", key_text(key)));
            }
        }
        unresolved
    }

    /// The sidebar arrangement's generation: 0 before anything is stored,
    /// and changed whenever it is. Cheap enough to poll.
    pub fn sidebar_arrangement_generation(&self) -> u64 {
        self.sidebar_arrangement.generation()
    }

    /// Commit the host's whole sidebar arrangement, if `expected` is its
    /// generation. Declared sections it leaves out are closed. A stale or
    /// invalid commit changes nothing; neither invalidates the snapshot.
    pub fn set_sidebar_arrangement(&mut self, doc: SidebarDoc, expected: u64) -> Result<Report, CommitError> {
        self.edit_sidebar_arrangement(expected, |stored, declared| stored.commit(declared, doc))
    }

    /// Reopen a closed section by its hints, with an equal share of its host.
    pub fn restore_sidebar_section(&mut self, key: &str, expected: u64) -> Result<Report, CommitError> {
        self.edit_sidebar_arrangement(expected, |stored, declared| stored.restore(declared, key))
    }

    /// Return the sidebar to its default arrangement: every declared section
    /// placed by its hints, nothing closed.
    pub fn reset_sidebar_arrangement(&mut self, expected: u64) -> Result<Report, CommitError> {
        self.edit_sidebar_arrangement(expected, |stored, declared| Ok(stored.reset(declared)))
    }

    /// A host's edit of the sidebar arrangement at `expected`, reconciled
    /// against what is declared.
    fn edit_sidebar_arrangement(
        &mut self,
        expected: u64,
        edit: impl FnOnce(&mut SidebarArrangement, &Declared) -> Result<Report, String>,
    ) -> Result<Report, CommitError> {
        let declared = self.sidebar_declared();
        let (_, report) = self.sidebar_arrangement.update(Some(expected), &mut self.clock, |stored| Ok(edit(stored, &declared)?))?;
        self.sidebar_report = report.clone();
        Ok(report)
    }

    /// Conservative revision of presentation and action dependencies. Unchanged
    /// heartbeats and ticks preserve it; recipe changes invalidate it even when
    /// the rendered content is unchanged.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Every change to what the sidebar presents comes through here, the
    /// Dashboard's state among it, so this is also where the dashboard
    /// record takes a new generation when its state changed.
    fn invalidate(&mut self) {
        self.revision = self.clock.tick();
        self.snapshot.take();
        self.evaluation.take();
        self.details.get_mut().clear();
        let state = self.dashboard_state();
        self.dashboard.follow(&mut self.clock, |current| *current = state);
    }

    fn evaluation(&self) -> &RevisionEvaluation {
        self.evaluation.get_or_init(|| self.state.evaluate_revision())
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
                .map(|(entity, message)| ActivationError { entity: entity.clone(), message: message.clone() })
                .collect(),
        })
    }

    /// The surface a first evaluation of the current state presents, with
    /// nothing kept from earlier evaluations. Kept state must not change it.
    #[cfg(test)]
    pub(crate) fn cold_surface(&self) -> SurfaceSnapshot {
        self.state.cold(|state| state.presentation_in(&state.evaluate_revision()))
    }

    pub fn detail_cards(&self) -> (u64, Vec<DetailCard>) {
        (self.state.evaluation_time(), self.state.detail_cards_in(self.evaluation(), None))
    }

    /// Exact catalog identity, including hidden and retained ended subjects.
    /// Cache at most 64 cards (including misses), and discard on revision change.
    pub fn detail_card(&self, entity: &EntityRef) -> Option<DetailCard> {
        if let Some(card) = self.details.borrow().get(entity) {
            return card.clone();
        }
        let card = self.state.detail_cards_in(self.evaluation(), Some(entity)).pop();
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
        self.state.subject_url(entity)
    }

    pub fn dispatch(&mut self, action: Action) -> Result<Vec<HostEffect>, String> {
        match action {
            Action::CopySubjectUrl { entity } => {
                let url = self.state.subject_url(&entity).ok_or("subject URL unavailable")?;
                Ok(vec![HostEffect::CopyUrl { url }])
            }
            Action::ActivatePlacement { key } => {
                let node = self.snapshot_shared().surface.node(&key).ok_or("unknown placement")?;
                // A reference activates what it presents.
                let entity = self.state.presented_entity(&node.entity);
                if let Some(url) = self.state.subject_url(&entity) {
                    return Ok(vec![HostEffect::OpenUrl { url }]);
                }
                if self.entity_is_pending(&entity) {
                    return Ok(vec![]);
                }
                if let PresentationState::Live { workspace_id, .. } = node.state {
                    if !self.state.workspaces().iter().any(|w| w.tab_id == workspace_id) {
                        return Err("workspace disappeared".into());
                    }
                    self.errors.remove(&entity);
                    let request_id = self.request_id();
                    self.pending.insert(request_id, Pending::Focus(entity));
                    self.invalidate();
                    Ok(vec![HostEffect::Focus { request_id, workspace_id }])
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
                        let workspace_id =
                            self.state.workspaces().iter().find(|w| w.position == position).ok_or("workspace disappeared")?.tab_id;
                        let request_id = self.request_id();
                        self.pending.insert(request_id, Pending::Focus(entity));
                        HostEffect::Focus { request_id, workspace_id }
                    }
                    Some(EntityActivation::Materialize(request)) => {
                        if !self.state.begin_latent_materialization(&request) {
                            return Ok(vec![]);
                        }
                        let request_id = self.request_id();
                        let primary_target = self.state.managed_primary_target(&entity, &request.recipe, request.checkout_path.as_deref());
                        let effect = HostEffect::Materialize {
                            request_id,
                            entity: entity.clone(),
                            name: request.name.clone(),
                            recipe: request.recipe.clone(),
                            cwd: request.checkout_path.clone(),
                            primary_target,
                        };
                        self.pending.insert(request_id, Pending::Materialize(entity, request));
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
                self.state.apply_rail_ui_action(RailUiAction::TogglePlacement { key });
                self.invalidate();
                Ok(vec![])
            }
            Action::SetVariable { key, name, value } => {
                let snapshot = self.snapshot_shared();
                let node = snapshot.surface.node(&key).ok_or("unknown placement")?;
                let definition =
                    node.variables.as_ref().and_then(|v| v.declarations.iter().find(|d| d.name == name)).ok_or("unknown variable")?;
                if value.as_ref().is_some_and(|value| !definition.accepts(value)) {
                    return Err("value is not allowed by the variable declaration".into());
                }
                self.state.set_node_variable(NodeKey::Placement(key), name, value);
                self.invalidate();
                Ok(vec![])
            }
            Action::ToggleDisplayVariable { name } => {
                if !self.state.apply_rail_ui_action(RailUiAction::ToggleVariable { name }) {
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
    pub fn complete(&mut self, request_id: u64, result: Result<Option<WorkspaceId>, String>) -> bool {
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
                    self.ensure_workspace(id);
                    self.state.bind_materializing_subject(&request, id, entity.clone());
                    // If observation arrived before acknowledgement, claim now.
                    let current = self.state.workspaces().to_vec();
                    self.state.observe_workspaces(current);
                }
                other => {
                    self.state.abort_latent_materialization(&request);
                    self.errors.insert(entity, other.err().unwrap_or_else(|| "materialize returned no workspace ID".into()));
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
fn retained_workspace_expired(_state: &ControllerState, _subjects: &BTreeSet<EntityRef>) -> bool {
    false
}

#[cfg(test)]
mod evaluation_tests {
    const PROVIDER: &str = "test";
    use super::*;
    use crate::{tab_workspace_id, SortMode};

    const CONFIG: &str = include_str!("../tests/fixtures/sidebar.kdl");
    fn entity() -> EntityRef {
        EntityRef::new(PROVIDER, "vessel", "v")
    }
    fn patch(source: &str, label: &str) -> MetadataPatch {
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
                    MetadataValueUpdate { value: MetadataValue::Text(value.into()), ttl_ms: None, precedence: None, ordinal: None },
                )
            })
            .collect(),
        }
    }
    fn add_project(sidebar: &mut Sidebar) {
        sidebar
            .apply(
                100,
                PROVIDER,
                [MetadataPatch {
                    target: MetadataTarget::Entity(EntityRef::new(PROVIDER, "project", "p")),
                    source_id: "base".into(),
                    unset: vec![],
                    set: [(
                        "flotilla.project".into(),
                        MetadataValueUpdate { value: MetadataValue::Text("p".into()), ttl_ms: None, precedence: None, ordinal: None },
                    )]
                    .into(),
                }],
            )
            .unwrap();
    }
    fn check(sidebar: &Sidebar) {
        // Issue #96: cached presentation and exact details must equal an uncached
        // controller evaluation after every mutation, including identities without rows.
        assert_eq!(sidebar.snapshot_shared().surface, sidebar.state.view_model().presentation.unwrap());
        let (_, all) = sidebar.state.detail_cards();
        for card in &all {
            assert_eq!(sidebar.detail_card(&card.entity).as_ref(), Some(card));
        }
        assert_eq!(sidebar.detail_cards().1, all);
    }

    #[test]
    fn revision_queries_share_evaluation_and_validation() {
        let mut sidebar = Sidebar::new(CONFIG).unwrap();
        sidebar.apply(100, PROVIDER, [patch("a", "first")]).unwrap();
        add_project(&mut sidebar);
        profile::start();
        let snapshot = sidebar.snapshot();
        let key = snapshot.surface.sections[0].nodes[0].children[0].key.clone();
        sidebar.detail_card(&entity()).unwrap();
        sidebar.detail_cards();
        sidebar.detail_card(&entity()).unwrap();
        assert_eq!(profile::take()["catalog"].calls, 1);
        sidebar.dispatch(Action::TogglePlacement { key }).unwrap();
        // Validation used the rendered snapshot; only the new output evaluates.
        assert!(!profile::take().contains_key("catalog"));
        assert_ne!(sidebar.snapshot().revision, snapshot.revision);
        assert_eq!(profile::take()["catalog"].calls, 1);
        check(&sidebar);
    }

    #[test]
    fn generated_revision_lifecycles_match_uncached_output() {
        // Deterministic operation generator spans duplicate observations, two
        // sources' writes, ticks, unsets, config variables, topology,
        // dispatch and completions. Real controller/store/template collaborators.
        for seed in 0..24u64 {
            let mut sidebar = Sidebar::new(CONFIG).unwrap();
            sidebar.apply(100, PROVIDER, [patch("base", "original")]).unwrap();
            add_project(&mut sidebar);
            let held = sidebar.snapshot();
            let mut now = 100;
            for step in 0..18 {
                match (seed + step * 7) % 9 {
                    0 => sidebar.apply(now, PROVIDER, [patch("update", &format!("{seed}/{step}"))]).unwrap(),
                    1 => {
                        now += 5;
                        sidebar.tick(now);
                    }
                    2 => {
                        now += 1;
                        sidebar.tick(now);
                    }
                    3 => {
                        let mut p = patch("update", "");
                        p.set.clear();
                        p.unset = vec!["display.label".into()];
                        sidebar.apply(now, PROVIDER, [p]).unwrap();
                    }
                    4 => sidebar.configure(&CONFIG.replace("default=\"roomy\"", "default=\"compact\"")).unwrap(),
                    5 => sidebar.observe(
                        vec![Workspace { id: tab_workspace_id(1), position: 0, name: "vessel:v".into(), selected: step % 2 == 0 }],
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
                            sidebar.dispatch(Action::SetVariable { key, name: "density".into(), value: Some("compact".into()) }).unwrap();
                        }
                    }
                    _ => {
                        for effect in sidebar.dispatch(Action::Activate { entity: entity() }).unwrap() {
                            match effect {
                                HostEffect::Materialize { request_id, .. } | HostEffect::Focus { request_id, .. } => {
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
            assert_eq!(held.surface.sections[0].nodes[0].children[0].label, "original");
        }
    }

    // Semantic output must not depend on whether tab metadata was resolved in
    // inventory or controller sort order, including a pin-driven reorder.
    #[test]
    fn presentation_matches_controller_for_every_sort_mode() {
        for mode in [SortMode::Controller, SortMode::Position, SortMode::PinnedFirst, SortMode::LatestStatus] {
            let mut sidebar = Sidebar::new(CONFIG).unwrap();
            sidebar.apply(100, PROVIDER, [patch("base", "original")]).unwrap();
            add_project(&mut sidebar);
            sidebar.observe(
                vec![
                    Workspace { id: tab_workspace_id(1), position: 1, name: "vessel:v".into(), selected: true },
                    Workspace { id: tab_workspace_id(2), position: 0, name: "Other".into(), selected: false },
                ],
                vec![],
            );
            sidebar.state.set_sort_mode(mode);
            sidebar.state.toggle_pin(tab_workspace_id(1));
            sidebar.invalidate();
            check(&sidebar);
            let tabs = sidebar.state.view_model().tabs;
            assert_eq!(tabs[0].tab_id, tab_workspace_id(if mode == SortMode::PinnedFirst { 1 } else { 2 }));
        }
    }

    #[test]
    fn exact_detail_cache_is_bounded_and_invalidates_misses() {
        let mut sidebar = Sidebar::new(CONFIG).unwrap();
        // Missing, empty and invalid identities are cached without growing forever.
        for i in 0..200 {
            assert!(sidebar.detail_card(&EntityRef::new(PROVIDER, "missing", i.to_string())).is_none());
            assert!(sidebar.details.borrow().len() <= 64);
        }
        assert!(sidebar.detail_card(&entity()).is_none());
        sidebar.apply(100, PROVIDER, [patch("base", "appeared")]).unwrap();
        assert_eq!(sidebar.detail_card(&entity()).unwrap().label, "appeared");
    }
}

#[cfg(test)]
impl Sidebar {
    /// Count what the sidebar holds: its snapshot's nodes first, then its
    /// state and what that keeps (counting only what the snapshot doesn't
    /// share).
    pub(crate) fn census(&self, census: &mut Census) {
        for section in &self.snapshot_shared().surface.sections {
            for node in &section.nodes {
                if census.first(node) {
                    node.census(census, "snapshot nodes");
                }
            }
        }
        self.state.census(census);
    }
}
