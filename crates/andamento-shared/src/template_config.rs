pub use andamento_core::template_config::*;
use std::path::Path;

pub fn load_template_catalog_from_json_file(
    path: &str,
) -> Result<TemplateConfigCatalog, TemplateConfigError> {
    let resolved = zellij_tile::vfs::resolve_host_path(path)
        .map_err(|err| TemplateConfigError::Io(err.to_string()))?;
    let content = std::fs::read_to_string(&resolved).map_err(|source| {
        TemplateConfigError::Io(format!("failed to read {}: {source}", resolved.display()))
    })?;
    parse_template_config_json(&content).map(TemplateConfigCatalog::with_bundled_defaults)
}

pub fn load_template_catalog_from_file(
    path: &str,
) -> Result<TemplateConfigCatalog, TemplateConfigError> {
    let resolved = zellij_tile::vfs::resolve_host_path(path)
        .map_err(|err| TemplateConfigError::Io(err.to_string()))?;
    let content = std::fs::read_to_string(&resolved).map_err(|source| {
        TemplateConfigError::Io(format!("failed to read {}: {source}", resolved.display()))
    })?;
    if Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "kdl")
    {
        parse_template_config_kdl(&content).map(TemplateConfigCatalog::with_bundled_defaults)
    } else {
        parse_template_config_json(&content).map(TemplateConfigCatalog::with_bundled_defaults)
    }
}
