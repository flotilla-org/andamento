mod metadata;
mod state;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use state::ControllerState;
use tabs_shared::PaneTarget;
use tabs_shared::PluginStatsRecorder;
use tabs_shared::MSG_RAIL_SIZE_TARGET;
use tabs_shared::MSG_VIEW_MODEL;
use tabs_shared::{
    ControllerBootstrapSnapshot, ExternalMessage, RailConfig, RailGroupingMode, RailSize,
    RailSizeObserved, RailSizeTarget, RailSizingPreset, RailStructure, RailViewMode, RendererHello,
    SortMode, StatsCollectRequest, MSG_APPLY_METADATA_PATCH, MSG_CLEAR_PANE_STATUS,
    MSG_CONFIG_EDITOR_HELLO, MSG_CONTROLLER_BOOTSTRAP_REQUEST, MSG_CONTROLLER_BOOTSTRAP_STATE,
    MSG_OBSERVED_IDENTITIES, MSG_RAIL_SIZE_OBSERVED, MSG_RENDERER_HELLO, MSG_REQUEST_STATE,
    MSG_SET_PANE_STATUS, MSG_SET_RAIL_CONFIG, MSG_SET_SORT_MODE, MSG_STATS_COLLECT, MSG_TOGGLE_PIN,
};
use tabs_shared::{TemplateConfigDiagnostics, TemplateConfigState};
use tabs_shared::{MSG_STATS_REPORT, MSG_STATS_REQUEST};
use zellij_tile::output::print;
use zellij_tile::prelude::*;

#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
const TEMPLATE_RELOAD_RETRY_SECS: f64 = 0.25;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
const RAIL_SIZE_SYNC_DEBOUNCE_SECS: f64 = 0.05;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
const VIEW_MODEL_PUSH_COALESCE_SECS: f64 = 0.01;
const MAX_TEMPLATE_RELOAD_ATTEMPTS: u8 = 20;

#[cfg(not(target_family = "wasm"))]
fn main() {}

#[derive(Default)]
pub struct PluginState {
    state: ControllerState,
    template_config_path: Option<String>,
    grouping_config_path: Option<String>,
    permissions_granted: bool,
    own_identity: Option<RendererHello>,
    bootstrap_requested: bool,
    template_reload_pending: bool,
    template_reload_attempts: u8,
    grouping_rule_count: usize,
    grouping_config_error: Option<String>,
    stats: PluginStatsRecorder,
    pending_view_model_push: PendingViewModelPush,
    pending_rail_size_sync: PendingRailSizeSync,
}

register_plugin!(PluginState);

impl ZellijPlugin for PluginState {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_identity = Some(RendererHello {
            plugin_id: ids.plugin_id,
            client_id: ids.client_id,
        });
        self.state
            .set_rail_config(parse_rail_config(&configuration));
        self.template_config_path = template_config_path_from_configuration(&configuration);
        self.grouping_config_path = grouping_config_path_from_configuration(&configuration);
        self.state
            .set_template_config_diagnostics(initial_template_config_diagnostics(
                self.template_config_path.clone(),
            ));
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            PermissionType::MessageAndLaunchOtherPlugins,
            PermissionType::OpenFiles,
            PermissionType::FullHdAccess,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::CwdChanged,
            EventType::PermissionRequestResult,
            EventType::FailedToChangeHostFolder,
            EventType::Timer,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        self.stats.increment("update.total");
        match event {
            Event::PermissionRequestResult(status) => {
                self.stats.increment("update.permission-result");
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    change_host_folder(PathBuf::from("/"));
                    self.schedule_template_reload();
                }
                self.request_bootstrap_snapshot();
                return true;
            }
            Event::FailedToChangeHostFolder(error) => {
                self.stats.increment("update.failed-host-folder");
                self.template_reload_pending = false;
                self.state
                    .set_template_config_diagnostics(template_config_error_diagnostics(
                        self.template_config_path.clone(),
                        format!(
                            "failed to set /host root: {}",
                            error.unwrap_or_else(|| "unknown error".to_owned())
                        ),
                    ));
                self.push_view_model_to_rails_with_pending(ViewModelPushReason::UpdateHostFolder);
                return true;
            }
            Event::Timer(seconds) => {
                self.stats.increment("update.timer");
                self.stats
                    .add("update.timer.elapsed-ms", (seconds * 1000.0) as u64);
                let mut should_render = false;
                // Timer events carry elapsed wall time, not the requested timeout,
                // so route them by pending controller state instead of by value.
                if self.template_reload_pending {
                    let loaded = self.retry_template_catalog_reload();
                    self.push_view_model_to_rails_with_pending(ViewModelPushReason::UpdateTemplate);
                    should_render |= loaded || self.template_reload_pending;
                }
                if self.pending_view_model_push.is_pending() {
                    self.flush_pending_view_model_push();
                    should_render = true;
                }
                if self.pending_rail_size_sync.is_pending() {
                    self.flush_pending_rail_size_sync();
                }
                return should_render;
            }
            Event::TabUpdate(tabs) => {
                self.stats.increment("update.tab");
                if self.state.update_tabs_from_zellij(tabs) {
                    self.queue_view_model_push(ViewModelPushReason::UpdateTab);
                }
            }
            Event::PaneUpdate(pane_manifest) => {
                self.stats.increment("update.pane");
                let live_plugin_ids = pane_manifest
                    .panes
                    .values()
                    .flat_map(|panes| panes.iter())
                    .filter(|pane| pane.is_plugin)
                    .map(|pane| pane.id)
                    .collect();
                let rails_before_retain = self.state.known_rail_count();
                let config_editors_before_retain = self.state.known_config_editor_count();
                self.state.retain_rails(&live_plugin_ids);
                self.stats
                    .add("retain-rails.rails-before", rails_before_retain as u64);
                self.stats.add(
                    "retain-rails.config-editors-before",
                    config_editors_before_retain as u64,
                );
                self.stats.add(
                    "retain-rails.rails-after",
                    self.state.known_rail_count() as u64,
                );
                self.stats.add(
                    "retain-rails.config-editors-after",
                    self.state.known_config_editor_count() as u64,
                );
                let mut state_changed = self.state.update_panes_from_manifest(pane_manifest);
                for terminal_id in self.state.terminal_panes_for_cwd_refresh() {
                    if let Ok(cwd) = get_pane_cwd(PaneId::Terminal(terminal_id)) {
                        state_changed |= self.state.set_pane_cwd(
                            PaneTarget::Terminal(terminal_id),
                            cwd.display().to_string(),
                        );
                    }
                }
                if state_changed {
                    self.queue_view_model_push(ViewModelPushReason::UpdatePane);
                }
            }
            Event::CwdChanged(pane_id, cwd, _) => {
                self.stats.increment("update.cwd");
                let state_changed = if let PaneId::Terminal(id) = pane_id {
                    self.state
                        .set_pane_cwd(PaneTarget::Terminal(id), cwd.display().to_string())
                } else {
                    false
                };
                if state_changed {
                    self.queue_view_model_push(ViewModelPushReason::UpdateCwd);
                }
            }
            _ => {}
        }
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        self.stats.increment(format!("pipe.{}", pipe_message.name));
        let started_at = Instant::now();
        let result = handle_pipe_message(&mut self.state, pipe_message);
        self.stats
            .record_span_elapsed("pipe.handle-message", started_at);
        if let Some(requester) = result.bootstrap_request {
            if self.own_identity.as_ref() != Some(&requester) {
                self.send_bootstrap_snapshot_to(requester);
            }
        }
        if let Some(request) = result.stats_collect_request {
            self.send_stats_report_to(request.requester.clone(), request.collection_id);
            self.request_stats_from_plugins(request);
        }
        if let Some(observed) = result.rail_size_observed {
            self.queue_rail_size_sync(observed);
        }
        if let Some(output) = result.cli_pipe_output.as_ref() {
            cli_pipe_output(&output.pipe_id, &output.output);
            unblock_cli_pipe_input(&output.pipe_id);
        }
        if result.state_changed {
            self.push_view_model_to_rails_with_pending(
                result
                    .view_model_push_reason
                    .unwrap_or(ViewModelPushReason::PipeUnknown),
            );
        }
        false
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.stats.increment("render.total");
        let started_at = Instant::now();
        let diagnostics = self.state.template_config_diagnostics();
        let mut lines = vec![
            "andamento controller".to_owned(),
            format!(
                "permissions: {}",
                if self.permissions_granted {
                    "granted"
                } else {
                    "waiting"
                }
            ),
            format!(
                "template state: {}",
                template_config_state_text(diagnostics.state)
            ),
            format!(
                "template path: {}",
                diagnostics.path.as_deref().unwrap_or("<not configured>")
            ),
            format!("template count: {}", diagnostics.template_count),
        ];
        lines.push(format!(
            "grouping config path: {}",
            self.grouping_config_path
                .as_deref()
                .unwrap_or("<not configured>")
        ));
        lines.push(format!("grouping rule count: {}", self.grouping_rule_count));
        if let Some(error) = self.grouping_config_error.as_ref() {
            lines.push(format!("grouping config error: {error}"));
        }
        if !diagnostics.template_names.is_empty() {
            lines.push(format!(
                "template names: {}",
                diagnostics.template_names.join(", ")
            ));
        }
        if let Some(error) = diagnostics.last_error.as_ref() {
            lines.push(format!("template error: {error}"));
        }
        lines.truncate(rows);
        while lines.len() < rows {
            lines.push(String::new());
        }
        print!("{}", render_plain_lines(&lines, cols).join("\n"));
        self.stats
            .record_span_elapsed("render.controller-pane", started_at);
    }
}

impl PluginState {
    fn schedule_template_reload(&mut self) {
        if self.template_config_path.is_none() && self.grouping_config_path.is_none() {
            self.reload_template_catalog();
            self.reload_grouping_catalog();
            self.push_view_model_to_rails_with_pending(ViewModelPushReason::UpdateTemplate);
            return;
        }
        self.template_reload_pending = true;
        self.template_reload_attempts = 0;
        self.state
            .set_template_config_diagnostics(initial_template_config_diagnostics(
                self.template_config_path.clone(),
            ));
        set_timeout(TEMPLATE_RELOAD_RETRY_SECS);
        self.push_view_model_to_rails_with_pending(ViewModelPushReason::UpdateTemplate);
    }

    fn retry_template_catalog_reload(&mut self) -> bool {
        self.template_reload_attempts = self.template_reload_attempts.saturating_add(1);
        let loaded = self.reload_template_catalog() & self.reload_grouping_catalog();
        self.template_reload_pending =
            should_retry_template_load(self.template_reload_attempts, loaded);
        if self.template_reload_pending {
            set_timeout(TEMPLATE_RELOAD_RETRY_SECS);
        }
        loaded
    }

    fn reload_template_catalog(&mut self) -> bool {
        let Some(path) = self.template_config_path.as_deref() else {
            self.state.set_template_catalog(None);
            self.state
                .set_template_config_diagnostics(TemplateConfigDiagnostics::default());
            return true;
        };
        match tabs_shared::template_config::load_template_catalog_from_file(path) {
            Ok(catalog) => {
                let diagnostics = TemplateConfigDiagnostics {
                    path: Some(path.to_owned()),
                    state: TemplateConfigState::Loaded,
                    template_count: catalog.len(),
                    template_names: catalog.template_names(),
                    last_error: None,
                };
                self.state.set_template_catalog(Some(catalog));
                self.state.set_template_config_diagnostics(diagnostics);
                true
            }
            Err(error) => {
                eprintln!("tabs-controller: failed to load template config: {error}");
                self.state.set_template_catalog(None);
                self.state
                    .set_template_config_diagnostics(template_config_error_diagnostics(
                        Some(path.to_owned()),
                        error.to_string(),
                    ));
                false
            }
        }
    }

    fn reload_grouping_catalog(&mut self) -> bool {
        let Some(path) = self.grouping_config_path.as_deref() else {
            self.state.set_grouping_catalog(None);
            self.grouping_rule_count = 0;
            self.grouping_config_error = None;
            return true;
        };
        match tabs_shared::grouping_config::load_grouping_catalog_from_file(path) {
            Ok(catalog) => {
                self.grouping_rule_count = catalog.rules.len();
                self.grouping_config_error = None;
                self.state.set_grouping_catalog(Some(catalog));
                true
            }
            Err(error) => {
                eprintln!("tabs-controller: failed to load grouping config: {error}");
                self.grouping_rule_count = 0;
                self.grouping_config_error = Some(error.to_string());
                self.state.set_grouping_catalog(None);
                false
            }
        }
    }

    fn request_bootstrap_snapshot(&mut self) {
        if !self.permissions_granted || self.bootstrap_requested {
            return;
        }
        let Some(identity) = self.own_identity.as_ref() else {
            return;
        };
        let Ok(payload) = serde_json::to_string(identity) else {
            return;
        };
        self.bootstrap_requested = true;
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_CONTROLLER_BOOTSTRAP_REQUEST).with_payload(payload),
        );
    }

    fn send_bootstrap_snapshot_to(&mut self, requester: RendererHello) {
        if !self.permissions_granted {
            return;
        }
        let started_at = Instant::now();
        let Ok(payload) = serde_json::to_string(&self.state.bootstrap_snapshot()) else {
            return;
        };
        self.stats
            .record_span_elapsed("json.encode-bootstrap-snapshot", started_at);
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_CONTROLLER_BOOTSTRAP_STATE)
                .with_destination_plugin_id(requester.plugin_id)
                .with_destination_client_id(requester.client_id)
                .with_payload(payload),
        );
    }

    fn queue_view_model_push(&mut self, reason: ViewModelPushReason) {
        if self.template_reload_pending {
            self.push_view_model_to_rails_with_pending(reason);
            return;
        }
        self.stats.increment("view-model.push.queued");
        if self.pending_view_model_push.queue(reason) {
            set_timeout(VIEW_MODEL_PUSH_COALESCE_SECS);
        }
    }

    fn flush_pending_view_model_push(&mut self) {
        let reasons = self.pending_view_model_push.take();
        self.push_view_model_to_rails(reasons);
    }

    fn push_view_model_to_rails_with_pending(&mut self, reason: ViewModelPushReason) {
        let mut reasons = self.pending_view_model_push.take();
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
        self.push_view_model_to_rails(reasons);
    }

    fn push_view_model_to_rails(&mut self, reasons: Vec<ViewModelPushReason>) {
        if reasons.is_empty() {
            return;
        }
        if !self.permissions_granted {
            self.stats
                .increment("view-model.push.skipped.no-permission");
            return;
        }
        let rail_target_count = self.state.known_rail_count();
        let config_editor_target_count = self.state.known_config_editor_count();
        let targets = self.state.rail_plugin_targets();
        if targets.is_empty() {
            self.stats.increment("view-model.push.skipped.no-targets");
            return;
        }
        let started_at = Instant::now();
        let Ok(payload) = serde_json::to_string(&self.state.view_model()) else {
            return;
        };
        self.stats
            .record_span_elapsed("json.encode-view-model", started_at);
        self.stats.increment("view-model.push.sent");
        for reason in reasons {
            self.stats.increment(reason.counter_name());
        }
        self.stats.add("view-model.targets", targets.len() as u64);
        self.stats
            .add("view-model.targets.rails", rail_target_count as u64);
        self.stats.add(
            "view-model.targets.config-editors",
            config_editor_target_count as u64,
        );
        self.stats
            .add("view-model.payload-bytes", payload.len() as u64);
        for rail in targets {
            pipe_message_to_plugin(
                MessageToPlugin::new(MSG_VIEW_MODEL)
                    .with_destination_plugin_id(rail.plugin_id)
                    .with_destination_client_id(rail.client_id)
                    .with_payload(payload.clone()),
            );
        }
    }

    fn queue_rail_size_sync(&mut self, observed: RailSizeObserved) {
        if self.pending_rail_size_sync.queue(observed) {
            set_timeout(RAIL_SIZE_SYNC_DEBOUNCE_SECS);
        }
    }

    fn flush_pending_rail_size_sync(&mut self) {
        let targets = self.pending_rail_size_sync.take_targets();
        for target in targets {
            self.send_rail_size_target_to_rails(target);
        }
    }

    fn send_rail_size_target_to_rails(&self, target: RailSizeTarget) {
        let Ok(payload) = serde_json::to_string(&target) else {
            return;
        };
        for rail in self
            .state
            .rail_plugin_targets()
            .into_iter()
            .filter(|rail| rail.client_id == target.client_id)
        {
            pipe_message_to_plugin(
                MessageToPlugin::new(MSG_RAIL_SIZE_TARGET)
                    .with_destination_plugin_id(rail.plugin_id)
                    .with_destination_client_id(rail.client_id)
                    .with_payload(payload.clone()),
            );
        }
    }

    fn send_stats_report_to(&self, requester: RendererHello, collection_id: u64) {
        let Some(identity) = self.own_identity.as_ref() else {
            return;
        };
        let snapshot = self
            .stats
            .snapshot(collection_id, identity.clone(), "controller");
        let Ok(payload) = serde_json::to_string(&snapshot) else {
            return;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_STATS_REPORT)
                .with_destination_plugin_id(requester.plugin_id)
                .with_destination_client_id(requester.client_id)
                .with_payload(payload),
        );
    }

    fn request_stats_from_plugins(&self, request: StatsCollectRequest) {
        let Ok(payload) = serde_json::to_string(&request) else {
            return;
        };
        for target in self.state.rail_plugin_targets() {
            pipe_message_to_plugin(
                MessageToPlugin::new(MSG_STATS_REQUEST)
                    .with_destination_plugin_id(target.plugin_id)
                    .with_destination_client_id(target.client_id)
                    .with_payload(payload.clone()),
            );
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct HandlePipeResult {
    state_changed: bool,
    view_model_push_reason: Option<ViewModelPushReason>,
    bootstrap_request: Option<RendererHello>,
    cli_pipe_output: Option<CliPipeOutput>,
    stats_collect_request: Option<StatsCollectRequest>,
    rail_size_observed: Option<RailSizeObserved>,
}

#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewModelPushReason {
    UpdateTab,
    UpdatePane,
    UpdateCwd,
    UpdateTemplate,
    UpdateHostFolder,
    PipeStatus,
    PipeMetadata,
    PipeRendererHello,
    PipeConfigEditorHello,
    PipeTogglePin,
    PipeConfig,
    PipeRequestState,
    PipeBootstrap,
    PipeUnknown,
}

#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
impl ViewModelPushReason {
    fn counter_name(self) -> &'static str {
        match self {
            Self::UpdateTab => "view-model.push.reason.tab",
            Self::UpdatePane => "view-model.push.reason.pane",
            Self::UpdateCwd => "view-model.push.reason.cwd",
            Self::UpdateTemplate => "view-model.push.reason.template",
            Self::UpdateHostFolder => "view-model.push.reason.host-folder",
            Self::PipeStatus => "view-model.push.reason.pipe.status",
            Self::PipeMetadata => "view-model.push.reason.pipe.metadata",
            Self::PipeRendererHello => "view-model.push.reason.pipe.renderer-hello",
            Self::PipeConfigEditorHello => "view-model.push.reason.pipe.config-editor-hello",
            Self::PipeTogglePin => "view-model.push.reason.pipe.toggle-pin",
            Self::PipeConfig => "view-model.push.reason.pipe.config",
            Self::PipeRequestState => "view-model.push.reason.pipe.request-state",
            Self::PipeBootstrap => "view-model.push.reason.pipe.bootstrap",
            Self::PipeUnknown => "view-model.push.reason.pipe.unknown",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PendingViewModelPush {
    scheduled: bool,
    reasons: Vec<ViewModelPushReason>,
}

impl PendingViewModelPush {
    fn queue(&mut self, reason: ViewModelPushReason) -> bool {
        let should_schedule = !self.scheduled;
        self.scheduled = true;
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
        should_schedule
    }

    fn take(&mut self) -> Vec<ViewModelPushReason> {
        self.scheduled = false;
        std::mem::take(&mut self.reasons)
    }

    fn is_pending(&self) -> bool {
        self.scheduled
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
struct PendingRailSizeSync {
    scheduled: bool,
    pending_by_client: BTreeMap<u16, RailSize>,
    target_by_client: BTreeMap<u16, RailSizeTarget>,
}

impl PendingRailSizeSync {
    fn queue(&mut self, observed: RailSizeObserved) -> bool {
        if self
            .target_by_client
            .get(&observed.rail.client_id)
            .map(|target| target.size.within_tolerance(observed.size))
            .unwrap_or(false)
            || self
                .pending_by_client
                .get(&observed.rail.client_id)
                .map(|pending| pending.within_tolerance(observed.size))
                .unwrap_or(false)
        {
            return false;
        }
        let should_schedule = !self.scheduled;
        self.scheduled = true;
        self.pending_by_client
            .insert(observed.rail.client_id, observed.size);
        should_schedule
    }

    fn take_targets(&mut self) -> Vec<RailSizeTarget> {
        self.scheduled = false;
        let pending = std::mem::take(&mut self.pending_by_client);
        pending
            .into_iter()
            .filter_map(|(client_id, size)| {
                if self
                    .target_by_client
                    .get(&client_id)
                    .map(|target| target.size.within_tolerance(size))
                    .unwrap_or(false)
                {
                    return None;
                }
                let version = self
                    .target_by_client
                    .get(&client_id)
                    .map(|target| target.version.saturating_add(1))
                    .unwrap_or(1);
                let target = RailSizeTarget {
                    client_id,
                    size,
                    version,
                };
                self.target_by_client.insert(client_id, target.clone());
                Some(target)
            })
            .collect()
    }

    fn is_pending(&self) -> bool {
        self.scheduled
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliPipeOutput {
    pipe_id: String,
    output: String,
}

fn handle_pipe_message(state: &mut ControllerState, pipe_message: PipeMessage) -> HandlePipeResult {
    match parse_controller_message(&pipe_message) {
        Ok(Some(ControllerMessage::External(ExternalMessage::SetPaneStatus(status)))) => {
            state.set_status(status);
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeStatus),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::External(ExternalMessage::ClearPaneStatus { pane_id }))) => {
            state.clear_status(pane_id);
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeStatus),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::External(ExternalMessage::MetadataPatch(patch)))) => {
            let state_changed = state.apply_metadata_patch(patch);
            HandlePipeResult {
                state_changed,
                view_model_push_reason: state_changed.then_some(ViewModelPushReason::PipeMetadata),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::RendererHello(hello))) => {
            let state_changed = state.register_rail(hello);
            HandlePipeResult {
                state_changed,
                view_model_push_reason: state_changed
                    .then_some(ViewModelPushReason::PipeRendererHello),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::ConfigEditorHello(hello))) => {
            let state_changed = state.register_config_editor(hello);
            HandlePipeResult {
                state_changed,
                view_model_push_reason: state_changed
                    .then_some(ViewModelPushReason::PipeConfigEditorHello),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::TogglePin(tab_id))) => {
            state.toggle_pin(tab_id);
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeTogglePin),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::SetSortMode(sort_mode))) => {
            state.set_sort_mode(sort_mode);
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeConfig),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::SetRailConfig(config))) => {
            state.set_rail_config(config);
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeConfig),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::RequestState)) => HandlePipeResult {
            state_changed: true,
            view_model_push_reason: Some(ViewModelPushReason::PipeRequestState),
            ..HandlePipeResult::default()
        },
        Ok(Some(ControllerMessage::BootstrapRequest(requester))) => HandlePipeResult {
            bootstrap_request: Some(requester),
            ..HandlePipeResult::default()
        },
        Ok(Some(ControllerMessage::BootstrapState(snapshot))) => {
            state.apply_bootstrap_snapshot(snapshot);
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeBootstrap),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::ObservedIdentitiesRequest(pipe_id))) => {
            let output = serde_json::to_string(&state.view_model().observed_identities)
                .unwrap_or_else(|_| "[]".to_owned());
            HandlePipeResult {
                cli_pipe_output: Some(CliPipeOutput { pipe_id, output }),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::StatsCollect(request))) => HandlePipeResult {
            stats_collect_request: Some(request),
            ..HandlePipeResult::default()
        },
        Ok(Some(ControllerMessage::RailSizeObserved(observed))) => HandlePipeResult {
            rail_size_observed: Some(observed),
            ..HandlePipeResult::default()
        },
        Ok(None) => HandlePipeResult::default(),
        Err(error) => {
            eprintln!("tabs-controller: {error}");
            HandlePipeResult::default()
        }
    }
}

fn parse_rail_config(configuration: &BTreeMap<String, String>) -> RailConfig {
    RailConfig {
        structure: configuration
            .get("rail_structure")
            .and_then(|value| parse_rail_structure(value))
            .unwrap_or_default(),
        sizing: configuration
            .get("rail_sizing")
            .and_then(|value| parse_rail_sizing(value))
            .unwrap_or_default(),
        grouping: configuration
            .get("rail_grouping")
            .and_then(|value| parse_rail_grouping(value))
            .unwrap_or_default(),
        view: configuration
            .get("rail_view")
            .and_then(|value| parse_rail_view(value))
            .unwrap_or_default(),
    }
}

fn template_config_path_from_configuration(
    configuration: &BTreeMap<String, String>,
) -> Option<String> {
    path_from_configuration(configuration, "template_config_path")
}

fn grouping_config_path_from_configuration(
    configuration: &BTreeMap<String, String>,
) -> Option<String> {
    path_from_configuration(configuration, "grouping_config_path")
}

fn path_from_configuration(configuration: &BTreeMap<String, String>, key: &str) -> Option<String> {
    configuration
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(zellij_tile::vfs::expand_env)
}

fn initial_template_config_diagnostics(path: Option<String>) -> TemplateConfigDiagnostics {
    TemplateConfigDiagnostics {
        state: if path.is_some() {
            TemplateConfigState::PendingPermission
        } else {
            TemplateConfigState::NotConfigured
        },
        path,
        template_count: 0,
        template_names: vec![],
        last_error: None,
    }
}

fn template_config_error_diagnostics(
    path: Option<String>,
    error: String,
) -> TemplateConfigDiagnostics {
    TemplateConfigDiagnostics {
        path,
        state: TemplateConfigState::Error,
        template_count: 0,
        template_names: vec![],
        last_error: Some(error),
    }
}

fn should_retry_template_load(attempts: u8, loaded: bool) -> bool {
    !loaded && attempts < MAX_TEMPLATE_RELOAD_ATTEMPTS
}

fn template_config_state_text(state: TemplateConfigState) -> &'static str {
    match state {
        TemplateConfigState::NotConfigured => "not configured",
        TemplateConfigState::PendingPermission => "pending permission",
        TemplateConfigState::Loaded => "loaded",
        TemplateConfigState::Error => "error",
    }
}

fn render_plain_lines(lines: &[String], cols: usize) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let mut line = line.chars().take(cols).collect::<String>();
            let width = line.chars().count();
            if width < cols {
                line.push_str(&" ".repeat(cols - width));
            }
            line
        })
        .collect()
}

fn parse_rail_structure(value: &str) -> Option<RailStructure> {
    match value {
        "joined-cells" | "joined_cells" | "joined" => Some(RailStructure::JoinedCells),
        "split-around-active" | "split_around_active" | "split" => {
            Some(RailStructure::SplitAroundActive)
        }
        "box-per-tab" | "box_per_tab" | "boxes" => Some(RailStructure::BoxPerTab),
        _ => None,
    }
}

fn parse_rail_sizing(value: &str) -> Option<RailSizingPreset> {
    match value {
        "compact" => Some(RailSizingPreset::Compact),
        "large" => Some(RailSizingPreset::Large),
        "active-large" | "active_large" => Some(RailSizingPreset::ActiveLarge),
        "pinned-large" | "pinned_large" => Some(RailSizingPreset::PinnedLarge),
        _ => None,
    }
}

fn parse_rail_grouping(value: &str) -> Option<RailGroupingMode> {
    match value {
        "none" | "off" | "false" => Some(RailGroupingMode::None),
        "directory" | "cwd" | "pane-cwd" | "pane_cwd" => Some(RailGroupingMode::Directory),
        _ => None,
    }
}

fn parse_rail_view(value: &str) -> Option<RailViewMode> {
    match value {
        "normal" | "default" => Some(RailViewMode::Normal),
        "metadata" | "debug" | "inspect" => Some(RailViewMode::Metadata),
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
enum ControllerMessage {
    External(ExternalMessage),
    RendererHello(RendererHello),
    ConfigEditorHello(RendererHello),
    TogglePin(u64),
    SetSortMode(SortMode),
    SetRailConfig(RailConfig),
    RequestState,
    BootstrapRequest(RendererHello),
    BootstrapState(ControllerBootstrapSnapshot),
    ObservedIdentitiesRequest(String),
    StatsCollect(StatsCollectRequest),
    RailSizeObserved(RailSizeObserved),
}

fn parse_controller_message(
    pipe_message: &PipeMessage,
) -> Result<Option<ControllerMessage>, String> {
    match pipe_message.name.as_str() {
        MSG_SET_PANE_STATUS | MSG_CLEAR_PANE_STATUS | MSG_APPLY_METADATA_PATCH => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| format!("{} requires a JSON payload", pipe_message.name))?;
            serde_json::from_str::<ExternalMessage>(payload)
                .map(ControllerMessage::External)
                .map(Some)
                .map_err(|e| format!("failed to parse external message: {e}"))
        }
        MSG_RENDERER_HELLO => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "renderer hello requires a JSON payload".to_owned())?;
            serde_json::from_str::<RendererHello>(payload)
                .map(ControllerMessage::RendererHello)
                .map(Some)
                .map_err(|e| format!("failed to parse renderer hello: {e}"))
        }
        MSG_CONFIG_EDITOR_HELLO => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "config editor hello requires a JSON payload".to_owned())?;
            serde_json::from_str::<RendererHello>(payload)
                .map(ControllerMessage::ConfigEditorHello)
                .map(Some)
                .map_err(|e| format!("failed to parse config editor hello: {e}"))
        }
        MSG_TOGGLE_PIN => pipe_message
            .args
            .get("tab_id")
            .ok_or_else(|| "toggle-pin requires tab_id arg".to_owned())
            .and_then(|tab_id| {
                tab_id
                    .parse::<u64>()
                    .map_err(|e| format!("invalid tab_id: {e}"))
            })
            .map(ControllerMessage::TogglePin)
            .map(Some),
        MSG_SET_SORT_MODE => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "set-sort-mode requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<SortMode>(payload)
                    .map_err(|e| format!("invalid sort mode: {e}"))
            })
            .map(ControllerMessage::SetSortMode)
            .map(Some),
        MSG_SET_RAIL_CONFIG => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "set-rail-config requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<RailConfig>(payload)
                    .map_err(|e| format!("invalid rail config: {e}"))
            })
            .map(ControllerMessage::SetRailConfig)
            .map(Some),
        MSG_REQUEST_STATE => Ok(Some(ControllerMessage::RequestState)),
        MSG_OBSERVED_IDENTITIES => match &pipe_message.source {
            PipeSource::Cli(pipe_id) => Ok(Some(ControllerMessage::ObservedIdentitiesRequest(
                pipe_id.clone(),
            ))),
            _ => Err("observed identities request requires a CLI pipe source".to_owned()),
        },
        MSG_STATS_COLLECT => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "stats collect requires payload".to_owned())?;
            serde_json::from_str::<StatsCollectRequest>(payload)
                .map(ControllerMessage::StatsCollect)
                .map(Some)
                .map_err(|e| format!("failed to parse stats collect request: {e}"))
        }
        MSG_RAIL_SIZE_OBSERVED => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "rail size observed requires payload".to_owned())?;
            serde_json::from_str::<RailSizeObserved>(payload)
                .map(ControllerMessage::RailSizeObserved)
                .map(Some)
                .map_err(|e| format!("failed to parse rail size observation: {e}"))
        }
        MSG_CONTROLLER_BOOTSTRAP_REQUEST => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "bootstrap request requires payload".to_owned())?;
            serde_json::from_str::<RendererHello>(payload)
                .map(ControllerMessage::BootstrapRequest)
                .map(Some)
                .map_err(|e| format!("failed to parse bootstrap request: {e}"))
        }
        MSG_CONTROLLER_BOOTSTRAP_STATE => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "bootstrap state requires payload".to_owned())?;
            serde_json::from_str::<ControllerBootstrapSnapshot>(payload)
                .map(ControllerMessage::BootstrapState)
                .map(Some)
                .map_err(|e| format!("failed to parse bootstrap state: {e}"))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tabs_shared::{PaneTarget, Priority, SetPaneStatus};

    fn pipe(name: &str, payload: Option<String>, args: BTreeMap<String, String>) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Keybind,
            name: name.to_owned(),
            payload,
            args,
            is_private: false,
        }
    }

    fn cli_pipe(name: &str, payload: Option<String>, pipe_id: &str) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Cli(pipe_id.to_owned()),
            name: name.to_owned(),
            payload,
            args: BTreeMap::new(),
            is_private: false,
        }
    }

    #[test]
    fn parses_set_pane_status() {
        let payload = serde_json::to_string(&ExternalMessage::SetPaneStatus(SetPaneStatus {
            pane_id: PaneTarget::Terminal(1),
            priority: Priority::Waiting,
            title: "waiting".to_owned(),
            detail: None,
            icon: None,
            timestamp_ms: None,
        }))
        .unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_SET_PANE_STATUS, Some(payload), BTreeMap::new()))
                .unwrap();

        assert!(matches!(
            parsed,
            Some(ControllerMessage::External(ExternalMessage::SetPaneStatus(
                _
            )))
        ));
    }

    #[test]
    fn parses_clear_pane_status() {
        let payload = serde_json::to_string(&ExternalMessage::ClearPaneStatus {
            pane_id: PaneTarget::Terminal(1),
        })
        .unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_CLEAR_PANE_STATUS, Some(payload), BTreeMap::new()))
                .unwrap();

        assert_eq!(
            parsed,
            Some(ControllerMessage::External(
                ExternalMessage::ClearPaneStatus {
                    pane_id: PaneTarget::Terminal(1)
                }
            ))
        );
    }

    #[test]
    fn parses_renderer_hello() {
        let payload = serde_json::to_string(&RendererHello {
            plugin_id: 7,
            client_id: 1,
        })
        .unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_RENDERER_HELLO, Some(payload), BTreeMap::new()))
                .unwrap();

        assert_eq!(
            parsed,
            Some(ControllerMessage::RendererHello(RendererHello {
                plugin_id: 7,
                client_id: 1
            }))
        );
    }

    #[test]
    fn parses_config_editor_hello() {
        let payload = serde_json::to_string(&RendererHello {
            plugin_id: 11,
            client_id: 1,
        })
        .unwrap();

        let parsed = parse_controller_message(&pipe(
            MSG_CONFIG_EDITOR_HELLO,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(
            parsed,
            Some(ControllerMessage::ConfigEditorHello(RendererHello {
                plugin_id: 11,
                client_id: 1
            }))
        );
    }

    #[test]
    fn parses_toggle_pin_arg() {
        let mut args = BTreeMap::new();
        args.insert("tab_id".to_owned(), "42".to_owned());

        let parsed = parse_controller_message(&pipe(MSG_TOGGLE_PIN, None, args)).unwrap();

        assert_eq!(parsed, Some(ControllerMessage::TogglePin(42)));
    }

    #[test]
    fn parses_rail_config_from_plugin_configuration() {
        let mut configuration = BTreeMap::new();
        configuration.insert("rail_structure".to_owned(), "box-per-tab".to_owned());
        configuration.insert("rail_sizing".to_owned(), "compact".to_owned());

        let config = parse_rail_config(&configuration);

        assert_eq!(config.structure, RailStructure::BoxPerTab);
        assert_eq!(config.sizing, RailSizingPreset::Compact);
    }

    #[test]
    fn parses_directory_grouping_from_plugin_configuration() {
        let mut configuration = BTreeMap::new();
        configuration.insert("rail_grouping".to_owned(), "directory".to_owned());

        let config = parse_rail_config(&configuration);

        assert_eq!(config.grouping, RailGroupingMode::Directory);
    }

    #[test]
    fn parses_metadata_view_from_plugin_configuration() {
        let mut configuration = BTreeMap::new();
        configuration.insert("rail_view".to_owned(), "metadata".to_owned());

        let config = parse_rail_config(&configuration);

        assert_eq!(config.view, RailViewMode::Metadata);
    }

    #[test]
    fn parses_template_config_path_from_plugin_configuration() {
        let mut configuration = BTreeMap::new();
        configuration.insert(
            "template_config_path".to_owned(),
            " /host/tmp/andamento.kdl ".to_owned(),
        );

        let path = template_config_path_from_configuration(&configuration);

        assert_eq!(path.as_deref(), Some("/host/tmp/andamento.kdl"));
    }

    #[test]
    fn parses_grouping_config_path_from_plugin_configuration() {
        let mut configuration = BTreeMap::new();
        configuration.insert(
            "grouping_config_path".to_owned(),
            " /host/tmp/andamento-groups.kdl ".to_owned(),
        );

        let path = grouping_config_path_from_configuration(&configuration);

        assert_eq!(path.as_deref(), Some("/host/tmp/andamento-groups.kdl"));
    }

    #[test]
    fn parses_set_rail_config_payload() {
        let config = RailConfig {
            structure: RailStructure::SplitAroundActive,
            sizing: RailSizingPreset::PinnedLarge,
            grouping: RailGroupingMode::None,
            view: RailViewMode::Normal,
        };
        let payload = serde_json::to_string(&config).unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_SET_RAIL_CONFIG, Some(payload), BTreeMap::new()))
                .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::SetRailConfig(config)));
    }

    #[test]
    fn parses_controller_bootstrap_request() {
        let requester = RendererHello {
            plugin_id: 21,
            client_id: 2,
        };
        let payload = serde_json::to_string(&requester).unwrap();

        let parsed = parse_controller_message(&pipe(
            MSG_CONTROLLER_BOOTSTRAP_REQUEST,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::BootstrapRequest(requester)));
    }

    #[test]
    fn parses_controller_bootstrap_state() {
        let snapshot = ControllerBootstrapSnapshot {
            sort_mode: SortMode::PinnedFirst,
            config: RailConfig {
                structure: RailStructure::BoxPerTab,
                sizing: RailSizingPreset::Compact,
                grouping: RailGroupingMode::Directory,
                view: RailViewMode::Metadata,
            },
            pinned_tabs: vec![7],
            pane_statuses: vec![SetPaneStatus {
                pane_id: PaneTarget::Terminal(1),
                priority: Priority::Waiting,
                title: "waiting".to_owned(),
                detail: None,
                icon: None,
                timestamp_ms: Some(10),
            }],
            metadata_patches: vec![],
        };
        let payload = serde_json::to_string(&snapshot).unwrap();

        let parsed = parse_controller_message(&pipe(
            MSG_CONTROLLER_BOOTSTRAP_STATE,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::BootstrapState(snapshot)));
    }

    #[test]
    fn parses_observed_identities_cli_request() {
        let parsed =
            parse_controller_message(&cli_pipe(MSG_OBSERVED_IDENTITIES, None, "pipe-1")).unwrap();

        assert_eq!(
            parsed,
            Some(ControllerMessage::ObservedIdentitiesRequest(
                "pipe-1".to_owned()
            ))
        );
    }

    #[test]
    fn parses_stats_collect_request() {
        let request = StatsCollectRequest {
            requester: RendererHello {
                plugin_id: 22,
                client_id: 3,
            },
            collection_id: 44,
        };
        let payload = serde_json::to_string(&request).unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_STATS_COLLECT, Some(payload), BTreeMap::new()))
                .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::StatsCollect(request)));
    }

    #[test]
    fn parses_rail_size_observed() {
        let observed = RailSizeObserved {
            rail: RendererHello {
                plugin_id: 33,
                client_id: 4,
            },
            size: RailSize::Percent(18.75),
        };
        let payload = serde_json::to_string(&observed).unwrap();

        let parsed = parse_controller_message(&pipe(
            MSG_RAIL_SIZE_OBSERVED,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::RailSizeObserved(observed)));
    }

    #[test]
    fn set_rail_config_message_updates_controller_state() {
        let config = RailConfig {
            structure: RailStructure::BoxPerTab,
            sizing: RailSizingPreset::Compact,
            grouping: RailGroupingMode::None,
            view: RailViewMode::Normal,
        };
        let payload = serde_json::to_string(&config).unwrap();
        let mut state = ControllerState::default();

        let changed = handle_pipe_message(
            &mut state,
            pipe(MSG_SET_RAIL_CONFIG, Some(payload), BTreeMap::new()),
        );

        assert!(changed.state_changed);
        assert_eq!(
            changed.view_model_push_reason,
            Some(ViewModelPushReason::PipeConfig)
        );
        assert_eq!(state.view_model().config, config);
    }

    #[test]
    fn metadata_patch_message_updates_resolved_tab_metadata() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![state::ControllerTab {
            tab_id: 1,
            position: 0,
            name: "main".to_owned(),
            active: true,
        }]);
        let patch = tabs_shared::MetadataPatch {
            target: tabs_shared::MetadataTarget::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                "tab.subject".to_owned(),
                tabs_shared::MetadataValueUpdate {
                    value: tabs_shared::MetadataValue::Text("checkout".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        };
        let payload = serde_json::to_string(&ExternalMessage::MetadataPatch(patch)).unwrap();

        let result = handle_pipe_message(
            &mut state,
            pipe(MSG_APPLY_METADATA_PATCH, Some(payload), BTreeMap::new()),
        );

        let model = state.view_model();
        let tab_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == tabs_shared::MetadataTarget::Tab(1))
            .expect("tab metadata");
        assert!(result.state_changed);
        assert_eq!(
            result.view_model_push_reason,
            Some(ViewModelPushReason::PipeMetadata)
        );
        assert_eq!(
            tab_metadata
                .values
                .get("tab.subject")
                .map(|entry| &entry.value),
            Some(&tabs_shared::MetadataValue::Text("checkout".to_owned()))
        );
        assert_eq!(
            tab_metadata
                .source_entries
                .get("tab.subject")
                .and_then(|entries| entries.first())
                .map(|entry| entry.source_id.as_str()),
            Some("test")
        );
    }

    #[test]
    fn duplicate_metadata_patch_message_does_not_request_state_broadcast() {
        let mut state = ControllerState::default();
        let patch = tabs_shared::MetadataPatch {
            target: tabs_shared::MetadataTarget::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                tabs_shared::MetadataValueUpdate {
                    value: tabs_shared::MetadataValue::Text("zellij-org/zellij".to_owned()),
                    ttl_ms: Some(10_000),
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        };
        let payload = serde_json::to_string(&ExternalMessage::MetadataPatch(patch)).unwrap();

        let first = handle_pipe_message(
            &mut state,
            pipe(
                MSG_APPLY_METADATA_PATCH,
                Some(payload.clone()),
                BTreeMap::new(),
            ),
        );
        let second = handle_pipe_message(
            &mut state,
            pipe(MSG_APPLY_METADATA_PATCH, Some(payload), BTreeMap::new()),
        );

        assert!(first.state_changed);
        assert!(!second.state_changed);
    }

    #[test]
    fn observed_identities_cli_request_returns_json_output() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![state::ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".to_owned(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        let cwd = "/Users/robert/dev/katzensteg".to_owned();
        state.set_pane_cwd(PaneTarget::Terminal(1), cwd.clone());

        let result = handle_pipe_message(
            &mut state,
            cli_pipe(MSG_OBSERVED_IDENTITIES, None, "pipe-1"),
        );
        let output = result.cli_pipe_output.expect("cli pipe output");
        let observed: Vec<tabs_shared::ObservedMetadataIdentity> =
            serde_json::from_str(&output.output).unwrap();

        assert!(!result.state_changed);
        assert_eq!(output.pipe_id, "pipe-1");
        assert!(observed.iter().any(|identity| {
            identity.identity
                == tabs_shared::MetadataIdentity {
                    key: "zellij.pane.cwd".to_owned(),
                    value: tabs_shared::MetadataValue::Text(cwd.clone()),
                }
        }));
    }

    #[test]
    fn bootstrap_request_does_not_mutate_controller_state() {
        let requester = RendererHello {
            plugin_id: 21,
            client_id: 2,
        };
        let payload = serde_json::to_string(&requester).unwrap();
        let mut state = ControllerState::default();

        let result = handle_pipe_message(
            &mut state,
            pipe(
                MSG_CONTROLLER_BOOTSTRAP_REQUEST,
                Some(payload),
                BTreeMap::new(),
            ),
        );

        assert!(!result.state_changed);
        assert_eq!(result.bootstrap_request, Some(requester));
    }

    #[test]
    fn duplicate_renderer_hello_does_not_request_state_broadcast() {
        let mut state = ControllerState::default();
        let hello = RendererHello {
            plugin_id: 9,
            client_id: 1,
        };
        let payload = serde_json::to_string(&hello).unwrap();

        let first = handle_pipe_message(
            &mut state,
            pipe(MSG_RENDERER_HELLO, Some(payload.clone()), BTreeMap::new()),
        );
        let second = handle_pipe_message(
            &mut state,
            pipe(MSG_RENDERER_HELLO, Some(payload), BTreeMap::new()),
        );

        assert!(first.state_changed);
        assert!(!second.state_changed);
    }

    #[test]
    fn template_load_retries_until_loaded_or_attempt_limit() {
        assert!(should_retry_template_load(1, false));
        assert!(should_retry_template_load(
            MAX_TEMPLATE_RELOAD_ATTEMPTS - 1,
            false
        ));
        assert!(!should_retry_template_load(
            MAX_TEMPLATE_RELOAD_ATTEMPTS,
            false
        ));
        assert!(!should_retry_template_load(1, true));
    }

    #[test]
    fn pending_view_model_push_coalesces_unique_reasons() {
        let mut pending = PendingViewModelPush::default();

        assert!(pending.queue(ViewModelPushReason::UpdateTab));
        assert!(!pending.queue(ViewModelPushReason::UpdatePane));
        assert!(!pending.queue(ViewModelPushReason::UpdateTab));

        assert_eq!(
            pending.take(),
            vec![
                ViewModelPushReason::UpdateTab,
                ViewModelPushReason::UpdatePane
            ]
        );
        assert!(!pending.is_pending());
        assert!(pending.queue(ViewModelPushReason::UpdateCwd));
    }

    #[test]
    fn pending_rail_size_sync_debounces_per_client_targets() {
        let mut pending = PendingRailSizeSync::default();
        let rail = RendererHello {
            plugin_id: 9,
            client_id: 1,
        };

        assert!(pending.queue(RailSizeObserved {
            rail: rail.clone(),
            size: RailSize::Percent(20.0),
        }));
        assert!(!pending.queue(RailSizeObserved {
            rail: rail.clone(),
            size: RailSize::Percent(20.0005),
        }));

        let targets = pending.take_targets();
        assert_eq!(
            targets,
            vec![RailSizeTarget {
                client_id: 1,
                size: RailSize::Percent(20.0),
                version: 1,
            }]
        );
        assert!(!pending.is_pending());
        assert!(!pending.queue(RailSizeObserved {
            rail,
            size: RailSize::Percent(20.0005),
        }));
        assert!(pending.queue(RailSizeObserved {
            rail: RendererHello {
                plugin_id: 11,
                client_id: 1,
            },
            size: RailSize::Percent(25.0),
        }));
        assert!(!pending.queue(RailSizeObserved {
            rail: RendererHello {
                plugin_id: 12,
                client_id: 2,
            },
            size: RailSize::Fixed(30),
        }));

        assert_eq!(
            pending.take_targets(),
            vec![
                RailSizeTarget {
                    client_id: 1,
                    size: RailSize::Percent(25.0),
                    version: 2,
                },
                RailSizeTarget {
                    client_id: 2,
                    size: RailSize::Fixed(30),
                    version: 1,
                },
            ]
        );
    }
}
