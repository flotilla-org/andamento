//! Helpers shared by the integration tests.
#![allow(dead_code)]

/// The subscription test facts come from.
pub const PROVIDER: &str = "test";

/// An entity of the test subscription.
pub fn entity(kind: &str, id: &str) -> andamento_core::EntityRef {
    andamento_core::EntityRef::new(PROVIDER, kind, id)
}

/// The `.workspace` entity Andamento presents for an unplaced workspace.
pub fn fallback(tab: u64) -> andamento_core::EntityRef {
    let id = andamento_core::tab_workspace_id(tab).to_string();
    andamento_core::EntityRef::local(".workspace", id)
}
