mod metadata;
mod state;

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

/// Most recent debug-trace entries the controller will surface in its render
/// pane. Cheap stand-in for proper logging (zellij's wasm-stderr LoggingPipe
/// emits at debug level, which the server's default INFO filter drops).
const RECENT_PIPE_LOG_CAPACITY: usize = 20;

use andamento_shared::PaneTarget;
use andamento_shared::PluginStatsRecorder;
use andamento_shared::MSG_RAIL_SIZE_TARGET;
use andamento_shared::MSG_VIEW_MODEL;
use andamento_shared::{
    ConfigInspectRequest, ControllerBootstrapSnapshot, ExternalMessage, GroupingTemplateSetRequest,
    MetadataVisibilitySetRequest, NodeVariableSetRequest, PluginRegistrationHello, RailConfig,
    RailRgbColor, RailSize, RailSizeObserved, RailSizeTarget, RailStructure, RailUiAction,
    RailUiState, RendererHello, SortMode, StatsCollectRequest, MSG_APPLY_METADATA_PATCH,
    MSG_CLEAR_PANE_STATUS, MSG_CONFIG_EDITOR_HELLO, MSG_CONFIG_INSPECT,
    MSG_CONTROLLER_BOOTSTRAP_REQUEST, MSG_CONTROLLER_BOOTSTRAP_STATE, MSG_OBSERVED_IDENTITIES,
    MSG_RAIL_SIZE_OBSERVED, MSG_RAIL_UI_ACTION, MSG_RAIL_UI_STATE, MSG_RENDERER_HELLO,
    MSG_REQUEST_RAIL_UI_STATE, MSG_REQUEST_STATE, MSG_SET_GROUPING_TEMPLATE,
    MSG_SET_METADATA_VISIBILITY, MSG_SET_NODE_VARIABLE, MSG_SET_PANE_STATUS, MSG_SET_RAIL_CONFIG,
    MSG_SET_SORT_MODE, MSG_STATS_COLLECT, MSG_TOGGLE_PIN,
};
use andamento_shared::{TemplateConfigDiagnostics, TemplateConfigState};
use andamento_shared::{MSG_STATS_REPORT, MSG_STATS_REQUEST};
use state::{ControllerState, EntityActivation};
use zellij_tile::output::print;
use zellij_tile::prelude::*;

#[cfg(test)]
use andamento_shared::{NodeKey, PluginPlacement};

#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
const TEMPLATE_RELOAD_RETRY_SECS: f64 = 0.25;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
const RAIL_SIZE_SYNC_DEBOUNCE_SECS: f64 = 0.05;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
const VIEW_MODEL_PUSH_COALESCE_SECS: f64 = 0.01;
const MAX_TEMPLATE_RELOAD_ATTEMPTS: u8 = 20;
const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";
const CONFIG_CLOSE_ON_HIDDEN: &str = "close_on_hidden";
const CONFIG_ORIGIN_TAB_ID: &str = "origin_tab_id";
const CONFIG_PANE_KIND: &str = "pane_kind";
const CONFIG_RAIL_SCOPE: &str = "rail_scope";

fn command_for_materialize_recipe(
    recipe: &str,
    checkout_path: Option<&str>,
    home_dir: Option<&std::path::Path>,
) -> Option<CommandToRun> {
    let mut command = CommandToRun::new_with_args("/bin/sh", vec!["-c", recipe]);
    command.cwd = Some(
        checkout_path
            .map(PathBuf::from)
            .or_else(|| home_dir.map(std::path::Path::to_path_buf))?,
    );
    Some(command)
}

fn rail_ui_state_broadcast_message(state: &RailUiState) -> Option<MessageToPlugin> {
    Some(MessageToPlugin::new(MSG_RAIL_UI_STATE).with_payload(serde_json::to_string(state).ok()?))
}

/// Native-target harness: replay captured connector patches through the real
/// controller pipeline and print the derived rail — the presentation stack's
/// composition, testable in seconds without zellij (andamento#37 postmortem).
///
///   cargo run -p andamento-controller -- <patches.jsonl> [grouping.kdl]
///       [--dump-template <group-header|tab-title|tab-status|compact|detail>]
#[cfg(not(target_family = "wasm"))]
fn main() {
    let mut args = std::env::args().skip(1);
    let Some(patches_path) = args.next() else {
        std::eprintln!(
            "usage: render-harness <patches.jsonl> [grouping.kdl] \
             [--dump-template <slot>]"
        );
        std::process::exit(2);
    };
    let mut grouping_path = None;
    let mut dump_template = None;
    while let Some(argument) = args.next() {
        if argument == "--dump-template" {
            dump_template = Some(args.next().expect("--dump-template requires a slot"));
        } else if grouping_path.replace(argument).is_some() {
            panic!("only one grouping/template KDL path may be supplied");
        }
    }
    let grouping_kdl = match grouping_path {
        Some(path) => std::fs::read_to_string(&path).expect("read grouping kdl"),
        None => include_str!("../../../templates/flotilla-default.kdl").to_owned(),
    };

    let mut state = ControllerState::default();
    let templates = andamento_shared::template_config::parse_template_config_kdl(&grouping_kdl)
        .expect("parse template kdl");
    state.set_template_catalog(Some(
        andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(templates),
    ));
    let live = andamento_shared::grouping_config::parse_grouping_config_kdl(&grouping_kdl)
        .expect("parse grouping kdl");
    state.set_grouping_catalog(Some(
        andamento_shared::grouping_config::GroupingConfigCatalog::with_bundled_defaults(live),
    ));
    let raw = std::fs::read_to_string(&patches_path).expect("read patches file");
    let (mut applied, mut failed) = (0usize, 0usize);
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let message = PipeMessage {
            source: PipeSource::Cli("render-harness".to_owned()),
            name: MSG_APPLY_METADATA_PATCH.to_owned(),
            payload: Some(line.to_owned()),
            args: BTreeMap::new(),
            is_private: false,
        };
        let result = handle_pipe_message(&mut state, message);
        if result.state_changed {
            applied += 1;
        } else {
            failed += 1;
            std::eprintln!(
                "no-op patch: {}",
                line.chars().take(160).collect::<String>()
            );
        }
    }

    let model = state.view_model();
    std::println!(
        "=== rail ({} patches applied, {} no-op; {} tabs, {} rows) ===",
        applied,
        failed,
        model.tabs.len(),
        model.rows.len()
    );
    for row in &model.rows {
        match row {
            andamento_shared::RailRow::GroupHeader {
                label,
                full_label,
                tab_count,
                ..
            } => {
                std::println!("[group] {label}  ({full_label}, tabs={tab_count})");
            }
            andamento_shared::RailRow::Tab { tab_id, indent, .. } => {
                std::println!("{}tab #{tab_id}", "  ".repeat(*indent));
            }
            andamento_shared::RailRow::Latent { latent, indent, .. } => {
                std::println!("{}(latent) {:?}", "  ".repeat(*indent), latent);
            }
            andamento_shared::RailRow::Entity { entity, indent, .. } => {
                std::println!(
                    "{}(entity:{}:{:?}) {}",
                    "  ".repeat(*indent),
                    entity.entity.id,
                    entity.form,
                    entity.label
                );
            }
        }
    }

    render_rail_lines(&model, &grouping_kdl);
    if let Some(slot) = dump_template.as_deref() {
        dump_effective_templates(&model, slot);
    }
}

/// Render the derived rows through the real rail renderer — actual template
/// selection, actual field composition, actual truncation — so template-level
/// questions are answerable here instead of only in a running zellij.
#[cfg(not(target_family = "wasm"))]
fn render_rail_lines(model: &andamento_shared::ControllerViewModel, config_kdl: &str) {
    use andamento_shared::template_config::TemplateConfigCatalog;
    let templates = match andamento_shared::template_config::parse_template_config_kdl(config_kdl) {
        Ok(config) => Some(TemplateConfigCatalog::with_bundled_defaults(config)),
        Err(error) => {
            std::eprintln!(
                "template config parse failed (rendering with bundled templates only): {error:?}"
            );
            None
        }
    };
    let cols = std::env::var("HARNESS_COLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(46usize);
    let rows = model.rows.len() * 4 + 8;
    let rendered = andamento_rail::render::render_lines_with_template_catalog(
        Some(model),
        &[],
        rows,
        cols,
        true,
        templates.as_ref(),
    );
    std::println!(
        "=== rendered ({} lines @ {cols} cols) ===",
        rendered.lines.len()
    );
    for line in &rendered.lines {
        std::println!("{line}");
    }
}

#[cfg(not(target_family = "wasm"))]
fn dump_effective_templates(model: &andamento_shared::ControllerViewModel, requested_slot: &str) {
    use andamento_shared::{RailRow, ResolvedTemplateSlot};

    fn print_dump(label: &str, slot: Option<&ResolvedTemplateSlot>) -> bool {
        let Some(slot) = slot else {
            return false;
        };
        std::println!("=== effective template: {label} ===");
        if slot.effective_kdl.is_empty() {
            std::println!("// resolved template: {}", slot.template_name);
        } else {
            std::print!("{}", slot.effective_kdl);
        }
        true
    }

    let mut dumped = false;
    match requested_slot {
        "group-header" => {
            for row in &model.rows {
                if let RailRow::GroupHeader {
                    full_label,
                    templates,
                    ..
                } = row
                {
                    dumped |= print_dump(full_label, templates.group_header.as_ref());
                }
            }
        }
        "compact" | "detail" => {
            for row in &model.rows {
                if let RailRow::Entity { entity, .. } = row {
                    let slot = if requested_slot == "compact" {
                        entity.templates.compact.as_ref()
                    } else {
                        entity.templates.detail.as_ref()
                    };
                    dumped |= print_dump(&entity.label, slot);
                }
            }
        }
        "tab-title" | "tab-status" => {
            for tab in &model.tabs {
                let slot = if requested_slot == "tab-title" {
                    tab.templates.tab_title.as_ref()
                } else {
                    tab.templates.tab_status.as_ref()
                };
                dumped |= print_dump(&tab.name, slot);
            }
        }
        other => {
            std::eprintln!("unknown template slot: {other}");
            return;
        }
    }
    if !dumped {
        std::eprintln!("no resolved {requested_slot} templates in the captured model");
    }
}

#[cfg(target_family = "wasm")]
fn render_rail_lines(_model: &andamento_shared::ControllerViewModel, _config_kdl: &str) {}

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
    recent_pipe_log: VecDeque<String>,
}

#[cfg(any(target_family = "wasm", feature = "native-plugin-factory"))]
register_plugin!(PluginState);

impl ZellijPlugin for PluginState {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_identity = Some(RendererHello {
            plugin_id: ids.plugin_id,
            client_id: ids.client_id,
        });
        self.state.set_rail_ui_writer_client_id(ids.client_id);
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
            PermissionType::RunCommands,
            PermissionType::FullHdAccess,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::PaneClosed,
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
                self.broadcast_rail_ui_state();
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
                // Renderer eviction is driven by Event::PaneClosed, not by
                // the live-plugin set in this manifest — the manifest can
                // arrive between a new rail's HELLO and its pane appearing,
                // racing the new rail out of known_rails.
                let mut state_changed = self.state.update_panes_from_manifest(pane_manifest);
                for terminal_id in self.state.terminal_panes_for_cwd_refresh() {
                    // Mark before the call: one attempt per pane lifetime. On
                    // failure (e.g. host-side timeout) we rely on
                    // Event::CwdChanged rather than retrying every PaneUpdate.
                    self.state
                        .mark_pane_cwd_requested(PaneTarget::Terminal(terminal_id));
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
            Event::PaneClosed(pane_id) => {
                self.stats.increment("update.pane-closed");
                if let PaneId::Plugin(plugin_id) = pane_id {
                    if self.state.unregister_renderer(plugin_id) {
                        self.stats.increment("renderer.unregistered");
                    }
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
        let payload_preview = pipe_message
            .payload
            .as_deref()
            .map(|p| {
                let truncated = if p.len() > 200 { &p[..200] } else { p };
                format!("{}...({} bytes)", truncated, p.len())
            })
            .unwrap_or_else(|| "<no payload>".to_owned());
        let entry = format!(
            "pipe name={:?} src={:?} payload={}",
            pipe_message.name, pipe_message.source, payload_preview,
        );
        if self.recent_pipe_log.len() >= RECENT_PIPE_LOG_CAPACITY {
            self.recent_pipe_log.pop_front();
        }
        self.recent_pipe_log.push_back(entry);
        let started_at = Instant::now();
        let mut result = handle_pipe_message(&mut self.state, pipe_message);
        self.stats
            .record_span_elapsed("pipe.handle-message", started_at);
        if let Some(entity) = result.activate_entity_request.take() {
            let state_changed = self.activate_entity(entity);
            result.state_changed |= state_changed;
            if state_changed {
                result.view_model_push_reason = Some(ViewModelPushReason::PipeMaterializeLatent);
            }
        }
        if let Some(request) = result.materialize_latent_request.take() {
            let state_changed = self.materialize_latent(request);
            result.state_changed |= state_changed;
            if state_changed {
                result.view_model_push_reason = Some(ViewModelPushReason::PipeMaterializeLatent);
            }
        }
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
        if let Some(request) = result.config_inspect_request.as_ref() {
            self.open_or_focus_config_editor(request);
        }
        if let Some(output) = result.cli_pipe_output.as_ref() {
            cli_pipe_output(&output.pipe_id, &output.output);
        }
        if let Some(pipe_id) = result.cli_pipe_unblock.as_ref() {
            unblock_cli_pipe_input(pipe_id);
        }
        if result.broadcast_rail_ui_state {
            self.broadcast_rail_ui_state();
        }
        if result.state_changed {
            let reason = result
                .view_model_push_reason
                .unwrap_or(ViewModelPushReason::PipeUnknown);
            match pipe_view_model_delivery(reason) {
                PipeViewModelDelivery::Immediate => {
                    self.push_view_model_to_rails_with_pending(reason);
                }
                PipeViewModelDelivery::Coalesced => {
                    self.queue_view_model_push(reason);
                }
            }
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
        for warning in &diagnostics.warnings {
            lines.push(format!("template warning: {warning}"));
        }
        if !self.recent_pipe_log.is_empty() {
            lines.push(String::new());
            lines.push(format!("recent pipes ({}):", self.recent_pipe_log.len()));
            for entry in self.recent_pipe_log.iter().rev() {
                lines.push(format!("  {entry}"));
            }
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
            self.state
                .set_template_config_diagnostics(TemplateConfigDiagnostics::default());
            self.state.set_template_catalog(None);
            return true;
        };
        match andamento_shared::template_config::load_template_catalog_from_file(path) {
            Ok(catalog) => {
                let diagnostics = TemplateConfigDiagnostics {
                    path: Some(path.to_owned()),
                    state: TemplateConfigState::Loaded,
                    template_count: catalog.len(),
                    template_names: catalog.template_names(),
                    last_error: None,
                    warnings: vec![],
                    effective_variables: vec![],
                };
                self.state.set_template_config_diagnostics(diagnostics);
                self.state.set_template_catalog(Some(catalog));
                true
            }
            Err(error) => {
                eprintln!("andamento-controller: failed to load template config: {error}");
                self.state
                    .set_template_config_diagnostics(template_config_error_diagnostics(
                        Some(path.to_owned()),
                        error.to_string(),
                    ));
                self.state.set_template_catalog(None);
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
        match andamento_shared::grouping_config::load_grouping_catalog_from_file(path) {
            Ok(catalog) => {
                self.grouping_rule_count = catalog.rules.len();
                self.grouping_config_error = None;
                self.state.set_grouping_catalog(Some(catalog));
                true
            }
            Err(error) => {
                eprintln!("andamento-controller: failed to load grouping config: {error}");
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
        for (client_id, client_targets) in group_view_model_targets(targets) {
            let started_at = Instant::now();
            let Ok(payload) = serde_json::to_string(&self.state.view_model_for_client(client_id))
            else {
                continue;
            };
            self.stats
                .record_span_elapsed("json.encode-view-model", started_at);
            self.stats.add(
                "view-model.payload-bytes",
                (payload.len() as u64).saturating_mul(client_targets.len() as u64),
            );
            for target in client_targets {
                pipe_message_to_plugin(
                    MessageToPlugin::new(MSG_VIEW_MODEL)
                        .with_destination_plugin_id(target.plugin_id)
                        .with_destination_client_id(target.client_id)
                        .with_payload(payload.clone()),
                );
            }
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

    fn broadcast_rail_ui_state(&self) {
        if !self.permissions_granted {
            return;
        }
        let Some(message) = rail_ui_state_broadcast_message(&self.state.rail_ui_state()) else {
            return;
        };
        pipe_message_to_plugin(message);
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

    fn open_or_focus_config_editor(&self, request: &ConfigInspectRequest) {
        let message = self
            .state
            .config_editor_target_for_client_tab(request.client_id, request.origin_tab_id)
            .and_then(|target| config_inspect_message_for_existing_editor(request, target))
            .or_else(|| config_inspect_message_for_new_editor(request));
        let Some(message) = message else {
            return;
        };
        pipe_message_to_plugin(message);
    }

    fn materialize_latent(&mut self, request: andamento_shared::MaterializeLatentRequest) -> bool {
        self.stats.increment("latent.materialize.request");
        if let Some(tab_position) = self.state.materialized_tab_position(&request) {
            self.stats.increment("latent.materialize.focus-existing");
            switch_tab_to((tab_position + 1) as u32);
            return false;
        }
        if !self.state.begin_latent_materialization(&request) {
            self.stats.increment("latent.materialize.rejected-stale");
            return false;
        }
        let home_dir = request
            .checkout_path
            .is_none()
            .then(|| {
                get_session_environment_variables()
                    .get("HOME")
                    .map(PathBuf::from)
            })
            .flatten();
        let Some(command) = command_for_materialize_recipe(
            &request.recipe,
            request.checkout_path.as_deref(),
            home_dir.as_deref(),
        ) else {
            self.stats.increment("latent.materialize.missing-home");
            return self.state.abort_latent_materialization(&request);
        };
        // Publish the opener-owned identity as pending before entering Zellij's
        // create call. Duplicate gestures now observe a non-openable latent.
        self.push_view_model_to_rails_with_pending(ViewModelPushReason::PipeMaterializeLatent);
        let (Some(tab_id), _) = open_command_pane_in_new_tab(command, BTreeMap::new()) else {
            self.stats.increment("latent.materialize.open-failed");
            return self.state.abort_latent_materialization(&request);
        };
        if !self.state.bind_materializing_tab(&request, tab_id as u64) {
            self.stats.increment("latent.materialize.bind-failed");
            return self.state.abort_latent_materialization(&request);
        }
        // Residual approximation race: if the recipe dies before Zellij
        // reports this tab, the latent can remain opening rather than be
        // claimed. Atomic creation context is tracked for a later substrate
        // upgrade at https://forgejo.lab.flotilla.work/fork-issues/zellij/issues/9.
        rename_tab_with_id(tab_id as u64, &request.name);
        self.stats.increment("latent.materialize.awaiting-claim");
        // The first TabUpdate containing this ID claims it and publishes the
        // opening-to-live transition; there is no intermediate orphan model.
        false
    }

    fn activate_entity(&mut self, entity: andamento_shared::EntityRef) -> bool {
        self.stats.increment("entity.activate.request");
        match self.state.activation_for_entity(&entity) {
            Some(EntityActivation::FocusTab { position }) => {
                self.stats.increment("entity.activate.focus-existing");
                switch_tab_to((position + 1) as u32);
                false
            }
            Some(EntityActivation::Materialize(request)) => {
                self.stats.increment("entity.activate.materialize");
                self.materialize_latent(request)
            }
            None => {
                self.stats.increment("entity.activate.unavailable");
                false
            }
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct HandlePipeResult {
    state_changed: bool,
    view_model_push_reason: Option<ViewModelPushReason>,
    bootstrap_request: Option<RendererHello>,
    cli_pipe_output: Option<CliPipeOutput>,
    cli_pipe_unblock: Option<String>,
    stats_collect_request: Option<StatsCollectRequest>,
    rail_size_observed: Option<RailSizeObserved>,
    config_inspect_request: Option<ConfigInspectRequest>,
    materialize_latent_request: Option<andamento_shared::MaterializeLatentRequest>,
    activate_entity_request: Option<andamento_shared::EntityRef>,
    broadcast_rail_ui_state: bool,
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
    PipeMetadataControls,
    PipeDisplayVariable,
    PipeMaterializeLatent,
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
            Self::PipeMetadataControls => "view-model.push.reason.pipe.metadata-controls",
            Self::PipeDisplayVariable => "view-model.push.reason.pipe.display-variable",
            Self::PipeMaterializeLatent => "view-model.push.reason.pipe.materialize-latent",
            Self::PipeUnknown => "view-model.push.reason.pipe.unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipeViewModelDelivery {
    Immediate,
    Coalesced,
}

fn pipe_view_model_delivery(reason: ViewModelPushReason) -> PipeViewModelDelivery {
    match reason {
        ViewModelPushReason::PipeMetadata => PipeViewModelDelivery::Coalesced,
        _ => PipeViewModelDelivery::Immediate,
    }
}

fn group_view_model_targets(targets: Vec<RendererHello>) -> Vec<(u16, Vec<RendererHello>)> {
    let mut grouped = Vec::<(u16, Vec<RendererHello>)>::new();
    for target in targets {
        if let Some((_, client_targets)) = grouped
            .iter_mut()
            .find(|(client_id, _)| *client_id == target.client_id)
        {
            client_targets.push(target);
        } else {
            grouped.push((target.client_id, vec![target]));
        }
    }
    grouped
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
    let cli_pipe_unblock = match &pipe_message.source {
        PipeSource::Cli(pipe_id) => Some(pipe_id.clone()),
        PipeSource::Plugin(_) | PipeSource::Keybind => None,
    };
    let mut result = match parse_controller_message(&pipe_message) {
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
            // Fingerprint: hash the address of `state` so we can see if multiple controller
            // instances exist (e.g. per client_id from multi-client cloning).
            let inst = (state as *const _) as usize & 0xFFFFFF;
            log::info!(
                "andamento-controller[inst {inst:x}]: RendererHello from plugin_id={} client_id={} (known rails before: {})",
                hello.identity.plugin_id,
                hello.identity.client_id,
                state.known_rail_count()
            );
            let state_changed = state.register_rail(hello);
            log::info!(
                "andamento-controller[inst {inst:x}]: register_rail state_changed={state_changed} (known rails after: {})",
                state.known_rail_count()
            );
            HandlePipeResult {
                state_changed,
                broadcast_rail_ui_state: true,
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
        Ok(Some(ControllerMessage::SetGroupingTemplate(request))) => {
            let state_changed = state.set_active_grouping_template(request.name);
            HandlePipeResult {
                state_changed,
                view_model_push_reason: state_changed.then_some(ViewModelPushReason::PipeConfig),
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
                broadcast_rail_ui_state: true,
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
        Ok(Some(ControllerMessage::SetMetadataVisibility(request))) => {
            state.set_metadata_visibility_for_client(
                request.client_id,
                request.node_key,
                request.state,
            );
            HandlePipeResult {
                state_changed: true,
                view_model_push_reason: Some(ViewModelPushReason::PipeMetadataControls),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::RailUiAction(action))) => {
            let display_variable_action = matches!(action, RailUiAction::ToggleVariable { .. });
            let changed = state.apply_rail_ui_action(action);
            let display_variable_changed = display_variable_action && changed;
            HandlePipeResult {
                state_changed: display_variable_changed,
                broadcast_rail_ui_state: changed,
                view_model_push_reason: display_variable_changed
                    .then_some(ViewModelPushReason::PipeDisplayVariable),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::RailUiState(snapshot))) => {
            state.apply_rail_ui_state(snapshot);
            HandlePipeResult::default()
        }
        Ok(Some(ControllerMessage::RequestRailUiState)) => HandlePipeResult {
            broadcast_rail_ui_state: true,
            ..HandlePipeResult::default()
        },
        Ok(Some(ControllerMessage::SetNodeVariable(request))) => {
            let state_changed =
                state.set_node_variable(request.node_key, request.name, request.value);
            HandlePipeResult {
                state_changed,
                view_model_push_reason: state_changed
                    .then_some(ViewModelPushReason::PipeMetadataControls),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::ConfigInspect(request))) => {
            let state_changed =
                state.set_inspected_node(request.client_id, request.node_key.clone());
            HandlePipeResult {
                state_changed,
                view_model_push_reason: state_changed
                    .then_some(ViewModelPushReason::PipeMetadataControls),
                config_inspect_request: Some(request),
                ..HandlePipeResult::default()
            }
        }
        Ok(Some(ControllerMessage::MaterializeLatent(request))) => HandlePipeResult {
            materialize_latent_request: Some(request),
            ..HandlePipeResult::default()
        },
        Ok(Some(ControllerMessage::ActivateEntity(entity))) => HandlePipeResult {
            activate_entity_request: Some(entity),
            ..HandlePipeResult::default()
        },
        Ok(None) => HandlePipeResult::default(),
        Err(error) => {
            let message = format!(
                "andamento-controller: rejected pipe message {}: {error}",
                pipe_message.name
            );
            log::error!("{message}");
            eprintln!("{message}");
            HandlePipeResult::default()
        }
    };
    result.cli_pipe_unblock = cli_pipe_unblock;
    result
}

fn parse_rail_config(configuration: &BTreeMap<String, String>) -> RailConfig {
    RailConfig {
        structure: configuration
            .get("rail_structure")
            .and_then(|value| parse_rail_structure(value))
            .unwrap_or_default(),
        segment_between_color: configuration
            .get("rail_segment_between_color")
            .and_then(|value| parse_rail_rgb_color(value)),
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
        warnings: vec![],
        effective_variables: vec![],
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
        warnings: vec![],
        effective_variables: vec![],
    }
}

fn config_inspect_message_for_existing_editor(
    request: &ConfigInspectRequest,
    target: RendererHello,
) -> Option<MessageToPlugin> {
    let payload = serde_json::to_string(request).ok()?;
    Some(
        MessageToPlugin::new(MSG_CONFIG_INSPECT)
            .with_destination_plugin_id(target.plugin_id)
            .with_destination_client_id(target.client_id)
            .with_payload(payload),
    )
}

fn config_inspect_message_for_new_editor(
    request: &ConfigInspectRequest,
) -> Option<MessageToPlugin> {
    let payload = serde_json::to_string(request).ok()?;
    let mut configuration = BTreeMap::new();
    configuration.insert(
        CONFIG_CONTROLLER_PLUGIN_URL.to_owned(),
        request.controller_plugin_url.clone(),
    );
    configuration.insert(
        CONFIG_RAIL_SCOPE.to_owned(),
        inspect_scope_label(&request.node_key),
    );
    configuration.insert(CONFIG_CLOSE_ON_HIDDEN.to_owned(), "true".to_owned());
    configuration.insert(
        CONFIG_ORIGIN_TAB_ID.to_owned(),
        request.origin_tab_id.to_string(),
    );
    configuration.insert(CONFIG_PANE_KIND.to_owned(), "floating".to_owned());
    Some(
        MessageToPlugin::new(MSG_CONFIG_INSPECT)
            .with_plugin_url(request.config_plugin_url.clone())
            .with_destination_client_id(request.client_id)
            .with_plugin_config(configuration)
            .with_payload(payload)
            .new_plugin_instance_should_float(true)
            .new_plugin_instance_should_be_focused(),
    )
}

fn inspect_scope_label(key: &andamento_shared::NodeKey) -> String {
    match key {
        andamento_shared::NodeKey::Root => "root".to_owned(),
        andamento_shared::NodeKey::Tab(tab_id) => format!("tab:{tab_id}"),
        andamento_shared::NodeKey::Group(_) => "group".to_owned(),
        andamento_shared::NodeKey::Entity(entity) => {
            format!("entity:{}:{}", entity.kind, entity.id)
        }
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

fn parse_rail_rgb_color(value: &str) -> Option<RailRgbColor> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some(RailRgbColor {
        red: u8::from_str_radix(&hex[0..2], 16).ok()?,
        green: u8::from_str_radix(&hex[2..4], 16).ok()?,
        blue: u8::from_str_radix(&hex[4..6], 16).ok()?,
    })
}

#[derive(Debug, PartialEq)]
enum ControllerMessage {
    External(ExternalMessage),
    RendererHello(PluginRegistrationHello),
    ConfigEditorHello(PluginRegistrationHello),
    TogglePin(u64),
    SetSortMode(SortMode),
    SetRailConfig(RailConfig),
    SetGroupingTemplate(GroupingTemplateSetRequest),
    RequestState,
    BootstrapRequest(RendererHello),
    BootstrapState(ControllerBootstrapSnapshot),
    ObservedIdentitiesRequest(String),
    StatsCollect(StatsCollectRequest),
    RailSizeObserved(RailSizeObserved),
    SetMetadataVisibility(MetadataVisibilitySetRequest),
    RailUiAction(RailUiAction),
    RailUiState(RailUiState),
    RequestRailUiState,
    SetNodeVariable(NodeVariableSetRequest),
    ConfigInspect(ConfigInspectRequest),
    MaterializeLatent(andamento_shared::MaterializeLatentRequest),
    ActivateEntity(andamento_shared::EntityRef),
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
            serde_json::from_str::<PluginRegistrationHello>(payload)
                .map(ControllerMessage::RendererHello)
                .map(Some)
                .map_err(|e| format!("failed to parse renderer hello: {e}"))
        }
        MSG_CONFIG_EDITOR_HELLO => {
            let payload = pipe_message
                .payload
                .as_deref()
                .ok_or_else(|| "config editor hello requires a JSON payload".to_owned())?;
            serde_json::from_str::<PluginRegistrationHello>(payload)
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
        MSG_SET_GROUPING_TEMPLATE => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "set-grouping-template requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<GroupingTemplateSetRequest>(payload)
                    .map_err(|e| format!("invalid grouping template request: {e}"))
            })
            .map(ControllerMessage::SetGroupingTemplate)
            .map(Some),
        MSG_SET_METADATA_VISIBILITY => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "set metadata visibility requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<MetadataVisibilitySetRequest>(payload)
                    .map_err(|e| format!("invalid metadata visibility request: {e}"))
            })
            .map(ControllerMessage::SetMetadataVisibility)
            .map(Some),
        MSG_RAIL_UI_ACTION => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "rail UI action requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<RailUiAction>(payload)
                    .map_err(|e| format!("invalid rail UI action: {e}"))
            })
            .map(ControllerMessage::RailUiAction)
            .map(Some),
        MSG_RAIL_UI_STATE => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "rail UI state requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<RailUiState>(payload)
                    .map_err(|e| format!("invalid rail UI state: {e}"))
            })
            .map(ControllerMessage::RailUiState)
            .map(Some),
        MSG_REQUEST_RAIL_UI_STATE => Ok(Some(ControllerMessage::RequestRailUiState)),
        MSG_SET_NODE_VARIABLE => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "set node variable requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<NodeVariableSetRequest>(payload)
                    .map_err(|e| format!("invalid node variable request: {e}"))
            })
            .map(ControllerMessage::SetNodeVariable)
            .map(Some),
        MSG_CONFIG_INSPECT => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "config inspect requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<ConfigInspectRequest>(payload)
                    .map_err(|e| format!("invalid config inspect request: {e}"))
            })
            .map(ControllerMessage::ConfigInspect)
            .map(Some),
        andamento_shared::MSG_MATERIALIZE_LATENT => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "materialize latent requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<andamento_shared::MaterializeLatentRequest>(payload)
                    .map_err(|e| format!("invalid materialize latent request: {e}"))
            })
            .map(ControllerMessage::MaterializeLatent)
            .map(Some),
        andamento_shared::MSG_ACTIVATE_ENTITY => pipe_message
            .payload
            .as_deref()
            .ok_or_else(|| "activate entity requires payload".to_owned())
            .and_then(|payload| {
                serde_json::from_str::<andamento_shared::EntityRef>(payload)
                    .map_err(|e| format!("invalid activate entity request: {e}"))
            })
            .map(ControllerMessage::ActivateEntity)
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
    use andamento_shared::{PaneTarget, Priority, RailUiRevision, SetPaneStatus};

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

    #[cfg(not(target_family = "wasm"))]
    fn rail_frame_snapshot(lines: &[String], cols: usize) -> String {
        let mut snapshot = format!("frame {cols}x{}", lines.len());
        for (index, line) in lines.iter().enumerate() {
            let mut plain = String::new();
            let mut chars = line.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\u{1b}' && chars.peek() == Some(&'[') {
                    let _ = chars.next();
                    for code_ch in chars.by_ref() {
                        if code_ch.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else {
                    plain.push(ch);
                }
            }
            let content = plain.trim_end();
            let trailing = cols.saturating_sub(content.chars().count());
            snapshot.push_str(&format!("\n{index:02} |{content}|"));
            if trailing > 0 {
                snapshot.push_str(&format!(" + {trailing} spaces"));
            }
        }
        snapshot
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
        let hello = PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 7,
                client_id: 1,
            },
            placement: PluginPlacement::Unknown,
        };
        let payload = serde_json::to_string(&hello).unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_RENDERER_HELLO, Some(payload), BTreeMap::new()))
                .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::RendererHello(hello)));
    }

    #[test]
    fn parses_config_editor_hello() {
        let hello = PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 11,
                client_id: 1,
            },
            placement: PluginPlacement::Unknown,
        };
        let payload = serde_json::to_string(&hello).unwrap();

        let parsed = parse_controller_message(&pipe(
            MSG_CONFIG_EDITOR_HELLO,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::ConfigEditorHello(hello)));
    }

    #[test]
    fn parses_config_inspect_request() {
        let request = ConfigInspectRequest {
            client_id: 4,
            origin_tab_id: 7,
            node_key: NodeKey::Tab(7),
            config_plugin_url: "andamento-config".to_owned(),
            controller_plugin_url: "andamento-controller".to_owned(),
        };
        let payload = serde_json::to_string(&request).unwrap();

        let parsed =
            parse_controller_message(&pipe(MSG_CONFIG_INSPECT, Some(payload), BTreeMap::new()))
                .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::ConfigInspect(request)));
    }

    #[test]
    fn parses_materialize_latent_request() {
        let request = andamento_shared::MaterializeLatentRequest {
            action_target: "flotilla:convoys/dev/latent-tabs".to_owned(),
            path: andamento_shared::GroupPath(vec![andamento_shared::GroupSegment {
                key: "flotilla.convoy".to_owned(),
                value: andamento_shared::MetadataValue::Text("dev/latent-tabs".to_owned()),
                label: Some("latent tabs".to_owned()),
            }]),
            name: "latent tabs".to_owned(),
            recipe: "flotilla attach latent-tabs".to_owned(),
            checkout_path: Some("/work/andamento".to_owned()),
        };
        let payload = serde_json::to_string(&request).unwrap();

        let parsed = parse_controller_message(&pipe(
            andamento_shared::MSG_MATERIALIZE_LATENT,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::MaterializeLatent(request)));
    }

    #[test]
    fn parses_activate_entity_request() {
        let entity = andamento_shared::EntityRef {
            kind: "vessel".to_owned(),
            id: "dev/focus/worker@lab".to_owned(),
        };
        let payload = serde_json::to_string(&entity).unwrap();

        let parsed = parse_controller_message(&pipe(
            andamento_shared::MSG_ACTIVATE_ENTITY,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(parsed, Some(ControllerMessage::ActivateEntity(entity)));
    }

    #[test]
    fn materialize_recipe_uses_checkout_path_as_cwd() {
        let command = command_for_materialize_recipe(
            "flotilla attach 'latent tabs'",
            Some("/work/andamento"),
            None,
        )
        .unwrap();

        assert_eq!(command.path.to_string_lossy(), "/bin/sh");
        assert_eq!(command.args, vec!["-c", "flotilla attach 'latent tabs'"]);
        assert_eq!(command.cwd, Some(PathBuf::from("/work/andamento")));
    }

    #[test]
    fn materialize_recipe_uses_home_as_cwd_without_checkout_path() {
        let command = command_for_materialize_recipe(
            "flotilla attach 'latent tabs'",
            None,
            Some(std::path::Path::new("/home/robert")),
        )
        .unwrap();

        assert_eq!(command.path.to_string_lossy(), "/bin/sh");
        assert_eq!(command.args, vec!["-c", "flotilla attach 'latent tabs'"]);
        assert_eq!(command.cwd, Some(PathBuf::from("/home/robert")));
    }

    #[test]
    fn materialize_recipe_requires_home_without_checkout_path() {
        assert!(
            command_for_materialize_recipe("flotilla attach latent-tabs", None, None).is_none()
        );
    }

    #[test]
    fn config_inspect_message_targets_existing_editor_by_plugin_id() {
        let request = ConfigInspectRequest {
            client_id: 4,
            origin_tab_id: 7,
            node_key: NodeKey::Tab(7),
            config_plugin_url: "andamento-config".to_owned(),
            controller_plugin_url: "andamento-controller".to_owned(),
        };
        let target = RendererHello {
            plugin_id: 44,
            client_id: 4,
        };

        let message = config_inspect_message_for_existing_editor(&request, target).unwrap();

        assert_eq!(message.plugin_url, None);
        assert_eq!(message.destination_plugin_id, Some(44));
        assert_eq!(message.destination_client_id, Some(4));
        assert_eq!(message.message_name, MSG_CONFIG_INSPECT);
        assert!(message.new_plugin_args.is_none());
        let payload: ConfigInspectRequest =
            serde_json::from_str(message.message_payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload, request);
    }

    #[test]
    fn config_inspect_message_launches_floating_editor_when_needed() {
        let request = ConfigInspectRequest {
            client_id: 4,
            origin_tab_id: 7,
            node_key: NodeKey::Tab(7),
            config_plugin_url: "andamento-config".to_owned(),
            controller_plugin_url: "andamento-controller".to_owned(),
        };

        let message = config_inspect_message_for_new_editor(&request).unwrap();

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-config"));
        assert_eq!(message.destination_client_id, Some(4));
        assert_eq!(message.message_name, MSG_CONFIG_INSPECT);
        assert_eq!(
            message
                .plugin_config
                .get("controller_plugin_url")
                .map(String::as_str),
            Some("andamento-controller")
        );
        assert_eq!(
            message.plugin_config.get("rail_scope").map(String::as_str),
            Some("tab:7")
        );
        assert_eq!(
            message
                .plugin_config
                .get("close_on_hidden")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            message
                .plugin_config
                .get("origin_tab_id")
                .map(String::as_str),
            Some("7")
        );
        assert_eq!(
            message.plugin_config.get("pane_kind").map(String::as_str),
            Some("floating")
        );
        let new_plugin_args = message.new_plugin_args.as_ref().unwrap();
        assert_eq!(new_plugin_args.should_float, Some(true));
        assert_eq!(new_plugin_args.should_focus, Some(true));
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

        let config = parse_rail_config(&configuration);

        assert_eq!(config.structure, RailStructure::BoxPerTab);
    }

    #[test]
    fn parses_segment_between_color_from_plugin_configuration() {
        let mut configuration = BTreeMap::new();
        configuration.insert(
            "rail_segment_between_color".to_owned(),
            "#010203".to_owned(),
        );

        let config = parse_rail_config(&configuration);

        assert_eq!(
            config.segment_between_color,
            Some(andamento_shared::RailRgbColor {
                red: 1,
                green: 2,
                blue: 3,
            })
        );
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
            segment_between_color: None,
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
                segment_between_color: None,
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
            node_variable_overrides: vec![],
            metadata_patches: vec![],
            rail_ui_state: RailUiState::default(),
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
    fn parses_metadata_visibility_set_request() {
        let request = andamento_shared::MetadataVisibilitySetRequest {
            client_id: 4,
            node_key: NodeKey::Tab(7),
            state: Some(andamento_shared::MetadataTriState::MetaChildren),
        };
        let payload = serde_json::to_string(&request).unwrap();

        let parsed = parse_controller_message(&pipe(
            andamento_shared::MSG_SET_METADATA_VISIBILITY,
            Some(payload),
            BTreeMap::new(),
        ))
        .unwrap();

        assert_eq!(
            parsed,
            Some(ControllerMessage::SetMetadataVisibility(request))
        );
    }

    #[test]
    fn collapse_state_from_one_rail_is_projected_to_every_rail() {
        let path = andamento_shared::GroupPath(vec![andamento_shared::GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: andamento_shared::MetadataValue::Text("/repo".to_owned()),
            label: Some("repo".to_owned()),
        }]);
        let mut state = ControllerState::default();
        for (plugin_id, tab_id) in [(11, 1), (22, 2)] {
            let hello = PluginRegistrationHello {
                identity: RendererHello {
                    plugin_id,
                    client_id: 4,
                },
                placement: PluginPlacement::Tab {
                    tab_id,
                    pane_kind: andamento_shared::PluginPaneKind::Tiled,
                },
            };
            let payload = serde_json::to_string(&hello).unwrap();
            handle_pipe_message(
                &mut state,
                pipe(MSG_RENDERER_HELLO, Some(payload), BTreeMap::new()),
            );
        }
        let request = andamento_shared::RailUiAction::ToggleGroup { path: path.clone() };
        let payload = serde_json::to_string(&request).unwrap();

        let result = handle_pipe_message(
            &mut state,
            pipe(
                andamento_shared::MSG_RAIL_UI_ACTION,
                Some(payload.clone()),
                BTreeMap::new(),
            ),
        );

        assert!(result.broadcast_rail_ui_state);
        let targets = state.rail_plugin_targets();
        assert_eq!(targets.len(), 2);
        assert!(targets.into_iter().all(|target| state
            .view_model_for_client(target.client_id)
            .collapsed_groups
            == vec![path.clone()]));
        assert_eq!(
            state.view_model_for_client(5).collapsed_groups,
            vec![path.clone()]
        );

        let result = handle_pipe_message(
            &mut state,
            pipe(
                andamento_shared::MSG_RAIL_UI_ACTION,
                Some(payload),
                BTreeMap::new(),
            ),
        );

        assert!(result.broadcast_rail_ui_state);
        assert!(state.rail_plugin_targets().into_iter().all(|target| state
            .view_model_for_client(target.client_id)
            .collapsed_groups
            .is_empty()));
    }

    #[test]
    fn rail_ui_actions_update_one_session_snapshot() {
        let mut state = ControllerState::default();
        let scroll = serde_json::to_string(&RailUiAction::ScrollBy { delta: 6 }).unwrap();

        let result = handle_pipe_message(
            &mut state,
            pipe(MSG_RAIL_UI_ACTION, Some(scroll), BTreeMap::new()),
        );

        assert!(result.broadcast_rail_ui_state);
        assert_eq!(state.rail_ui_state().scroll_offset, 6);

        let ensure_visible =
            serde_json::to_string(&RailUiAction::SetScrollOffset { offset: 19 }).unwrap();
        handle_pipe_message(
            &mut state,
            pipe(MSG_RAIL_UI_ACTION, Some(ensure_visible), BTreeMap::new()),
        );
        assert_eq!(state.rail_ui_state().scroll_offset, 19);

        let reset = serde_json::to_string(&RailUiAction::ResetScroll).unwrap();
        handle_pipe_message(
            &mut state,
            pipe(MSG_RAIL_UI_ACTION, Some(reset), BTreeMap::new()),
        );

        assert_eq!(state.rail_ui_state().scroll_offset, 0);
    }

    #[test]
    fn controller_clone_total_orders_same_sequence_broadcasts() {
        let mut clone = ControllerState::default();
        let current = RailUiState {
            revision: RailUiRevision {
                sequence: 7,
                writer_client_id: 2,
            },
            collapsed_groups: vec![],
            scroll_offset: 11,
            variables: BTreeMap::new(),
        };
        let winner = RailUiState {
            revision: RailUiRevision {
                sequence: 7,
                writer_client_id: 3,
            },
            collapsed_groups: vec![],
            scroll_offset: 13,
            variables: BTreeMap::new(),
        };
        let stale = RailUiState {
            revision: RailUiRevision {
                sequence: 7,
                writer_client_id: 1,
            },
            collapsed_groups: vec![],
            scroll_offset: 1,
            variables: BTreeMap::new(),
        };

        handle_pipe_message(
            &mut clone,
            pipe(
                MSG_RAIL_UI_STATE,
                Some(serde_json::to_string(&current).unwrap()),
                BTreeMap::new(),
            ),
        );
        handle_pipe_message(
            &mut clone,
            pipe(
                MSG_RAIL_UI_STATE,
                Some(serde_json::to_string(&winner).unwrap()),
                BTreeMap::new(),
            ),
        );
        handle_pipe_message(
            &mut clone,
            pipe(
                MSG_RAIL_UI_STATE,
                Some(serde_json::to_string(&stale).unwrap()),
                BTreeMap::new(),
            ),
        );

        assert_eq!(clone.rail_ui_state(), winner);
    }

    #[test]
    fn late_rail_request_rebroadcasts_current_ui_state_without_target_filters() {
        let mut state = ControllerState::default();
        state.set_rail_ui_writer_client_id(6);
        let action = serde_json::to_string(&RailUiAction::ScrollBy { delta: 4 }).unwrap();
        let action_result = handle_pipe_message(
            &mut state,
            pipe(MSG_RAIL_UI_ACTION, Some(action), BTreeMap::new()),
        );

        let result = handle_pipe_message(
            &mut state,
            pipe(MSG_REQUEST_RAIL_UI_STATE, None, BTreeMap::new()),
        );
        let message = rail_ui_state_broadcast_message(&state.rail_ui_state()).unwrap();

        assert!(action_result.broadcast_rail_ui_state);
        assert!(result.broadcast_rail_ui_state);
        assert_eq!(message.plugin_url, None);
        assert_eq!(message.destination_plugin_id, None);
        assert_eq!(message.destination_client_id, None);
        assert_eq!(message.message_name, MSG_RAIL_UI_STATE);
        assert_eq!(
            serde_json::from_str::<RailUiState>(message.message_payload.as_deref().unwrap())
                .unwrap(),
            RailUiState {
                revision: RailUiRevision {
                    sequence: 1,
                    writer_client_id: 6,
                },
                collapsed_groups: vec![],
                scroll_offset: 4,
                variables: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn display_variable_action_broadcasts_state_and_pushes_a_new_view_model() {
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"display-variable "show-issues" type="bool" default=true label="Issues" icon="I""#,
                )
                .unwrap(),
            ),
        ));
        let action = serde_json::to_string(&RailUiAction::ToggleVariable {
            name: "show-issues".to_owned(),
        })
        .unwrap();

        let result = handle_pipe_message(
            &mut state,
            pipe(MSG_RAIL_UI_ACTION, Some(action), BTreeMap::new()),
        );

        assert!(result.broadcast_rail_ui_state);
        assert!(result.state_changed);
        assert_eq!(
            result.view_model_push_reason,
            Some(ViewModelPushReason::PipeDisplayVariable)
        );
        assert_eq!(
            state.rail_ui_state().variables.get("show-issues"),
            Some(&andamento_shared::DisplayVariableValue::Bool(false))
        );
    }

    #[test]
    fn node_variable_request_sets_group_config_override() {
        let group_path = andamento_shared::GroupPath(vec![andamento_shared::GroupSegment {
            key: "vcs.repo".to_owned(),
            value: andamento_shared::MetadataValue::Text("example/repo".to_owned()),
            label: Some("repo".to_owned()),
        }]);
        let request = andamento_shared::NodeVariableSetRequest {
            client_id: 4,
            node_key: NodeKey::Group(group_path.clone()),
            name: "child-layout".to_owned(),
            value: Some("strip".to_owned()),
        };
        let payload = serde_json::to_string(&request).unwrap();
        let mut state = ControllerState::default();
        state.set_template_catalog(None);
        state.set_rail_config(RailConfig::default());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Entity(andamento_shared::EntityRef {
                kind: "repo".to_owned(),
                id: "example/repo".to_owned(),
            }),
            source_id: "test".to_owned(),
            set: BTreeMap::from([
                (
                    "entity.kind".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: andamento_shared::MetadataValue::Text("repo".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "entity.id".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: andamento_shared::MetadataValue::Text("example/repo".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "vcs.repo".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: andamento_shared::MetadataValue::Text("example/repo".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });
        state.update_tabs(vec![state::ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".to_owned(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        state.set_pane_cwd(PaneTarget::Terminal(1), "/repo".to_owned());

        let changed = handle_pipe_message(
            &mut state,
            pipe(
                andamento_shared::MSG_SET_NODE_VARIABLE,
                Some(payload),
                BTreeMap::new(),
            ),
        );

        assert!(changed.state_changed);
        assert_eq!(
            changed.view_model_push_reason,
            Some(ViewModelPushReason::PipeMetadataControls)
        );
        assert!(!state.set_node_variable(
            NodeKey::Group(group_path),
            "child-layout".to_owned(),
            Some("strip".to_owned()),
        ));
    }

    #[test]
    fn node_variable_request_sets_root_config_override() {
        let request = andamento_shared::NodeVariableSetRequest {
            client_id: 4,
            node_key: NodeKey::Root,
            name: "child-layout".to_owned(),
            value: Some("strip".to_owned()),
        };
        let payload = serde_json::to_string(&request).unwrap();
        let mut state = ControllerState::default();
        state.set_template_catalog(None);

        let changed = handle_pipe_message(
            &mut state,
            pipe(
                andamento_shared::MSG_SET_NODE_VARIABLE,
                Some(payload),
                BTreeMap::new(),
            ),
        );

        let model = state.view_model();
        let root_variables = model
            .template_config
            .effective_variables
            .iter()
            .find(|variables| variables.node == NodeKey::Root)
            .expect("root variables");
        assert!(changed.state_changed);
        assert_eq!(
            changed.view_model_push_reason,
            Some(ViewModelPushReason::PipeMetadataControls)
        );
        assert_eq!(
            root_variables
                .values
                .get("child-layout")
                .map(|entry| entry.value.as_str()),
            Some("strip")
        );
    }

    #[test]
    fn set_rail_config_message_updates_controller_state() {
        let config = RailConfig {
            structure: RailStructure::BoxPerTab,
            segment_between_color: None,
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
    fn grouping_template_message_switches_projection_without_metadata_changes() {
        let request = GroupingTemplateSetRequest {
            name: Some("flotilla.default".to_owned()),
        };
        let payload = serde_json::to_string(&request).unwrap();
        let mut state = ControllerState::default();

        let changed = handle_pipe_message(
            &mut state,
            pipe(MSG_SET_GROUPING_TEMPLATE, Some(payload), BTreeMap::new()),
        );

        assert!(changed.state_changed);
        assert_eq!(
            changed.view_model_push_reason,
            Some(ViewModelPushReason::PipeConfig)
        );
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
        let patch = andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                "tab.subject".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: andamento_shared::MetadataValue::Text("checkout".to_owned()),
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
            .find(|metadata| metadata.target == andamento_shared::ResolvedMetadataTarget::Tab(1))
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
            Some(&andamento_shared::MetadataValue::Text(
                "checkout".to_owned()
            ))
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
    fn entity_target_patch_stream_renders_latent_nodes_without_live_panes() {
        let mut state = ControllerState::default();
        state.set_template_catalog(None);
        let live_grouping_config = andamento_shared::grouping_config::parse_grouping_config_kdl(
            include_str!("../../../templates/andamento-git.kdl"),
        )
        .unwrap();
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::with_bundled_defaults(
                live_grouping_config,
            ),
        ));
        state.set_rail_config(RailConfig::default());
        let project_patch = serde_json::json!({
            "type": "metadata-patch",
            "target": {
                "kind": "entity",
                "value": {
                    "kind": "project",
                    "id": "flotilla/andamento@fleet"
                }
            },
            "source_id": "flotilla-connector",
            "set": {
                "flotilla.project": {
                    "value": {
                        "type": "text",
                        "value": "flotilla/andamento@fleet"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 1
                },
                "flotilla.project.name": {
                    "value": {
                        "type": "text",
                        "value": "andamento"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 1
                },
                "action.primary.key": {
                    "value": {
                        "type": "text",
                        "value": "open"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 1
                },
                "count.convoys": {
                    "value": {
                        "type": "integer",
                        "value": 1
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 1
                }
            }
        })
        .to_string();
        let convoy_patch = serde_json::json!({
            "type": "metadata-patch",
            "target": {
                "kind": "entity",
                "value": {
                    "kind": "convoy",
                    "id": "andamento/fix-entity-patches-vanish@fleet"
                }
            },
            "source_id": "flotilla-connector",
            "set": {
                "flotilla.project": {
                    "value": {
                        "type": "text",
                        "value": "flotilla/andamento@fleet"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 2
                },
                "flotilla.project.name": {
                    "value": {
                        "type": "text",
                        "value": "andamento"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 2
                },
                "flotilla.convoy": {
                    "value": {
                        "type": "text",
                        "value": "andamento/fix-entity-patches-vanish@fleet"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 2
                },
                "flotilla.convoy.name": {
                    "value": {
                        "type": "text",
                        "value": "fix entity patches vanish"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 2
                },
                "display.label": {
                    "value": {
                        "type": "text",
                        "value": "fix entity patches vanish"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 2
                }
            }
        })
        .to_string();
        let issue_patch = serde_json::json!({
            "type": "metadata-patch",
            "target": {
                "kind": "entity",
                "value": {
                    "kind": "issue",
                    "id": "github/flotilla-org/andamento#37"
                }
            },
            "source_id": "flotilla-connector",
            "set": {
                "flotilla.project": {
                    "value": {
                        "type": "text",
                        "value": "flotilla/andamento@fleet"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 3
                },
                "flotilla.project.name": {
                    "value": {
                        "type": "text",
                        "value": "andamento"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 3
                },
                "flotilla.issue": {
                    "value": {
                        "type": "text",
                        "value": "github/flotilla-org/andamento#37"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 3
                },
                "display.label": {
                    "value": {
                        "type": "text",
                        "value": "#37 entity patches vanish"
                    },
                    "ttl_ms": null,
                    "precedence": null,
                    "ordinal": 3
                }
            }
        })
        .to_string();

        for payload in [project_patch, convoy_patch, issue_patch] {
            let result = handle_pipe_message(
                &mut state,
                pipe(MSG_APPLY_METADATA_PATCH, Some(payload), BTreeMap::new()),
            );
            assert!(result.state_changed);
            assert_eq!(
                result.view_model_push_reason,
                Some(ViewModelPushReason::PipeMetadata)
            );
        }

        let model = state.view_model();
        assert!(model.tabs.is_empty());
        assert!(model.rows.iter().any(|row| matches!(
            row,
            andamento_shared::RailRow::GroupHeader { path, .. }
                if path.0.iter().any(|segment| {
                    segment.key == "flotilla.project"
                        && segment.value
                            == andamento_shared::MetadataValue::Text(
                                "flotilla/andamento@fleet".to_owned()
                            )
                })
        )));
        assert!(model.rows.iter().any(|row| matches!(
            row,
            andamento_shared::RailRow::Latent { latent, .. }
                if latent.name == "fix entity patches vanish"
                    && latent.path.0.iter().any(|segment| segment.key == "flotilla.convoy")
        )));
        assert!(model.rows.iter().any(|row| matches!(
            row,
            andamento_shared::RailRow::Entity { entity, .. }
                if entity.entity.kind == "issue"
                    && entity.label == "#37 entity patches vanish"
                    && entity.templates.compact.is_some()
                    && entity.templates.detail.is_some()
        )));
    }

    // Native-only because this is the captured render-harness scenario and
    // renders through andamento-rail.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn git_rule_keeps_branchless_convoy_siblings_visible_under_their_repo() {
        let config_kdl = include_str!("../../../templates/andamento-git.kdl");
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(
                andamento_shared::template_config::parse_template_config_kdl(config_kdl).unwrap(),
            ),
        ));
        let live_grouping_config =
            andamento_shared::grouping_config::parse_grouping_config_kdl(config_kdl).unwrap();
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::with_bundled_defaults(
                live_grouping_config,
            ),
        ));
        state.set_rail_config(RailConfig::default());
        // Two convoys under one repo, so single-member conflation cannot fold
        // the repo group away and its header genuinely renders.
        for (index, name) in [(1, "scoping-regression"), (2, "second-convoy")] {
            let convoy_patch = serde_json::json!({
                "type": "metadata-patch",
                "target": {
                    "kind": "entity",
                    "value": { "kind": "convoy", "id": format!("flotilla/{name}@fleet") }
                },
                "source_id": "flotilla-connector",
                "set": {
                    "vcs.repo": {
                        "value": { "type": "text", "value": "flotilla-org/andamento" },
                        "ttl_ms": null, "precedence": null, "ordinal": index
                    },
                    "repo.name": {
                        "value": { "type": "text", "value": "andamento" },
                        "ttl_ms": null, "precedence": null, "ordinal": index
                    },
                    "flotilla.convoy": {
                        "value": { "type": "text", "value": format!("flotilla/{name}@fleet") },
                        "ttl_ms": null, "precedence": null, "ordinal": index
                    },
                    "flotilla.convoy.name": {
                        "value": { "type": "text", "value": name },
                        "ttl_ms": null, "precedence": null, "ordinal": index
                    },
                    "display.label": {
                        "value": { "type": "text", "value": name },
                        "ttl_ms": null, "precedence": null, "ordinal": index
                    }
                }
            })
            .to_string();
            let result = handle_pipe_message(
                &mut state,
                pipe(
                    MSG_APPLY_METADATA_PATCH,
                    Some(convoy_patch),
                    BTreeMap::new(),
                ),
            );
            assert!(result.state_changed);
        }

        let model = state.view_model();
        let repo_path = andamento_shared::GroupPath(vec![andamento_shared::GroupSegment {
            key: "vcs.repo".to_owned(),
            value: andamento_shared::MetadataValue::Text("flotilla-org/andamento".to_owned()),
            label: Some("andamento".to_owned()),
        }]);
        let latent_rows = model
            .rows
            .iter()
            .filter_map(|row| match row {
                andamento_shared::RailRow::Latent {
                    latent,
                    parent_path,
                    ..
                } => Some((latent, parent_path)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(latent_rows.len(), 2);
        assert!(latent_rows.iter().all(|(latent, parent_path)| {
            latent.path.0.starts_with(&repo_path.0)
                && latent.path.0.last().is_some_and(|segment| {
                    segment.key == "flotilla.convoy"
                        && segment.value
                            == andamento_shared::MetadataValue::Text(latent.entity.id.clone())
                })
                && parent_path.as_ref() == Some(&latent.path)
        }));

        let templates =
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(
                andamento_shared::template_config::parse_template_config_kdl(config_kdl).unwrap(),
            );
        let rendered = andamento_rail::render::render_lines_with_template_catalog(
            Some(&model),
            &[],
            model.rows.len() * 4 + 8,
            60,
            true,
            Some(&templates),
        );

        // Repo-level group header renders the repo through the git template.
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("flotilla-org/andamento")),
            "repo-level header should render the vcs.repo value; rows={:?}, lines={:?}",
            model.rows,
            rendered.lines,
        );
        for label in ["scoping-regression", "second-convoy"] {
            assert!(
                rendered.lines.iter().any(|line| line.contains(label)),
                "branchless convoy {label} must render beneath its repo: {:?}",
                rendered.lines
            );
        }
    }

    // Native-only because this exercises the #53 frame-snapshot seam through
    // the controller's real grouping pipeline and the real rail renderer.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn vessel_with_project_facts_renders_under_its_project_before_its_repo() {
        let git_config_kdl = include_str!("../../../templates/andamento-git.kdl");
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(
                andamento_shared::template_config::parse_template_config_kdl(git_config_kdl)
                    .unwrap(),
            ),
        ));
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::with_bundled_defaults(
                andamento_shared::grouping_config::parse_grouping_config_kdl(git_config_kdl)
                    .unwrap(),
            ),
        ));
        state.set_rail_config(RailConfig::default());
        state.update_tabs(vec![state::ControllerTab {
            tab_id: 1,
            position: 0,
            name: "work".to_owned(),
            active: true,
        }]);

        let project_patch = serde_json::json!({
            "type": "metadata-patch",
            "target": {
                "kind": "entity",
                "value": { "kind": "project", "id": "flotilla/andamento@fleet" }
            },
            "source_id": "flotilla-connector",
            "set": {
                "flotilla.project": {
                    "value": { "type": "text", "value": "flotilla/andamento@fleet" },
                    "ttl_ms": null, "precedence": null, "ordinal": 0
                },
                "flotilla.project.name": {
                    "value": { "type": "text", "value": "Andamento" },
                    "ttl_ms": null, "precedence": null, "ordinal": 0
                },
                "display.label": {
                    "value": { "type": "text", "value": "Andamento" },
                    "ttl_ms": null, "precedence": null, "ordinal": 0
                }
            }
        })
        .to_string();
        assert!(
            handle_pipe_message(
                &mut state,
                pipe(
                    MSG_APPLY_METADATA_PATCH,
                    Some(project_patch),
                    BTreeMap::new(),
                ),
            )
            .state_changed
        );

        let vessel_id = "flotilla/rail-grouping-unification/work@fleet";
        let vessel_patch = serde_json::json!({
            "type": "metadata-patch",
            "target": {
                "kind": "entity",
                "value": { "kind": "vessel", "id": vessel_id }
            },
            "source_id": "flotilla-connector",
            "set": {
                "flotilla.project": {
                    "value": { "type": "text", "value": "flotilla/andamento@fleet" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "flotilla.project.name": {
                    "value": { "type": "text", "value": "Andamento" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "vcs.repo": {
                    "value": { "type": "text", "value": "flotilla-org/andamento" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "vcs.repo.name": {
                    "value": { "type": "text", "value": "andamento" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "repo.name": {
                    "value": { "type": "text", "value": "andamento" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "flotilla.vessel": {
                    "value": { "type": "text", "value": vessel_id },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "flotilla.vessel.name": {
                    "value": { "type": "text", "value": "work" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                },
                "display.label": {
                    "value": { "type": "text", "value": "work" },
                    "ttl_ms": null, "precedence": null, "ordinal": 1
                }
            }
        })
        .to_string();
        assert!(
            handle_pipe_message(
                &mut state,
                pipe(
                    MSG_APPLY_METADATA_PATCH,
                    Some(vessel_patch),
                    BTreeMap::new(),
                ),
            )
            .state_changed
        );
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "flotilla-actuator".to_owned(),
            set: BTreeMap::from([
                (
                    "entity.kind".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: andamento_shared::MetadataValue::Text("vessel".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "entity.id".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: andamento_shared::MetadataValue::Text(vessel_id.to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });

        let model = state.view_model();
        let root_path = model
            .rows
            .iter()
            .find_map(|row| match row {
                andamento_shared::RailRow::GroupHeader { path, .. } => Some(path),
                _ => None,
            })
            .expect("project root group");
        assert_eq!(
            root_path.0.first().map(|segment| segment.key.as_str()),
            Some("flotilla.project"),
            "a Flotilla vessel must be rooted under its project, not its vcs repo"
        );

        let templates =
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(
                andamento_shared::template_config::parse_template_config_kdl(git_config_kdl)
                    .unwrap(),
            );
        let rendered = andamento_rail::render::render_lines_with_template_catalog(
            Some(&model),
            &[],
            20,
            48,
            true,
            Some(&templates),
        );
        assert!(
            rendered.lines.iter().any(|line| line.contains("Andamento")),
            "the project group must be visible in the rendered frame: {:?}",
            rendered.lines
        );
        insta::assert_snapshot!(
            "vessel_with_project_facts_renders_under_its_project_before_its_repo",
            rail_frame_snapshot(&rendered.lines, 48)
        );
    }

    #[test]
    fn metadata_patch_cli_pipe_is_unblocked_without_output() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![state::ControllerTab {
            tab_id: 1,
            position: 0,
            name: "main".to_owned(),
            active: true,
        }]);
        let patch = andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                "tab.subject".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: andamento_shared::MetadataValue::Text("checkout".to_owned()),
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
            cli_pipe(MSG_APPLY_METADATA_PATCH, Some(payload), "pipe-42"),
        );

        assert!(result.state_changed);
        assert_eq!(result.cli_pipe_output, None);
        assert_eq!(result.cli_pipe_unblock, Some("pipe-42".to_owned()));
    }

    #[test]
    fn duplicate_metadata_patch_message_does_not_request_state_broadcast() {
        let mut state = ControllerState::default();
        let patch = andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: andamento_shared::MetadataValue::Text("zellij-org/zellij".to_owned()),
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
        state.set_rail_config(RailConfig::default());
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
        let observed: Vec<andamento_shared::ObservedMetadataIdentity> =
            serde_json::from_str(&output.output).unwrap();

        assert!(!result.state_changed);
        assert_eq!(output.pipe_id, "pipe-1");
        assert_eq!(result.cli_pipe_unblock, Some("pipe-1".to_owned()));
        assert!(observed.iter().any(|identity| {
            identity.identity
                == andamento_shared::MetadataIdentity {
                    key: "zellij.pane.cwd".to_owned(),
                    value: andamento_shared::MetadataValue::Text(cwd.clone()),
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
    fn duplicate_renderer_hello_skips_full_model_push_but_replays_ui_state() {
        let mut state = ControllerState::default();
        let hello = PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 9,
                client_id: 1,
            },
            placement: PluginPlacement::Unknown,
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
        assert!(first.broadcast_rail_ui_state);
        assert!(second.broadcast_rail_ui_state);
    }

    #[test]
    fn config_inspect_request_sets_client_inspected_node() {
        let mut state = ControllerState::default();
        let request = ConfigInspectRequest {
            client_id: 4,
            origin_tab_id: 7,
            node_key: NodeKey::Tab(7),
            config_plugin_url: "andamento-config".to_owned(),
            controller_plugin_url: "andamento-controller".to_owned(),
        };
        let payload = serde_json::to_string(&request).unwrap();

        let result = handle_pipe_message(
            &mut state,
            pipe(MSG_CONFIG_INSPECT, Some(payload), BTreeMap::new()),
        );

        assert!(result.state_changed);
        assert_eq!(
            result.view_model_push_reason,
            Some(ViewModelPushReason::PipeMetadataControls)
        );
        assert_eq!(
            state.view_model_for_client(4).inspected_node,
            Some(NodeKey::Tab(7))
        );
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
    fn metadata_pipe_bursts_defer_full_view_model_fanout() {
        let mut pending = PendingViewModelPush::default();
        let timers_scheduled = (0..100)
            .filter(|_| {
                assert_eq!(
                    pipe_view_model_delivery(ViewModelPushReason::PipeMetadata),
                    PipeViewModelDelivery::Coalesced
                );
                pending.queue(ViewModelPushReason::PipeMetadata)
            })
            .count();

        assert_eq!(timers_scheduled, 1);
        assert_eq!(pending.take(), vec![ViewModelPushReason::PipeMetadata]);
        assert_eq!(
            pipe_view_model_delivery(ViewModelPushReason::PipeRendererHello),
            PipeViewModelDelivery::Immediate
        );
        assert_eq!(
            pipe_view_model_delivery(ViewModelPushReason::PipeRequestState),
            PipeViewModelDelivery::Immediate
        );
    }

    #[test]
    fn view_models_are_built_once_per_client() {
        let grouped = group_view_model_targets(vec![
            RendererHello {
                plugin_id: 10,
                client_id: 1,
            },
            RendererHello {
                plugin_id: 11,
                client_id: 1,
            },
            RendererHello {
                plugin_id: 20,
                client_id: 2,
            },
        ]);

        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, 1);
        assert_eq!(grouped[0].1.len(), 2);
        assert_eq!(grouped[1].0, 2);
        assert_eq!(grouped[1].1.len(), 1);
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
