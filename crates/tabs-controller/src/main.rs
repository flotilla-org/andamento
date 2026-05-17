mod metadata;
mod state;

use std::collections::BTreeMap;

use state::ControllerState;
#[cfg(target_family = "wasm")]
use tabs_shared::PaneTarget;
#[cfg(target_family = "wasm")]
use tabs_shared::MSG_VIEW_MODEL;
use tabs_shared::{
    ControllerBootstrapSnapshot, ExternalMessage, RailConfig, RailGroupingMode, RailSizingPreset,
    RailStructure, RendererHello, SortMode, MSG_CLEAR_PANE_STATUS, MSG_CONFIG_EDITOR_HELLO,
    MSG_CONTROLLER_BOOTSTRAP_REQUEST, MSG_CONTROLLER_BOOTSTRAP_STATE, MSG_RENDERER_HELLO,
    MSG_REQUEST_STATE, MSG_SET_PANE_STATUS, MSG_SET_RAIL_CONFIG, MSG_SET_SORT_MODE, MSG_TOGGLE_PIN,
};
use zellij_tile::prelude::*;

#[cfg(not(target_family = "wasm"))]
fn main() {}

#[cfg(target_family = "wasm")]
#[derive(Default)]
struct PluginState {
    state: ControllerState,
    permissions_granted: bool,
    own_identity: Option<RendererHello>,
    bootstrap_requested: bool,
}

#[cfg(target_family = "wasm")]
register_plugin!(PluginState);

#[cfg(target_family = "wasm")]
impl ZellijPlugin for PluginState {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_identity = Some(RendererHello {
            plugin_id: ids.plugin_id,
            client_id: ids.client_id,
        });
        self.state
            .set_rail_config(parse_rail_config(&configuration));
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::CwdChanged,
            EventType::PermissionRequestResult,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                self.request_bootstrap_snapshot();
            }
            Event::TabUpdate(tabs) => {
                self.state.update_tabs_from_zellij(tabs);
                self.push_view_model_to_rails();
            }
            Event::PaneUpdate(pane_manifest) => {
                let live_plugin_ids = pane_manifest
                    .panes
                    .values()
                    .flat_map(|panes| panes.iter())
                    .filter(|pane| pane.is_plugin)
                    .map(|pane| pane.id)
                    .collect();
                self.state.retain_rails(&live_plugin_ids);
                self.state.update_panes_from_manifest(pane_manifest);
                for terminal_id in self.state.terminal_panes_for_cwd_refresh() {
                    if let Ok(cwd) = get_pane_cwd(PaneId::Terminal(terminal_id)) {
                        self.state.set_pane_cwd(
                            PaneTarget::Terminal(terminal_id),
                            cwd.display().to_string(),
                        );
                    }
                }
                self.push_view_model_to_rails();
            }
            Event::CwdChanged(pane_id, cwd, _) => {
                if let PaneId::Terminal(id) = pane_id {
                    self.state
                        .set_pane_cwd(PaneTarget::Terminal(id), cwd.display().to_string());
                }
                self.push_view_model_to_rails();
            }
            _ => {}
        }
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let result = handle_pipe_message(&mut self.state, pipe_message);
        if let Some(requester) = result.bootstrap_request {
            if self.own_identity.as_ref() != Some(&requester) {
                self.send_bootstrap_snapshot_to(requester);
            }
        }
        if result.state_changed {
            self.push_view_model_to_rails();
        }
        false
    }
}

#[cfg(target_family = "wasm")]
impl PluginState {
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

    fn send_bootstrap_snapshot_to(&self, requester: RendererHello) {
        if !self.permissions_granted {
            return;
        }
        let Ok(payload) = serde_json::to_string(&self.state.bootstrap_snapshot()) else {
            return;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_CONTROLLER_BOOTSTRAP_STATE)
                .with_destination_plugin_id(requester.plugin_id)
                .with_destination_client_id(requester.client_id)
                .with_payload(payload),
        );
    }

    fn push_view_model_to_rails(&self) {
        if !self.permissions_granted {
            return;
        }
        let Ok(payload) = serde_json::to_string(&self.state.view_model()) else {
            return;
        };
        for rail in self.state.rail_plugin_targets() {
            pipe_message_to_plugin(
                MessageToPlugin::new(MSG_VIEW_MODEL)
                    .with_destination_plugin_id(rail.plugin_id)
                    .with_destination_client_id(rail.client_id)
                    .with_payload(payload.clone()),
            );
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct HandlePipeResult {
    state_changed: bool,
    bootstrap_request: Option<RendererHello>,
}

fn handle_pipe_message(state: &mut ControllerState, pipe_message: PipeMessage) -> HandlePipeResult {
    match parse_controller_message(&pipe_message) {
        Ok(Some(ControllerMessage::External(ExternalMessage::SetPaneStatus(status)))) => {
            state.set_status(status);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::External(ExternalMessage::ClearPaneStatus { pane_id }))) => {
            state.clear_status(pane_id);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::RendererHello(hello))) => {
            state.register_rail(hello);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::ConfigEditorHello(hello))) => {
            state.register_config_editor(hello);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::TogglePin(tab_id))) => {
            state.toggle_pin(tab_id);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::SetSortMode(sort_mode))) => {
            state.set_sort_mode(sort_mode);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::SetRailConfig(config))) => {
            state.set_rail_config(config);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
        Ok(Some(ControllerMessage::RequestState)) => HandlePipeResult {
            state_changed: true,
            bootstrap_request: None,
        },
        Ok(Some(ControllerMessage::BootstrapRequest(requester))) => HandlePipeResult {
            state_changed: false,
            bootstrap_request: Some(requester),
        },
        Ok(Some(ControllerMessage::BootstrapState(snapshot))) => {
            state.apply_bootstrap_snapshot(snapshot);
            HandlePipeResult {
                state_changed: true,
                bootstrap_request: None,
            }
        }
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
    }
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

#[derive(Debug, PartialEq, Eq)]
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
}

fn parse_controller_message(
    pipe_message: &PipeMessage,
) -> Result<Option<ControllerMessage>, String> {
    match pipe_message.name.as_str() {
        MSG_SET_PANE_STATUS | MSG_CLEAR_PANE_STATUS => {
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
    fn parses_set_rail_config_payload() {
        let config = RailConfig {
            structure: RailStructure::SplitAroundActive,
            sizing: RailSizingPreset::PinnedLarge,
            grouping: RailGroupingMode::None,
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
    fn set_rail_config_message_updates_controller_state() {
        let config = RailConfig {
            structure: RailStructure::BoxPerTab,
            sizing: RailSizingPreset::Compact,
            grouping: RailGroupingMode::None,
        };
        let payload = serde_json::to_string(&config).unwrap();
        let mut state = ControllerState::default();

        let changed = handle_pipe_message(
            &mut state,
            pipe(MSG_SET_RAIL_CONFIG, Some(payload), BTreeMap::new()),
        );

        assert!(changed.state_changed);
        assert_eq!(state.view_model().config, config);
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
}
