//! Host theme conversion; terminal drawing lives in andamento-terminal.
pub use andamento_terminal::render::*;
fn color_from_zellij(color: zellij_tile::prelude::PaletteColor) -> PaletteColor {
    match color {
        zellij_tile::prelude::PaletteColor::Rgb(rgb) => PaletteColor::Rgb(rgb),
        zellij_tile::prelude::PaletteColor::EightBit(index) => PaletteColor::EightBit(index),
    }
}
pub fn theme_from_zellij(colors: zellij_tile::prelude::Styling) -> RenderTheme {
    RenderTheme {
        // Match the normal tab bar's selected/unselected foreground choices,
        // but do not paint a background over the terminal default.
        active_border: color_from_zellij(colors.ribbon_selected.background),
        inactive_border: color_from_zellij(colors.ribbon_unselected.background),
        body_foreground: color_from_zellij(colors.text_unselected.base),
        segment_active_background: color_from_zellij(colors.ribbon_selected.background),
        segment_active_foreground: color_from_zellij(colors.ribbon_selected.base),
        segment_inactive_background: color_from_zellij(colors.ribbon_unselected.background),
        segment_inactive_foreground: color_from_zellij(colors.ribbon_unselected.base),
        // The plugin API Styling currently does not expose Zellij's top-level
        // theme background. Keep this explicit in RenderTheme so config/API
        // work can supply the real terminal-like background without changing
        // strip rendering.
        segment_between_background: color_from_zellij(colors.text_unselected.background),
    }
}

#[cfg(test)]
mod frame_snapshots;
#[cfg(test)]
mod test_fixtures;
