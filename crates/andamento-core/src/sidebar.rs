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
    state::{ControllerState, ControllerTab, EntityActivation},
    EntityRef, MaterializeLatentRequest, MetadataPatch, NodeKey, PlacementKey, RailUiAction,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: u64,
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
    Apply {
        now_ms: u64,
        patches: Vec<MetadataPatch>,
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
        workspace_id: Option<u64>,
        error: Option<String>,
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
        workspace_id: u64,
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
    retained_paths: BTreeMap<u64, BTreeSet<EntityRef>>,
    retained_paths_revision: Option<u64>,
    pub managed: crate::managed::ManagedContent,
}

impl Sidebar {
    pub fn handle(&mut self, request: Request) -> Result<Response, String> {
        let effects = match request {
            Request::Configure { kdl } => {
                self.configure(&kdl)?;
                vec![]
            }
            Request::Apply { now_ms, patches } => {
                self.apply(now_ms, patches);
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
        self.invalidate();
        Ok(())
    }

    /// Apply a drained batch before requesting a snapshot. Time is monotonic
    /// milliseconds in this instance, supplied by the host; an empty batch is a tick.
    pub fn apply(&mut self, now_ms: u64, patches: impl IntoIterator<Item = MetadataPatch>) {
        let _phase = crate::profile::span("apply");
        let maintenance = crate::profile::span("workspace-maintenance");
        let subjects = self.state.workspace_subjects();
        let mut changed = self.retain_workspace_paths(&subjects);
        drop(maintenance);
        changed |= self.state.advance_time(now_ms);
        let patch_phase = crate::profile::span("patch-application");
        for patch in patches {
            changed |= self.state.apply_metadata_patch(patch);
        }
        drop(patch_phase);
        let maintenance = crate::profile::span("workspace-maintenance");
        changed |= self.state.refresh_retained_subjects();
        changed |= self
            .state
            .mark_ended_workspace_paths(&self.retained_paths, &subjects);
        drop(maintenance);
        if changed {
            self.managed.publish(self.state.managed_content());
            self.invalidate();
        }
    }

    fn retain_workspace_paths(&mut self, subjects: &BTreeMap<u64, EntityRef>) -> bool {
        fn collect(
            nodes: &[crate::presentation::PlacementNode],
            ancestors: &mut Vec<EntityRef>,
            paths: &mut BTreeMap<u64, BTreeSet<EntityRef>>,
        ) {
            for node in nodes {
                let subject = node.entity.kind != "andamento.workspace";
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
        self.retained_paths.retain(|id, subjects_on_path| {
            subjects.contains_key(id) && !retained_workspace_expired(&self.state, subjects_on_path)
        });
        self.state
            .retain_workspace_paths(self.retained_paths.values().flatten().cloned().collect())
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
            self.managed.publish(self.state.managed_content());
            self.invalidate();
        }
    }

    /// Replace terminal directory observations after supplying workspace topology.
    /// Matching is exact and host-normalized; explicit workspace identity wins.
    /// Several terminal directories may belong to one workspace. Associations
    /// only affect focus/presentation and never enroll content for replacement.
    pub fn observe_workdirs(&mut self, workdirs: Vec<(u64, String)>) {
        if self.state.observe_workdirs(workdirs) {
            self.invalidate();
        }
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
        self.state.subject_url(entity)
    }

    pub fn dispatch(&mut self, action: Action) -> Result<Vec<HostEffect>, String> {
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
                let entity = node.entity.clone();
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
    pub fn complete(&mut self, request_id: u64, result: Result<Option<u64>, String>) -> bool {
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
    use super::*;
    use crate::{MetadataTarget, MetadataValue, MetadataValueUpdate};

    const CONFIG: &str = include_str!("../tests/fixtures/sidebar.kdl");
    fn entity() -> EntityRef {
        EntityRef {
            kind: "vessel".into(),
            id: "v".into(),
        }
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
                target: MetadataTarget::Entity(EntityRef {
                    kind: "project".into(),
                    id: "p".into(),
                }),
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
                            id: 1,
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
                        id: 1,
                        position: 1,
                        name: "vessel:v".into(),
                        selected: true,
                    },
                    Workspace {
                        id: 2,
                        position: 0,
                        name: "Other".into(),
                        selected: false,
                    },
                ],
                vec![],
            );
            sidebar.state.set_sort_mode(mode);
            sidebar.state.toggle_pin(1);
            sidebar.invalidate();
            check(&sidebar);
            let tabs = sidebar.state.view_model().tabs;
            assert_eq!(
                tabs[0].tab_id,
                if mode == crate::SortMode::PinnedFirst {
                    1
                } else {
                    2
                }
            );
        }
    }

    #[test]
    fn exact_detail_cache_is_bounded_and_invalidates_misses() {
        let mut sidebar = Sidebar::new(CONFIG).unwrap();
        // Missing, empty and invalid identities are cached without growing forever.
        for i in 0..200 {
            assert!(sidebar
                .detail_card(&EntityRef {
                    kind: "missing".into(),
                    id: i.to_string()
                })
                .is_none());
            assert!(sidebar.details.borrow().len() <= 64);
        }
        assert!(sidebar.detail_card(&entity()).is_none());
        sidebar.apply(100, [patch("base", "appeared", None, 0)]);
        assert_eq!(sidebar.detail_card(&entity()).unwrap().label, "appeared");
    }
}
