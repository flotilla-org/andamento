use std::cmp::{max, min};
use std::collections::BTreeMap;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::*;

#[derive(Default, Debug)]
struct State {
    tabs: Vec<TabInfo>,
    active_tab_idx: usize, // 1-indexed to match switch_tab_to
    mode_info: ModeInfo,
    visible_tab_positions: Vec<usize>, // 0-indexed tab positions rendered per row
    permissions_granted: bool,
    permission_status: Option<PermissionStatus>,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        set_selectable(false);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::ModeUpdate,
            EventType::Mouse,
            EventType::PermissionRequestResult,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::ModeUpdate(mode_info) => {
                if self.mode_info != mode_info {
                    should_render = true;
                }
                self.mode_info = mode_info;
            }
            Event::TabUpdate(tabs) => {
                let next_active = tabs
                    .iter()
                    .position(|t| t.active)
                    .map(|i| i + 1)
                    .unwrap_or(1);
                if self.tabs != tabs || self.active_tab_idx != next_active {
                    should_render = true;
                }
                self.tabs = tabs;
                self.active_tab_idx = next_active;
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse);
            }
            Event::PermissionRequestResult(status) => {
                self.permission_status = Some(status);
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                should_render = true;
            }
            _ => {}
        }
        should_render
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if rows == 0 || cols == 0 {
            return;
        }
        if !self.permissions_granted {
            let message = match self.permission_status {
                Some(PermissionStatus::Denied) => "andamento: permissions denied",
                Some(PermissionStatus::Granted) => "andamento: waiting for tab state",
                None => "andamento: waiting for permissions",
            };
            self.render_message(message, rows, cols);
            return;
        }
        if self.tabs.is_empty() {
            self.render_message("andamento: waiting for tabs", rows, cols);
            return;
        }

        let visible_range = self.visible_range(rows);
        self.visible_tab_positions = visible_range.clone().collect();

        let mut out = String::new();
        for row in 0..rows {
            let line = if let Some(tab_index) = self.visible_tab_positions.get(row).copied() {
                let tab = &self.tabs[tab_index];
                let prefix = if tab.active { ">" } else { " " };
                let status = self.status_suffix(tab);
                let mut title = self.display_name(tab);
                if !status.is_empty() {
                    title.push(' ');
                    title.push_str(&status);
                }
                pad_to_width(
                    &truncate_to_width(&format!("{} {}", prefix, title), cols),
                    cols,
                )
            } else {
                " ".repeat(cols)
            };
            out.push_str(&line);
            if row + 1 < rows {
                out.push('\n');
            }
        }
        print!("{}", out);
    }
}

impl State {
    fn render_message(&self, message: &str, rows: usize, cols: usize) {
        let mut out = String::new();
        for row in 0..rows {
            let line = if row == 0 {
                pad_to_width(&truncate_to_width(message, cols), cols)
            } else {
                " ".repeat(cols)
            };
            out.push_str(&line);
            if row + 1 < rows {
                out.push('\n');
            }
        }
        print!("{}", out);
    }

    fn handle_mouse(&mut self, mouse: Mouse) {
        match mouse {
            Mouse::LeftClick(line, _) if line >= 0 => {
                let row = line as usize;
                if let Some(tab_position) = self.visible_tab_positions.get(row).copied() {
                    switch_tab_to((tab_position + 1) as u32);
                }
            }
            Mouse::ScrollUp(_) => {
                let prev = max(self.active_tab_idx.saturating_sub(1), 1);
                switch_tab_to(prev as u32);
            }
            Mouse::ScrollDown(_) => {
                let next = min(self.active_tab_idx + 1, self.tabs.len());
                switch_tab_to(next as u32);
            }
            _ => {}
        }
    }

    fn visible_range(&self, rows: usize) -> std::ops::Range<usize> {
        let total = self.tabs.len();
        if rows >= total {
            return 0..total;
        }
        let active_zero_idx = self
            .active_tab_idx
            .saturating_sub(1)
            .min(total.saturating_sub(1));
        let half = rows / 2;
        let mut start = active_zero_idx.saturating_sub(half);
        let mut end = start + rows;
        if end > total {
            end = total;
            start = end.saturating_sub(rows);
        }
        start..end
    }

    fn display_name(&self, tab: &TabInfo) -> String {
        if tab.active && self.mode_info.mode == InputMode::RenameTab && tab.name.is_empty() {
            "Enter name...".to_owned()
        } else if tab.name.is_empty() {
            format!("Tab {}", tab.position + 1)
        } else {
            tab.name.clone()
        }
    }

    fn status_suffix(&self, tab: &TabInfo) -> String {
        let mut bits = Vec::new();
        if tab.is_fullscreen_active {
            bits.push("FULL");
        }
        if tab.is_sync_panes_active {
            bits.push("SYNC");
        }
        if tab.has_bell_notification || tab.is_flashing_bell {
            bits.push("!");
        }
        if bits.is_empty() {
            String::new()
        } else {
            format!("[{}]", bits.join(","))
        }
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_owned();
    }

    let mut current_width = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        current_width += ch_width;
    }
    out.push('…');
    out
}

fn pad_to_width(text: &str, width: usize) -> String {
    let text_width = text.width();
    if text_width >= width {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + (width - text_width));
    out.push_str(text);
    out.push_str(&" ".repeat(width - text_width));
    out
}
