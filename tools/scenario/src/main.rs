use flotilla_manifest::{
    projection::{project_catalog, Catalog, CatalogInput},
    recipe::FlotillaRecipes,
};
use flotilla_protocol::result_set::{
    AwarenessNode, ConvoyRow, IndependentRow, ProjectRepositoriesRow, StandingRoleRow,
};
use serde::Deserialize;
use std::io::{self, Write};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    offset_ms: u64,
    #[serde(default)]
    awareness: Option<Vec<AwarenessNode>>,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    convoys: Vec<ConvoyRow>,
    #[serde(default)]
    independents: Vec<IndependentRow>,
    #[serde(default)]
    standing_roles: Vec<StandingRoleRow>,
    #[serde(default)]
    project_repositories: Vec<ProjectRepositoriesRow>,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: andamento-scenario SCENARIO.json")?;
    let steps: Vec<Step> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let mut previous = Catalog::default();
    let mut previous_ms = 0;
    let mut out = io::stdout().lock();
    for step in steps {
        if step.offset_ms < previous_ms {
            return Err("decreasing scenario offset".into());
        }
        let catalog = project_catalog(
            &CatalogInput {
                awareness: step.awareness.as_deref(),
                convoys: &step.convoys,
                independents: &step.independents,
                standing_roles: &step.standing_roles,
                project_repositories: &step.project_repositories,
            },
            &FlotillaRecipes::new("flotilla"),
        );
        let patches = if step.refresh {
            catalog.reassert_patches()
        } else {
            catalog.diff_patches(&previous)
        };
        for patch in patches {
            let mut patch = serde_json::to_value(patch)?;
            patch["type"] = serde_json::json!("metadata-patch");
            serde_json::to_writer(
                &mut out,
                &serde_json::json!({"offset_ms":step.offset_ms,"patch":patch}),
            )?;
            writeln!(out)?;
        }
        previous = catalog;
        previous_ms = step.offset_ms;
    }
    Ok(())
}
