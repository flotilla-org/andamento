#ifndef ANDAMENTO_H
#define ANDAMENTO_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct Andamento Andamento;
typedef struct AndamentoSnapshot AndamentoSnapshot;
typedef struct AndamentoEffects AndamentoEffects;
typedef struct { const uint8_t *data; size_t len; } AndamentoText;
#define ANDAMENTO_NONE SIZE_MAX
/* ABI 3: a host-supplied 128-bit Workspace ID, passed by value. */
typedef struct { uint8_t bytes[16]; } AndamentoWorkspaceId;

/* ABI 2 replaces the experimental JSON request ABI; no compatibility promise
 * with ABI 1. Additive symbols preserve ABI 2; statically linked hosts pin a
 * library revision, while dynamic hosts can use dlsym to probe optional symbols directly.
 * Tags have uint32_t storage; do not use C enum size assumptions.
 *
 * ABI 3 is additive: every ABI 2 call and struct is unchanged, and a host may
 * accept either version. ABI 3 adds calls carrying 128-bit Workspace IDs
 * (AndamentoWorkspaceId), marked "ABI 3" below. The host supplies every
 * Workspace ID (UUIDv7 in practice); Andamento never generates one and treats
 * the 16 bytes as opaque. An ABI 2 uint64_t ID n is the Workspace ID whose
 * first 8 bytes are zero and whose last 8 bytes are n, big-endian. No UUIDv7
 * has that form, so both kinds can be used side by side: a workspace observed
 * through ABI 2 as 7 is the same workspace as that embedded ID in ABI 3 calls.
 * ABI 2 uint64_t output fields report the embedded n, or 0 for a wider ID;
 * read wider IDs through the ABI 3 getters. JSON patches name a workspace as
 * {"kind":"tab","value":7} or {"kind":"tab","value":"<uuid>"} (hyphenated
 * or 32 hex digits).
 *
 * Providers (ABI 3). An entity is {provider, kind, id}: the provider is the
 * Dashboard's subscription ID, which the host supplies per patch source, and
 * the same kind and ID under two providers are two entities. A patch never
 * names its own provider: the _from calls stamp theirs over every entity a
 * patch names (its target, entity references among its values, and the
 * entity a workspace's entity.kind/entity.id or .host.kind/.host.id facts
 * name). ABI 2 calls, and JSON entities without a "provider", use the
 * default provider: "local" until andamento_set_default_provider changes it,
 * so a host can move to providers gradually. Local sections, groups and refs
 * are always "local". Provider names are nonempty UTF-8.
 *
 * Serialize calls on a sidebar. All input buffers/arrays are borrowed for the
 * call only; text is UTF-8, length-delimited, and may contain NUL. NULL input
 * pointers require zero length. Non-NULL pointers must be valid/aligned for
 * the declared length. All handles must be live, correctly typed allocations
 * from this library (or NULL). Never use a handle after releasing it.
 *
 * Mutations return 1 on success, 0 on error. Optional error_out is cleared on
 * entry and receives an owned NUL-terminated error; free with string_free.
 * Invalid input is rejected before mutation. A panic poisons the sidebar;
 * destroy/recreate it. No mutation computes or serializes an output snapshot
 * (action validation may resolve presentation state internally).
 */
uint32_t andamento_abi_version(void);
Andamento *andamento_create(const uint8_t *config_kdl, size_t len, char **error_out);
uint32_t andamento_configure(Andamento *, AndamentoText kdl, char **error_out);

/* Available with the default Cargo feature "json". Optional ingress decoder for ONE producer MetadataPatch, not a request
 * envelope. Encoding and transport are adapter choices. Future CBOR ingress
 * can feed the same typed Rust core without changing rendering or actions. */
uint32_t andamento_apply_patch_json(Andamento *, uint64_t now_ms, AndamentoText json, char **error_out);
/* ABI 3: one JSON patch from provider (a subscription ID), stamped over every
 * entity it names. Requires the "json" feature. */
uint32_t andamento_apply_patch_json_from(Andamento *, uint64_t now_ms, AndamentoText provider,
    AndamentoText json, char **error_out);
/* ABI 3: the default provider ABI 2 calls stamp and entities named by kind
 * and ID alone belong to. Set it before applying facts or importing records. */
uint32_t andamento_set_default_provider(Andamento *, AndamentoText provider, char **error_out);
/* ABI 3: remove all of a provider's facts in one call, as when its
 * subscription is removed. What open workspaces and refs showed of its
 * entities is retained, as when facts expire: a workspace keeps its subject
 * and its row stays on its path; a ref keeps presenting its target. */
uint32_t andamento_provider_retract(Andamento *, AndamentoText provider, char **error_out);
/* ABI 3: mark a provider stale (stale nonzero), when its connection drops, or
 * fresh again (zero). A stale provider's facts are kept as they were when it
 * became stale: TTLs don't expire them, and nodes presenting its entities
 * report stale (andamento_snapshot_node_provider). Facts already expired
 * then stay expired. Fresh again, each TTL fact that was live renews its
 * lease from the current time, as though the provider had reasserted it: one
 * it doesn't reassert within its TTL then expires. Facts without a TTL are
 * unaffected either way. Retracting a provider clears its staleness. */
uint32_t andamento_provider_set_stale(Andamento *, AndamentoText provider, uint32_t stale, char **error_out);

/* A native scalar entity-fact convenience interface, not a C copy of the full
 * producer schema. Other targets, lists, and path values are currently handled
 * through the optional decoder or Rust's typed MetadataPatch interface.
 * kind: unset=0, text=1, bool=2 (integer 0/1), integer=3.
 * Optional flags distinguish absent values from zero. A batch is validated
 * before applying. Keys must be unique within the batch.
 */
enum { ANDAMENTO_FACT_UNSET, ANDAMENTO_FACT_TEXT, ANDAMENTO_FACT_BOOL, ANDAMENTO_FACT_INTEGER };
typedef struct {
    AndamentoText key;
    uint32_t kind;
    AndamentoText text;
    int64_t integer;
    uint32_t has_ttl;
    uint64_t ttl_ms;
    uint32_t has_precedence;
    int64_t precedence;
    uint32_t has_ordinal;
    int64_t ordinal;
} AndamentoFact;
uint32_t andamento_apply_entity(Andamento *, uint64_t now_ms, AndamentoText kind,
    AndamentoText id, AndamentoText source, const AndamentoFact *, size_t count, char **error_out);
/* ABI 3: the same scalar facts on a workspace (a tab target), e.g. the
 * .host.kind/.host.id naming its host entity. */
uint32_t andamento_apply_workspace(Andamento *, uint64_t now_ms, AndamentoWorkspaceId workspace,
    AndamentoText source, const AndamentoFact *, size_t count, char **error_out);
/* ABI 3: the same calls from provider (a subscription ID). */
uint32_t andamento_apply_entity_from(Andamento *, uint64_t now_ms, AndamentoText provider,
    AndamentoText kind, AndamentoText id, AndamentoText source, const AndamentoFact *, size_t count,
    char **error_out);
uint32_t andamento_apply_workspace_from(Andamento *, uint64_t now_ms, AndamentoText provider,
    AndamentoWorkspaceId workspace, AndamentoText source, const AndamentoFact *, size_t count,
    char **error_out);
/* Monotonic milliseconds scoped to this client. Tick advances expiry without
 * facts. Drain facts/topology/completions before acquiring a render snapshot. */
uint32_t andamento_tick(Andamento *, uint64_t now_ms, char **error_out);

typedef struct { uint64_t id; size_t position; AndamentoText name; uint32_t selected; } AndamentoWorkspace;
enum { ANDAMENTO_PANE_TERMINAL, ANDAMENTO_PANE_PLUGIN };
typedef struct { uint64_t workspace_id; uint32_t pane_id, kind, selectable, focused; int64_t ordinal; } AndamentoPane;
/* Ephemeral directory associations, replaced in full on each call. Supply
 * topology first. Paths must be normalized by the host; matching is exact.
 * Empty paths/unknown workspace IDs are ignored. Explicit identity wins.
 * This never persists an opener identity or opts into managed replacement. */
typedef struct { uint64_t workspace_id; AndamentoText cwd; } AndamentoWorkdir;
uint32_t andamento_observe_workdirs(Andamento *, const AndamentoWorkdir *, size_t count, char **error);
/* Full replacement of topology. ABI 2 IDs may be scoped to this client;
 * ABI 3 IDs are the host's Workspace IDs, the same in every client.
 * Pane observations currently retain Zellij's terminal/plugin u32 identity.
 * Native hosts with wider IDs or other view kinds must pass an empty pane list;
 * do not truncate IDs or classify arbitrary native views as plugins. */
uint32_t andamento_observe(Andamento *, const AndamentoWorkspace *, size_t count,
    const AndamentoPane *, size_t pane_count, char **error_out);
enum { ANDAMENTO_COMPLETE_FOCUS, ANDAMENTO_COMPLETE_MATERIALIZE, ANDAMENTO_COMPLETE_ERROR };
/* Complete every focus/materialize request, including cancellation and timeout.
 * Inspect needs no completion. Duplicates/unknown request IDs are ignored.
 * Only MATERIALIZE uses workspace_id; only ERROR reads message. */
uint32_t andamento_complete(Andamento *, uint64_t request_id, uint32_t outcome,
    uint64_t workspace_id, AndamentoText message, char **error_out);
/* ABI 3 topology and completion: as above, with 128-bit IDs. Use either
 * observe call; each replaces the full topology. */
typedef struct { AndamentoWorkspaceId id; size_t position; AndamentoText name; uint32_t selected; } AndamentoWorkspace3;
typedef struct { AndamentoWorkspaceId workspace_id; uint32_t pane_id, kind, selectable, focused; int64_t ordinal; } AndamentoPane3;
typedef struct { AndamentoWorkspaceId workspace_id; AndamentoText cwd; } AndamentoWorkdir3;
uint32_t andamento_observe3(Andamento *, const AndamentoWorkspace3 *, size_t count,
    const AndamentoPane3 *, size_t pane_count, char **error_out);
uint32_t andamento_observe_workdirs3(Andamento *, const AndamentoWorkdir3 *, size_t count, char **error);
uint32_t andamento_complete3(Andamento *, uint64_t request_id, uint32_t outcome,
    AndamentoWorkspaceId workspace_id, AndamentoText message, char **error_out);
/* ABI 3: declare a workspace the host created itself, such as one the user
 * made, which never went through MATERIALIZE. A successful MATERIALIZE
 * completion registers its workspace too. Registration is independent of
 * topology (either may come first; closing does not forget) and does not
 * change the snapshot. Forget a workspace the host deleted rather than kept.
 * Registering twice and forgetting an unknown ID are harmless.
 * andamento_workspace_registered returns 1 if registered, else 0. */
uint32_t andamento_workspace_register(Andamento *, AndamentoWorkspaceId, char **error_out);
uint32_t andamento_workspace_forget(Andamento *, AndamentoWorkspaceId, char **error_out);
uint32_t andamento_workspace_registered(Andamento *, AndamentoWorkspaceId, char **error_out);

/* Snapshot owns all returned text. It survives sidebar mutation/destruction;
 * release only after rendering and all borrowed text use have finished.
 * Getters return 0 for NULL handles/output or invalid index, leaving output
 * untouched. Nodes are preorder: sections have parent NONE; each other parent
 * is an earlier node index. Collapsed children remain available.
 * Keys are opaque, stable within this client for a placement across snapshots;
 * qualify section keys with is_section. Do not parse their representation.
 */
/* Returns 1 if the snapshot still matches this client, 0 for a stale, NULL,
 * or other-client snapshot (no error), or an invalid/poisoned client (error).
 * Unchanged heartbeats/ticks preserve validity. Recipe changes invalidate it
 * even if visible content is identical. Check after draining a batch; acquire
 * only when stale. This query does not build a snapshot. */
uint32_t andamento_snapshot_is_current(Andamento *, const AndamentoSnapshot *, char **error_out);
AndamentoSnapshot *andamento_snapshot_acquire(Andamento *, char **error_out);
enum { ANDAMENTO_CATALOG, ANDAMENTO_LATENT, ANDAMENTO_OPENING, ANDAMENTO_LIVE };
typedef struct {
    size_t parent;
    uint32_t is_section;
    AndamentoText key, entity_kind, entity_id, label, layout, form;
    uint32_t state;
    uint64_t workspace_id;
    uint32_t selected, openable, collapsed, pinned;
    size_t first_field, field_count, first_detail, detail_count;
    size_t first_control, control_count, activate, toggle;
} AndamentoNode;
typedef struct { AndamentoText default_host; uint32_t has_order; int64_t order; } AndamentoRegionHints;
enum { ANDAMENTO_FIELD_REQUIRED, ANDAMENTO_FIELD_OPTIONAL, ANDAMENTO_FIELD_PRIORITY };
typedef struct { AndamentoText text; uint32_t class_, has_priority; int64_t priority; } AndamentoField;
enum { ANDAMENTO_CONTROL_OPEN_CONFIG, ANDAMENTO_CONTROL_DISPLAY_VARIABLE,
       ANDAMENTO_CONTROL_SCROLL_DOWN, ANDAMENTO_CONTROL_SCROLL_UP, ANDAMENTO_CONTROL_INSPECT_ROOT };
/* Controls without a core action have action NONE; their tagged intent belongs
 * to the host (e.g. scrolling). The renderer decides geometry and glyph fallback. */
/* value_kind: absent=0, bool=1 (checked), enum=2 (value). */
typedef struct {
    uint32_t kind; AndamentoText label, glyph; size_t action;
    uint32_t value_kind, checked; AndamentoText value;
} AndamentoControl;
size_t andamento_snapshot_node_count(const AndamentoSnapshot *);
uint32_t andamento_snapshot_node(const AndamentoSnapshot *, size_t index, AndamentoNode *out);
/* ABI 3: a LIVE node's Workspace ID; returns 0 for other nodes. */
uint32_t andamento_snapshot_node_workspace(const AndamentoSnapshot *, size_t index, AndamentoWorkspaceId *out);
/* ABI 3: a node's entity provider, and stale (optional, may be NULL): 1 when
 * the provider of the entity the node presents (a ref's target) is stale.
 * Returns 0 for section nodes. */
uint32_t andamento_snapshot_node_provider(const AndamentoSnapshot *, size_t index,
    AndamentoText *provider, uint32_t *stale);
/* Snapshot-owned opaque loop invocation key. Compare for equality; do not parse.
 * Empty for section nodes. Additive ABI 2 API; AndamentoNode is unchanged. */
uint32_t andamento_snapshot_node_loop_key(const AndamentoSnapshot *, size_t index, AndamentoText *out);
/* Additive ABI 2 extension. Section defaults only; user layout is host-owned.
 * default_host is snapshot-owned and empty when absent; order is meaningful
 * only when has_order is nonzero. Returns 0 for nonsections/invalid arguments. */
uint32_t andamento_snapshot_region_hints(const AndamentoSnapshot *, size_t index, AndamentoRegionHints *out);
uint32_t andamento_snapshot_field(const AndamentoSnapshot *, size_t index, AndamentoField *out);
uint32_t andamento_snapshot_control(const AndamentoSnapshot *, size_t index, AndamentoControl *out);
uint32_t andamento_snapshot_control_variable(const AndamentoSnapshot *, size_t index,
    AndamentoText *name, uint32_t *persist);
size_t andamento_snapshot_diagnostic_count(const AndamentoSnapshot *);
uint32_t andamento_snapshot_diagnostic(const AndamentoSnapshot *, size_t index, AndamentoText *out);
/* Optional structured-detail extension (probe symbols with dlsym). ABI remains 2.
 * Opt in via andamento_snapshot_acquire_details; legacy acquire resolves no cards.
 * On a legacy snapshot detail_count is 0 and detail_find is ANDAMENTO_NONE.
 * All zero-length output Text values have non-NULL data pointers.
 * Indices, actions and borrowed UTF-8 texts belong to this immutable snapshot.
 * Enumerate catalog cards or find exact kind/id, independent of tree placement.
 * Invalid indices/pointers/UTF-8 return 0 (find returns ANDAMENTO_NONE).
 * Missing field: has_value=0. Known empty: has_value=1, text.len=0.
 * Observations use host monotonic milliseconds, not Unix time; observed_at_ms
 * is the winning fact's receipt time. TTL and source_id identify freshness and
 * producer provenance. Synthetic facts have no observation. Retained expired
 * facts may be stale; their original winning producer identity is retained.
 * Identical TTL heartbeats refresh receipt time (age is not time since change).
 * Relation display_text is the target's label, separate from its exact identity.
 * Duplicate references keep their first occurrence. A missing field has no source_key.
 * detail is ANDAMENTO_NONE when no catalog target exists. Pass the exact kind/id
 * navigation path to relation(); 0 means skip this index. Self links are omitted.
 * activate/copy_url use the existing andamento_dispatch validation/effect path:
 * activate means focus/materialize/open/inspect by current workspace controls;
 * copy_url means copy the subject URL. Both target the stable card entity.
 * Preview identity remains the existing node workspace_id; no field role
 * changes attachment policy or chooses a preview workspace.
 */
AndamentoSnapshot *andamento_snapshot_acquire_details(Andamento *, char **error_out);
enum { ANDAMENTO_DETAIL_IDENTITY, ANDAMENTO_DETAIL_TITLE, ANDAMENTO_DETAIL_STATE,
       ANDAMENTO_DETAIL_FACT, ANDAMENTO_DETAIL_RELATION };
typedef struct { AndamentoText kind, id; } AndamentoEntity;
/* Additive ABI 2: host-owned order of one sibling run, keyed by the text from
 * andamento_snapshot_node_loop_key. Entities omitted from the list keep data
 * order relative to it (each after its nearest named data-order predecessor,
 * else first); entities that no longer match are ignored. Empty list clears. */
uint32_t andamento_set_sibling_order(Andamento *, AndamentoText loop_key,
    const AndamentoEntity *entities, size_t count, char **error);
/* ABI 3: an entity with its provider; the ABI 3 variants below name it. */
typedef struct { AndamentoText provider, kind, id; } AndamentoEntity3;
uint32_t andamento_set_sibling_order3(Andamento *, AndamentoText loop_key,
    const AndamentoEntity3 *entities, size_t count, char **error);

/* ABI 3: named records. Andamento owns the sidebar's logical state and exports
 * it as records, KDL text in a versioned envelope; the host decides where each
 * is stored and when it is written. Andamento never touches the filesystem.
 *   "dashboard": persisted display variables, row collapse, sibling orders,
 *     placement variables, local sections, groups and refs, and the sidebar
 *     arrangement.
 *   "workspace/<id>": one per registered workspace (<id> as Andamento prints
 *     it: decimal for an embedded ID, else a hyphenated UUID): its subject, and
 *     the subject and its path as last seen (label, ended or retained, when
 *     last seen, and the facts placement reads), so its row is drawn where it
 *     was with no facts; and its slots (the cached Suggested Layout baseline
 *     with its arrangement hint, the user's overrides and own slots) and its
 *     arrangement document. A closed workspace keeps its record; forgetting
 *     the workspace drops it.
 * names: the record names, one per line (no trailing newline).
 * generation: nonzero; it changes when, and only when, the record's content
 *   changes, and is never reused. Write a record when its generation differs
 *   from the one last written; read it again after an import. Returns 0 with an
 *   error for an unknown record.
 * export: the record's KDL text (UTF-8), version 4: every entity names its
 *   provider, a workspace record holds its slots and arrangement, and the
 *   dashboard record its sidebar arrangement.
 * import: may come before the first observe and needs no facts. Importing
 *   "workspace/<id>" registers the workspace and binds it to its subject. A
 *   record that doesn't parse, has another version or names another record is
 *   rejected without change. Nodes this version doesn't know, directly inside
 *   the envelope, are kept and exported again. Version 1 records (before
 *   providers) still import: local sections, groups and refs get "local" and
 *   every other entity the default provider, so set the default first.
 *   Version 2 records import with no slots and no arrangement; version 3
 *   dashboard records with no sidebar arrangement, which the template's
 *   hints then place.
 * Bytes out-parameters are written only on success; free them with
 * andamento_bytes_free (a zeroed AndamentoBytes is harmless). */
typedef struct { uint8_t *data; size_t len; } AndamentoBytes;
uint32_t andamento_record_names(Andamento *, AndamentoBytes *out, char **error_out);
uint64_t andamento_record_generation(Andamento *, AndamentoText name, char **error_out);
uint32_t andamento_record_export(Andamento *, AndamentoText name, AndamentoBytes *out, char **error_out);
uint32_t andamento_record_import(Andamento *, AndamentoText name, AndamentoText kdl, char **error_out);
void andamento_bytes_free(AndamentoBytes);
/* ABI 3: set a declared display variable without a snapshot action, so a host
 * can restore one before the first snapshot. value is "true"/"false" for a
 * boolean, or one of an enum's values; empty returns it to its default.
 * Unknown variables and values the declaration doesn't allow are rejected. */
uint32_t andamento_set_display_variable(Andamento *, AndamentoText name, AndamentoText value, char **error_out);
/* ABI 3: local entities: the sections (".section"), groups (".group") and refs
 * (".ref", pins) people make. Andamento owns them and keeps them in the
 * dashboard record; set replaces all of an entity's facts, and remove deletes
 * it (removing an unknown one is harmless). They are placed exactly as the
 * same entities published as facts. A fact's kind is TEXT, BOOL (integer 0/1)
 * or INTEGER as for AndamentoFact, or ENTITY, one entity reference (such as
 * .section, .group or .target). Keys must be unique. */
enum { ANDAMENTO_FACT_ENTITY = 4 };
typedef struct {
    AndamentoText key;
    uint32_t kind;
    AndamentoText text;
    int64_t integer;
    AndamentoEntity entity;
} AndamentoLocalFact;
uint32_t andamento_local_set(Andamento *, AndamentoText kind, AndamentoText id,
    const AndamentoLocalFact *facts, size_t count, char **error_out);
/* ABI 3: as above, where an ENTITY fact names its provider (andamento_local_set
 * gives it the default provider), so a ref can pin any subscription's entity. */
typedef struct {
    AndamentoText key;
    uint32_t kind;
    AndamentoText text;
    int64_t integer;
    AndamentoEntity3 entity;
} AndamentoLocalFact3;
uint32_t andamento_local_set3(Andamento *, AndamentoText kind, AndamentoText id,
    const AndamentoLocalFact3 *facts, size_t count, char **error_out);
uint32_t andamento_local_remove(Andamento *, AndamentoText kind, AndamentoText id, char **error_out);
typedef struct {
    AndamentoEntity entity;
    AndamentoText label;
    size_t field_count, activate, copy_url;
    uint64_t now_ms;
    AndamentoText error;
    uint32_t has_workspace;
    uint64_t workspace_id;
} AndamentoDetail;
typedef struct {
    AndamentoText intent, label;
    AndamentoEntity entity;
    size_t action;
} AndamentoDetailAction;
/* action index 0 = primary, 1 = copy-url (0 return when unavailable).
 * Primary intent: open-url, focus-workspace, materialize-workspace or inspect.
 * Presentation labels come from workspace action.primary.label when supplied.
 * Dispatch remains snapshot validated and resolves current controls.
 */
uint32_t andamento_snapshot_detail_action(const AndamentoSnapshot *, size_t detail, size_t index, AndamentoDetailAction *out);
typedef struct {
    AndamentoText name, section;
    uint32_t role;
    AndamentoText label;
    uint32_t has_value;
    AndamentoText text, source_key, source_id;
    uint32_t has_observation;
    uint64_t observed_at_ms;
    uint32_t has_ttl;
    uint64_t ttl_ms;
    uint32_t stale;
    size_t relation_count;
} AndamentoDetailField;
typedef struct {
    AndamentoEntity entity;
    AndamentoText display_text;
    size_t detail;
} AndamentoDetailRelation;
/* Add exact-identity details on demand to a current plain snapshot. Text and
 * action indices remain snapshot-owned and stable across later requests.
 * ANDAMENTO_NONE without error means missing. New requests on stale snapshots
 * fail; existing details remain readable. Each identity is appended once.
 * Missing results are not cached per snapshot (the core bounds revision misses).
 * The snapshot clock is fixed at acquisition, including for later appended cards.
 * Not thread-safe with concurrent reads/requests/release of this snapshot. */
size_t andamento_snapshot_detail_request(Andamento *, AndamentoSnapshot *, AndamentoText kind, AndamentoText id, char **error);
size_t andamento_snapshot_detail_count(const AndamentoSnapshot *);
size_t andamento_snapshot_detail_find(const AndamentoSnapshot *, AndamentoText kind, AndamentoText id);
uint32_t andamento_snapshot_detail(const AndamentoSnapshot *, size_t index, AndamentoDetail *out);
/* ABI 3: the detail's Workspace ID; returns 0 when has_workspace is 0. */
uint32_t andamento_snapshot_detail_workspace(const AndamentoSnapshot *, size_t index, AndamentoWorkspaceId *out);
uint32_t andamento_snapshot_detail_field(const AndamentoSnapshot *, size_t detail, size_t field, AndamentoDetailField *out);
uint32_t andamento_snapshot_detail_relation(const AndamentoSnapshot *, size_t detail, size_t field,
    size_t relation, const AndamentoEntity *path, size_t path_count, AndamentoDetailRelation *out);
/* ABI 3: request and find name the entity's provider (the ABI 2 calls use the
 * default provider); detail_provider reads a card's entity provider. */
size_t andamento_snapshot_detail_request3(Andamento *, AndamentoSnapshot *, AndamentoEntity3 entity, char **error);
size_t andamento_snapshot_detail_find3(const AndamentoSnapshot *, AndamentoEntity3 entity);
uint32_t andamento_snapshot_detail_provider(const AndamentoSnapshot *, size_t index, AndamentoText *out);

/* Action references belong to one snapshot. Another client or a snapshot from
 * before a revision-changing mutation is rejected. Dispatch captured clicks against
 * their displayed snapshot BEFORE draining queued facts/ticks/topology. If an
 * update already intervened, discard the stale click and redraw. Only a NEW
 * interaction may be hit-tested against the new frame: never replay old
 * coordinates or reinterpret an old action index against a new snapshot.
 * Dispatch queues effects; it does not execute host operations. */
uint32_t andamento_dispatch(Andamento *, const AndamentoSnapshot *, size_t action, char **error_out);
void andamento_snapshot_release(AndamentoSnapshot *);

/* Take drains the effect queue exactly once, independently of snapshots.
 * An empty queue returns a valid empty batch. The batch owns its text until
 * release and survives sidebar destruction. Copy fields needed asynchronously
 * or retain the batch. Releasing it does NOT complete its requests. */
AndamentoEffects *andamento_effects_take(Andamento *, char **error_out);
enum { ANDAMENTO_EFFECT_FOCUS, ANDAMENTO_EFFECT_MATERIALIZE, ANDAMENTO_EFFECT_INSPECT,
       ANDAMENTO_EFFECT_OPEN_URL, ANDAMENTO_EFFECT_COPY_URL };
typedef struct {
    uint32_t kind;
    uint64_t request_id, workspace_id;
    AndamentoText entity_kind, entity_id, name, recipe;
    uint32_t has_cwd;
    AndamentoText cwd;
} AndamentoEffect;
size_t andamento_effects_count(const AndamentoEffects *);
uint32_t andamento_effects_get(const AndamentoEffects *, size_t index, AndamentoEffect *out);
/* ABI 3: a FOCUS effect's Workspace ID; returns 0 for other effects. */
uint32_t andamento_effects_workspace(const AndamentoEffects *, size_t index, AndamentoWorkspaceId *out);
/* ABI 3: a MATERIALIZE or INSPECT effect's entity provider; 0 for others. */
uint32_t andamento_effects_provider(const AndamentoEffects *, size_t index, AndamentoText *out);
/* URL effects carry the resolved URL in recipe; no completion is required.
 * Activation opens a subject URL. Copying is an additive ABI 2 action. */
/* Subject rows also expose a copy action through their retained snapshot. */
size_t andamento_snapshot_copy_url_action(const AndamentoSnapshot *, size_t node_index);
uint32_t andamento_copy_subject_url(Andamento *, AndamentoText kind, AndamentoText id, char **error_out);
uint32_t andamento_copy_subject_url3(Andamento *, AndamentoEntity3 entity, char **error_out); /* ABI 3 */
/* Managed-content target a MATERIALIZE effect's recipe resolves; additive to
 * ABI 2 (AndamentoEffect is unchanged). Returns 0 when there is none. Record it
 * as the applied target so the first content plan sees the new content as current. */
uint32_t andamento_effects_primary_target(const AndamentoEffects *, size_t index, AndamentoText *out);
void andamento_effects_release(AndamentoEffects *);
void andamento_string_free(char *);
void andamento_destroy(Andamento *);
/* Optional managed-primary content reconciliation; additive to ABI 2. It
 * plans the "primary" slot (see Slots below): these calls and the slot calls
 * for "primary" share one binding and its tokens.
 * Only entities declaring workspace.primary.state opt in. Missing/expired facts
 * suspend updates; they never delete content. Plans own borrowed text.
 * Plan using the host's actual persisted descriptor. Validate immediately before
 * committing on the single owner thread, then complete. Repeated plans return
 * the same token until desired/binding state changes. Failures require retry or
 * a new desired descriptor. Topology observation forgets closed workspaces.
 */
typedef struct AndamentoContentPlan AndamentoContentPlan;
enum { ANDAMENTO_CONTENT_UNAVAILABLE, ANDAMENTO_CONTENT_HELD, ANDAMENTO_CONTENT_CURRENT,
       ANDAMENTO_CONTENT_UPDATING, ANDAMENTO_CONTENT_FAILED };
typedef struct {
  uint32_t state;
  uint64_t token;
  AndamentoText target, command;
  uint32_t has_cwd;
  AndamentoText cwd;
} AndamentoContent;
AndamentoContentPlan *andamento_content_plan(Andamento *, uint64_t workspace_id,
    AndamentoText entity_kind, AndamentoText entity_id, AndamentoText applied_target,
    AndamentoText applied_command, uint32_t has_cwd, AndamentoText applied_cwd, char **error_out);
uint32_t andamento_content_get(const AndamentoContentPlan *, AndamentoContent *out);
uint32_t andamento_content_valid(Andamento *, uint64_t workspace_id, uint64_t token, char **error_out);
uint32_t andamento_content_complete(Andamento *, uint64_t workspace_id, uint64_t token, uint32_t success, char **error_out);
uint32_t andamento_content_retry(Andamento *, uint64_t workspace_id, char **error_out);
/* ABI 3: the same calls with 128-bit IDs; release and get are shared. */
AndamentoContentPlan *andamento_content_plan3(Andamento *, AndamentoWorkspaceId workspace_id,
    AndamentoText entity_kind, AndamentoText entity_id, AndamentoText applied_target,
    AndamentoText applied_command, uint32_t has_cwd, AndamentoText applied_cwd, char **error_out);
uint32_t andamento_content_valid3(Andamento *, AndamentoWorkspaceId workspace_id, uint64_t token, char **error_out);
uint32_t andamento_content_complete3(Andamento *, AndamentoWorkspaceId workspace_id, uint64_t token, uint32_t success, char **error_out);
uint32_t andamento_content_retry3(Andamento *, AndamentoWorkspaceId workspace_id, char **error_out);
/* ABI 3: as andamento_content_plan3, naming the entity's provider. */
AndamentoContentPlan *andamento_content_plan_entity(Andamento *, AndamentoWorkspaceId workspace_id,
    AndamentoEntity3 entity, AndamentoText applied_target, AndamentoText applied_command,
    uint32_t has_cwd, AndamentoText applied_cwd, char **error_out);
void andamento_content_release(AndamentoContentPlan *);

/* ABI 3: Slots and arrangement documents (docs/sidebar-design/
 * slots-and-arrangements.md). Every call names a registered workspace.
 *
 * Slots. A workspace's slots are its subject's Suggested Layout slots (a
 * cached baseline: a malformed or expired layout keeps it, and a new valid
 * layout replaces it), then detached slots the baseline has since dropped,
 * then the slots the host adds, in that order, which is also the default
 * placement order. A host adds the user's own slots with keys "u:<id>",
 * <id> being 1 to 64 of [a-z0-9_-]; provider keys never contain ':'.
 * Setting a baseline slot's key overrides its View Spec and rebind policy,
 * which detaches it from the provider; setting the baseline's own spec and
 * policy, or reattach, drops the override. remove deletes the user's own
 * slot or a detached slot the baseline dropped (a baseline slot is the
 * provider's to remove: an error); removing an unknown key is harmless. The
 * managed primary content of an entity publishing workspace.primary.* is the
 * "primary" slot, and the andamento_content_* calls plan that slot.
 *
 * A View Spec's content is a provider FACET (entity and facet; an empty
 * provider is the default provider) or a local recipe: COMMAND (argv when
 * argc is nonzero, else the shell line in command; optional cwd), FILE
 * (path), URL (url) or JACKSTAY (launcher and endpoint). Fields other kinds
 * don't use are ignored on input and empty on output. presentation is an
 * open vocabulary ("terminal", "web", "markdown", ...): a frontend that can't
 * render the content shows a placeholder and never drops the slot. Rebind:
 * REPLACE closes the instance a rebind replaces; KEEP_PREVIOUS keeps it
 * reachable, reported as previous until release_previous; ASK asks the user
 * first, and declining is completing the update unsuccessfully (FAILED, not
 * retried until the resolution changes or retry).
 *
 * slots_acquire returns an owned list; its text and argv arrays are valid
 * until slots_release, which, like every release, accepts NULL. */
enum { ANDAMENTO_SLOT_COMMAND, ANDAMENTO_SLOT_FILE, ANDAMENTO_SLOT_URL,
       ANDAMENTO_SLOT_JACKSTAY, ANDAMENTO_SLOT_FACET };
enum { ANDAMENTO_REBIND_REPLACE, ANDAMENTO_REBIND_KEEP_PREVIOUS, ANDAMENTO_REBIND_ASK };
typedef struct {
    uint32_t content;
    AndamentoEntity3 entity;
    AndamentoText facet;
    AndamentoText command;
    const AndamentoText *argv;
    size_t argc;
    uint32_t has_cwd;
    AndamentoText cwd;
    AndamentoText path, url, launcher, endpoint;
    uint32_t has_presentation;
    AndamentoText presentation;
} AndamentoViewSpec;
typedef struct {
    AndamentoText key;
    AndamentoViewSpec spec;
    uint32_t rebind;
    /* in_baseline: the current baseline has this key. detached: the user
     * overrode it, so it no longer follows the provider. */
    uint32_t in_baseline, detached;
} AndamentoSlot;
typedef struct AndamentoSlots AndamentoSlots;
AndamentoSlots *andamento_slots_acquire(Andamento *, AndamentoWorkspaceId, char **error_out);
size_t andamento_slots_count(const AndamentoSlots *);
uint32_t andamento_slots_get(const AndamentoSlots *, size_t index, AndamentoSlot *out);
void andamento_slots_release(AndamentoSlots *);
uint32_t andamento_slot_set(Andamento *, AndamentoWorkspaceId, AndamentoText key,
    const AndamentoViewSpec *, uint32_t rebind, char **error_out);
uint32_t andamento_slot_remove(Andamento *, AndamentoWorkspaceId, AndamentoText key, char **error_out);
uint32_t andamento_slot_reattach(Andamento *, AndamentoWorkspaceId, AndamentoText key, char **error_out);

/* ABI 3: each slot follows managed primary content's protocol: plan with the
 * resolution identity the host applied (empty for none), prepare the runtime
 * instance without touching the current one, validate the token immediately
 * before committing on the owner thread, then complete. state is an
 * ANDAMENTO_CONTENT_* value. While UPDATING, token, resolution (an opaque
 * identity: record it and pass it back as applied), target (the backing
 * instance, when the provider names one) and recipe (what to run; its
 * presentation is unset) describe the update. A provider facet resolves
 * from the subject's layout while the slot follows it, else from the facet
 * entity's own layout slot of that name ("primary": its workspace.primary.*
 * facts); a local recipe resolves to itself. previous: the resolution a
 * KEEP_PREVIOUS rebind replaced, until release_previous (which returns 1 if
 * there was one). Plans own their text until release. Topology observation
 * forgets closed workspaces' bindings; removing a slot forgets its own. */
typedef struct AndamentoSlotPlan AndamentoSlotPlan;
typedef struct {
    uint32_t state;
    uint64_t token;
    AndamentoText resolution;
    uint32_t has_target;
    AndamentoText target;
    AndamentoViewSpec recipe;
    uint32_t rebind;
    uint32_t has_previous;
    AndamentoText previous;
} AndamentoSlotContent;
AndamentoSlotPlan *andamento_slot_plan(Andamento *, AndamentoWorkspaceId, AndamentoText key,
    AndamentoText applied, char **error_out);
uint32_t andamento_slot_plan_get(const AndamentoSlotPlan *, AndamentoSlotContent *out);
void andamento_slot_plan_release(AndamentoSlotPlan *);
uint32_t andamento_slot_valid(Andamento *, AndamentoWorkspaceId, AndamentoText key, uint64_t token, char **error_out);
uint32_t andamento_slot_complete(Andamento *, AndamentoWorkspaceId, AndamentoText key, uint64_t token,
    uint32_t success, char **error_out);
uint32_t andamento_slot_retry(Andamento *, AndamentoWorkspaceId, AndamentoText key, char **error_out);
uint32_t andamento_slot_release_previous(Andamento *, AndamentoWorkspaceId, AndamentoText key, char **error_out);

/* ABI 3: a workspace's arrangement is one document the host commits whole at
 * the end of a gesture: a tree of panels with stable, unique, nonempty IDs.
 * Panels are a preorder array: the first is the root (parent NONE), every
 * other names an earlier SPLIT as its parent, and a split's children are the
 * panels naming it, in array order. A SPLIT lays its children out along axis
 * (ROW: left to right; COLUMN: top to bottom) and has no tabs. A TABS panel's
 * tabs are tabs[first_tab .. first_tab + tab_count), each naming a slot key;
 * selected indexes them, or is NONE. weight is relative to siblings, finite
 * and positive (the root's is kept but unused). No panels: an empty document.
 *
 * set_arrangement stores the document if expected_generation is the current
 * generation (0 before any): it returns COMMITTED, or STALE with no error and
 * no change (read the arrangement again), or INVALID (0) with an error and
 * no change. generation_out, when not NULL, receives the generation after
 * the call. The document is validated against the slot set: each slot has at
 * most one tab, and a tab names a slot, or a slot the stored document already
 * tabbed that has since gone. Then it is reconciled: each slot with no tab is
 * appended to the first TABS panel in preorder (selected there if that panel
 * selects nothing), and a tab whose slot has gone is kept and reported gone.
 * Committing the stored document again keeps its generation. Andamento also
 * reconciles when the baseline changes, and until the host first commits,
 * the document is the baseline's arrangement hint; either changes the
 * generation, so the host's next commit at the old one is STALE.
 *
 * Neither committing an arrangement nor any slot call changes the sidebar's
 * revision: snapshots stay current (andamento_snapshot_is_current). They
 * change andamento_workspace_content_revision instead, as do resolution
 * changes; poll it to learn when to re-read slots, plans and arrangements.
 *
 * arrangement_acquire returns an owned copy in the same form: tabs carry
 * placed (Andamento placed it since the host last committed) and gone (its
 * slot has gone); both are ignored on input. Its text is valid until release. */
enum { ANDAMENTO_PANEL_SPLIT, ANDAMENTO_PANEL_TABS };
enum { ANDAMENTO_AXIS_ROW, ANDAMENTO_AXIS_COLUMN };
enum { ANDAMENTO_ARRANGEMENT_INVALID, ANDAMENTO_ARRANGEMENT_COMMITTED, ANDAMENTO_ARRANGEMENT_STALE };
typedef struct {
    size_t parent;
    AndamentoText id;
    double weight;
    uint32_t kind, axis;
    size_t first_tab, tab_count, selected;
} AndamentoPanel;
typedef struct { AndamentoText slot; uint32_t placed, gone; } AndamentoTab;
typedef struct AndamentoArrangement AndamentoArrangement;
typedef struct { uint64_t generation; uint32_t owned; size_t panel_count, tab_count; } AndamentoArrangementInfo;
uint32_t andamento_set_arrangement(Andamento *, AndamentoWorkspaceId,
    const AndamentoPanel *panels, size_t panel_count, const AndamentoTab *tabs, size_t tab_count,
    uint64_t expected_generation, uint64_t *generation_out, char **error_out);
AndamentoArrangement *andamento_arrangement_acquire(Andamento *, AndamentoWorkspaceId, char **error_out);
/* owned: the host has committed it; until then it follows the baseline's hint. */
uint32_t andamento_arrangement_info(const AndamentoArrangement *, AndamentoArrangementInfo *out);
uint32_t andamento_arrangement_panel(const AndamentoArrangement *, size_t index, AndamentoPanel *out);
uint32_t andamento_arrangement_tab(const AndamentoArrangement *, size_t index, AndamentoTab *out);
void andamento_arrangement_release(AndamentoArrangement *);
uint64_t andamento_workspace_content_revision(Andamento *, char **error_out);

/* ABI 3: the Dashboard's sidebar arrangement (docs/sidebar-design/
 * sidebar-arrangement.md): which sections are docked where, as one document
 * in the same form as a workspace's arrangement, stored in the dashboard
 * record. Tabs hold section keys: a region's name, ".section:<id>" for a
 * local section (in the region whose placement has a layout="section" loop),
 * ".unplaced" for the workspace fallback (always declared; hide it while it
 * has no rows), or "u:<id>" for a View of the host's own, which Andamento
 * stores but never places or flags. Pixel geometry and section collapse stay
 * with the host.
 *
 * The panel array is the dock, then the floating panels: panels before
 * floating_first are the dock's one tree (none when floating_first is 0),
 * and from floating_first on, each panel whose parent is NONE starts a
 * floating panel (its position and size are the host's). Parents never
 * cross that boundary. A key may have two tabs: the later ones are removed.
 *
 * set_sidebar_arrangement commits the host's whole document at the expected
 * generation, as set_arrangement does (COMMITTED, STALE or INVALID), at the
 * end of a gesture. A tab names a declared or local section, a host View, or
 * a key the stored document already holds. Every declared section the
 * document leaves out is closed: the host closes a section by committing
 * without it, and the first commit adopts a layout the host saved before
 * Andamento stored one. A closed section it tabs again is restored.
 *
 * Andamento reconciles the document when what is declared changes: on
 * configure and when local sections or groups change or are imported. A
 * section that has neither a tab nor a closed record is placed in a new tab
 * panel of its default-host ("floating", else the dock), before the subtree
 * holding the next section in default order that is placed there, else last.
 * Default order: pinned regions first, then by order, an omitted order being
 * the declaration index (an unhinted ".unplaced" or default local section
 * last); ties keep declaration order. Placed sections keep their place when
 * hints change. Keys that no longer resolve, tabbed or closed, are kept and
 * flagged (the tab's gone flag, an UNRESOLVED note), never dropped: they are
 * where they were if they resolve again, and go when the host commits
 * without them. Each change moves the generation; poll it (cheap) to learn
 * when to read the arrangement again. None of these calls changes the
 * snapshot's revision.
 *
 * restore_section reopens a closed (or never placed) declared section by its
 * hints, with an equal share of the dock, scaling the others' weights to make
 * room; a section with a tab is left alone, an undeclared one is INVALID.
 * reset drops the user's arrangement, closed sections and flagged keys, and
 * places every declared section by its hints.
 *
 * sidebar_arrangement_acquire returns the document through the arrangement
 * getters (owned: the host has committed since the last reset; a tab's
 * placed: Andamento placed it since the host last committed; gone: its key
 * no longer resolves), with floating_first, and notes: every CLOSED section,
 * every UNRESOLVED key, and what the last reconciliation or call did
 * (PLACED, RESTORED, DUPLICATE: a key whose later tabs it removed). For a
 * workspace's arrangement floating_first is panel_count and there are no
 * notes. */
enum { ANDAMENTO_SECTION_CLOSED, ANDAMENTO_SECTION_PLACED, ANDAMENTO_SECTION_RESTORED,
       ANDAMENTO_SECTION_DUPLICATE, ANDAMENTO_SECTION_UNRESOLVED };
typedef struct { uint32_t kind; AndamentoText key; } AndamentoSectionNote;
uint32_t andamento_set_sidebar_arrangement(Andamento *,
    const AndamentoPanel *panels, size_t panel_count, size_t floating_first,
    const AndamentoTab *tabs, size_t tab_count,
    uint64_t expected_generation, uint64_t *generation_out, char **error_out);
uint32_t andamento_sidebar_restore_section(Andamento *, AndamentoText key,
    uint64_t expected_generation, uint64_t *generation_out, char **error_out);
uint32_t andamento_sidebar_reset(Andamento *, uint64_t expected_generation,
    uint64_t *generation_out, char **error_out);
uint64_t andamento_sidebar_arrangement_generation(Andamento *, char **error_out);
AndamentoArrangement *andamento_sidebar_arrangement_acquire(Andamento *, char **error_out);
size_t andamento_arrangement_floating_first(const AndamentoArrangement *);
size_t andamento_arrangement_note_count(const AndamentoArrangement *);
uint32_t andamento_arrangement_note(const AndamentoArrangement *, size_t index, AndamentoSectionNote *out);

#ifdef __cplusplus
}
#endif
#endif
