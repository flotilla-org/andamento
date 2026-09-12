pub use andamento_core::grouping_config::*;
use std::path::Path;

pub fn load_grouping_catalog_from_file(
    path: &str,
) -> Result<GroupingConfigCatalog, GroupingConfigError> {
    let resolved = zellij_tile::vfs::resolve_host_path(path)
        .map_err(|err| GroupingConfigError::Io(err.to_string()))?;
    let content = std::fs::read_to_string(&resolved).map_err(|source| {
        GroupingConfigError::Io(format!("failed to read {}: {source}", resolved.display()))
    })?;
    let config = if Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "kdl")
    {
        parse_grouping_config_kdl(&content)?
    } else {
        parse_grouping_config_json(&content)?
    };
    Ok(GroupingConfigCatalog::with_bundled_defaults(config))
}
