mod metadata;
mod state;

use std::collections::BTreeMap;

use state::ControllerState;
#[cfg(target_family = "wasm")]
use tabs_shared::PaneTarget;
#[cfg(target_family = "wasm")]
use tabs_shared::MSG_VIEW_MODEL;
use tabs_shared::{
    ExternalMessage, RailConfig, RailGroupingMode, RailSizingPreset, RailStructure, RendererHello,
    SortMode, MSG_CLEAR_PANE_STATUS, MSG_CONFIG_EDITOR_HELLO, MSG_RENDERER_HELLO,
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
}

#[cfg(target_family = "wasm")]
register_plugin!(PluginState);

#[cfg(target_family = "wasm")]
impl ZellijPlugin for PluginState {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
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
        if handle_pipe_message(&mut self.state, pipe_message) {
            self.push_view_model_to_rails();
        }
        false
    }
}

#[cfg(target_family = "wasm")]
impl PluginState {
    fn push_view_model_to_rails(&self) {
        if !self.permissions_granted {
            return;
        }
        let Ok(payload) = serde_json::to_string(&self.state.view_model()) else {
            return;
        };
        for rail_id in self.state.rail_plugin_ids() {
            pipe_message_to_plugin(
                MessageToPlugin::new(MSG_VIEW_MODEL)
                    .with_destination_plugin_id(rail_id)
                    .with_payload(payload.clone()),
            );
        }
    }
}

fn handle_pipe_message(state: &mut ControllerState, pipe_message: PipeMessage) -> bool {
    match parse_controller_message(&pipe_message) {
        Ok(Some(ControllerMessage::External(ExternalMessage::SetPaneStatus(status)))) => {
            state.set_status(status);
            true
        }
        Ok(Some(ControllerMessage::External(ExternalMessage::ClearPaneStatus { pane_id }))) => {
            state.clear_status(pane_id);
            true
        }
        Ok(Some(ControllerMessage::RendererHello(hello))) => {
            state.register_rail(hello);
            true
        }
        Ok(Some(ControllerMessage::ConfigEditorHello(hello))) => {
            state.register_config_editor(hello);
            true
        }
        Ok(Some(ControllerMessage::TogglePin(tab_id))) => {
            state.toggle_pin(tab_id);
            true
        }
        Ok(Some(ControllerMessage::SetSortMode(sort_mode))) => {
            state.set_sort_mode(sort_mode);
            true
        }
        Ok(Some(ControllerMessage::SetRailConfig(config))) => {
            state.set_rail_config(config);
            true
        }
        Ok(Some(ControllerMessage::RequestState)) => true,
        Ok(None) => false,
        Err(error) => {
            eprintln!("tabs-controller: {error}");
            false
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

        assert!(changed);
        assert_eq!(state.view_model().config, config);
    }
}
