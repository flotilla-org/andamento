//! The Dashboard: its records, display variables, local sections, groups
//! and refs, and sibling orders.
use super::*;
use andamento_core::PlacementLoopKey;

/// The names of the records Andamento holds, one per line (no trailing
/// newline): "dashboard", then "workspace/<id>" for each registered
/// workspace (<id> a hyphenated UUID).
#[no_mangle]
pub unsafe extern "C" fn andamento_record_names(h: *mut Andamento, out: *mut Bytes, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| write_bytes(out, h.sidebar.record_names().join("\n").into_bytes())).is_some() as u32
}
/// A record's generation, nonzero: it changes whenever the record does and
/// is never reused. 0 with an error for an unknown record. Store a record
/// when its generation differs from the one last stored.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_generation(h: *mut Andamento, name: Text, error_out: *mut *mut c_char) -> u64 {
    run(h, error_out, |h| h.sidebar.record_generation(&name.read()?)).unwrap_or(0)
}
/// A record to store as it is: its bytes are private to Andamento and may
/// change from one build to the next. Free them with andamento_bytes_free.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_get(h: *mut Andamento, name: Text, out: *mut Bytes, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| write_bytes(out, h.sidebar.export_record(&name.read()?)?.into_bytes())).is_some() as u32
}
/// Restore a record andamento_record_get returned, before the first
/// observe. A record that doesn't read is rejected without change.
/// Restoring "workspace/<id>" registers the workspace.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_put(h: *mut Andamento, name: Text, record: Text, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| h.sidebar.import_record(&name.read()?, &record.read()?)).is_some() as u32
}

/// Set a declared display variable without a snapshot action, so a host
/// can restore one before the first snapshot: "true"/"false" for a boolean,
/// one of an enum's values, or empty for its default. Unknown variables and
/// values the declaration doesn't allow are rejected.
#[no_mangle]
pub unsafe extern "C" fn andamento_set_display_variable(h: *mut Andamento, name: Text, value: Text, error_out: *mut *mut c_char) -> u32 {
    use andamento_core::DisplayVariableValue;
    run(h, error_out, |h| {
        let name = name.read()?;
        let value = value.read()?;
        if value.is_empty() {
            return h.sidebar.set_display_variable(&name, None);
        }
        let flag = match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        // An enum may declare "true" or "false" as a value.
        match flag {
            Some(flag) => h
                .sidebar
                .set_display_variable(&name, Some(DisplayVariableValue::Bool(flag)))
                .or_else(|_| h.sidebar.set_display_variable(&name, Some(DisplayVariableValue::Enum(value)))),
            None => h.sidebar.set_display_variable(&name, Some(DisplayVariableValue::Enum(value))),
        }
    })
    .is_some() as u32
}
/// Set the host-owned order of the sibling run identified by `loop_key` (a
/// node's loop_key). Entities the list omits keep data order relative to it
/// (each after its nearest named data-order predecessor, else first);
/// entities that no longer match are ignored. An empty list clears it.
#[no_mangle]
pub unsafe extern "C" fn andamento_set_sibling_order(
    h: *mut Andamento,
    loop_key: Text,
    entities: *const EntityView,
    count: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let key = loop_key.read()?;
        let key = PlacementLoopKey::decode(&key).ok_or("invalid loop key")?;
        let order = slice(entities, count)?.iter().map(|e| e.read()).collect::<Result<Vec<_>, String>>()?;
        h.sidebar.set_sibling_order(key, order)
    })
    .is_some() as u32
}

/// A fact of a local entity: ANDAMENTO_FACT_TEXT, _BOOL, _INTEGER or
/// _ENTITY (one entity reference, such as .section, .group or .target).
#[repr(C)]
pub struct LocalFact {
    pub key: Text,
    pub kind: u32,
    pub text: Text,
    pub integer: i64,
    pub entity: EntityView,
}
/// Set a local section (".section"), group (".group") or ref (".ref", a
/// pin), replacing all its facts. Andamento owns local entities (their
/// provider is "local") and keeps them in the dashboard record; they are
/// placed exactly as the same entities published as facts.
#[no_mangle]
pub unsafe extern "C" fn andamento_local_set(
    h: *mut Andamento,
    kind: Text,
    id: Text,
    facts: *const LocalFact,
    count: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let entity = EntityRef::local(kind.read()?, id.read()?);
        let mut values = BTreeMap::new();
        for f in slice(facts, count)? {
            let value = match f.kind {
                ANDAMENTO_FACT_ENTITY => MetadataValue::EntityRefs(vec![f.entity.read()?]),
                kind => scalar(kind, f.text, f.integer)?,
            };
            if values.insert(f.key.read()?, value).is_some() {
                return Err("duplicate fact key".into());
            }
        }
        h.sidebar.set_local(entity, values)
    })
    .is_some() as u32
}
/// Remove a local entity; removing an unknown one is harmless.
#[no_mangle]
pub unsafe extern "C" fn andamento_local_remove(h: *mut Andamento, kind: Text, id: Text, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.remove_local(&EntityRef::local(kind.read()?, id.read()?));
        Ok(())
    })
    .is_some() as u32
}

/// A local entity and its facts, as andamento_local_set takes them (an
/// entity reference fact gives its first entity).
#[repr(C)]
pub struct LocalEntityView {
    pub entity: EntityView,
    pub facts: *const LocalFact,
    pub fact_count: usize,
}

/// A saved sibling order: the run's loop key (as a node's loop_key) and its
/// entities in order.
#[repr(C)]
pub struct SiblingOrderView {
    pub loop_key: Text,
    pub entities: *const EntityView,
    pub entity_count: usize,
}

/// The Dashboard's local entities and saved sibling orders, as stored.
#[repr(C)]
pub struct DashboardView {
    /// The dashboard record's generation (andamento_record_generation): read
    /// again when it moves.
    pub generation: u64,
    pub locals: *const LocalEntityView,
    pub local_count: usize,
    pub orders: *const SiblingOrderView,
    pub order_count: usize,
}

/// What a DashboardView points into.
#[repr(C)]
struct OwnedDashboard {
    value: DashboardView,
    locals: Vec<(EntityRef, Vec<(String, MetadataValue)>)>,
    orders: Vec<(String, Vec<EntityRef>)>,
    local_views: Vec<LocalEntityView>,
    fact_views: Vec<Vec<LocalFact>>,
    order_views: Vec<SiblingOrderView>,
    entity_views: Vec<Vec<EntityView>>,
}

/// The Dashboard's local entities, in key order, and its saved sibling
/// orders. Free it with andamento_dashboard_free.
#[no_mangle]
pub unsafe extern "C" fn andamento_dashboard_get(h: *mut Andamento, error_out: *mut *mut c_char) -> *mut DashboardView {
    run(h, error_out, |h| {
        let mut owned = Box::new(OwnedDashboard {
            value: DashboardView {
                generation: h.sidebar.record_generation("dashboard")?,
                locals: ptr::null(),
                local_count: 0,
                orders: ptr::null(),
                order_count: 0,
            },
            locals: h
                .sidebar
                .local_entities()
                .iter()
                .map(|(entity, facts)| {
                    let facts = facts.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    (entity.clone(), facts)
                })
                .collect(),
            orders: h.sidebar.sibling_orders().iter().map(|(key, entities)| (key.encode(), entities.clone())).collect(),
            local_views: Vec::new(),
            fact_views: Vec::new(),
            order_views: Vec::new(),
            entity_views: Vec::new(),
        });
        // Views borrow the boxed strings, whose bytes don't move.
        let fact_views: Vec<Vec<LocalFact>> =
            owned.locals.iter().map(|(_, facts)| facts.iter().map(|(key, value)| local_fact_view(key, value)).collect()).collect();
        let local_views = owned
            .locals
            .iter()
            .zip(&fact_views)
            .map(|((entity, _), facts)| LocalEntityView { entity: EntityView::of(entity), facts: facts.as_ptr(), fact_count: facts.len() })
            .collect();
        let entity_views: Vec<Vec<EntityView>> =
            owned.orders.iter().map(|(_, entities)| entities.iter().map(EntityView::of).collect()).collect();
        let order_views = owned
            .orders
            .iter()
            .zip(&entity_views)
            .map(|((key, _), entities)| SiblingOrderView {
                loop_key: Text::borrowed(key),
                entities: entities.as_ptr(),
                entity_count: entities.len(),
            })
            .collect();
        owned.fact_views = fact_views;
        owned.local_views = local_views;
        owned.entity_views = entity_views;
        owned.order_views = order_views;
        owned.value.locals = owned.local_views.as_ptr();
        owned.value.local_count = owned.local_views.len();
        owned.value.orders = owned.order_views.as_ptr();
        owned.value.order_count = owned.order_views.len();
        Ok(Box::into_raw(owned).cast::<DashboardView>())
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_dashboard_free(dashboard: *mut DashboardView) {
    if !dashboard.is_null() {
        drop(Box::from_raw(dashboard.cast::<OwnedDashboard>()));
    }
}

fn local_fact_view(key: &str, value: &MetadataValue) -> LocalFact {
    let mut fact = LocalFact {
        key: Text::borrowed(key),
        kind: ANDAMENTO_FACT_TEXT,
        text: Text::borrowed(""),
        integer: 0,
        entity: EntityView::empty(),
    };
    match value {
        MetadataValue::Text(text) => fact.text = Text::borrowed(text),
        MetadataValue::Bool(flag) => (fact.kind, fact.integer) = (ANDAMENTO_FACT_BOOL, *flag as i64),
        MetadataValue::Integer(n) => (fact.kind, fact.integer) = (ANDAMENTO_FACT_INTEGER, *n),
        MetadataValue::EntityRefs(refs) => {
            fact.kind = ANDAMENTO_FACT_ENTITY;
            if let Some(entity) = refs.first() {
                fact.entity = EntityView::of(entity);
            }
        }
        // Local facts are never lists or group paths.
        MetadataValue::StringList(_) | MetadataValue::GroupPath(_) => {}
    }
    fact
}
