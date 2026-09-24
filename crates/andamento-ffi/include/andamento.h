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

/* ABI 2 replaces the experimental JSON request ABI; no compatibility promise
 * with ABI 1. Tags have uint32_t storage; do not use C enum size assumptions.
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
/* Monotonic milliseconds scoped to this client. Tick advances expiry without
 * facts. Drain facts/topology/completions before acquiring a render snapshot. */
uint32_t andamento_tick(Andamento *, uint64_t now_ms, char **error_out);

typedef struct { uint64_t id; size_t position; AndamentoText name; uint32_t selected; } AndamentoWorkspace;
enum { ANDAMENTO_PANE_TERMINAL, ANDAMENTO_PANE_PLUGIN };
typedef struct { uint64_t workspace_id; uint32_t pane_id, kind, selectable, focused; int64_t ordinal; } AndamentoPane;
/* Full replacement of topology; workspace IDs are scoped to this client.
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
/* Snapshot-owned opaque loop invocation key. Compare for equality; do not parse.
 * Empty for section nodes. Additive ABI 2 API; AndamentoNode is unchanged. */
uint32_t andamento_snapshot_node_loop_key(const AndamentoSnapshot *, size_t index, AndamentoText *out);
uint32_t andamento_snapshot_field(const AndamentoSnapshot *, size_t index, AndamentoField *out);
uint32_t andamento_snapshot_control(const AndamentoSnapshot *, size_t index, AndamentoControl *out);
size_t andamento_snapshot_diagnostic_count(const AndamentoSnapshot *);
uint32_t andamento_snapshot_diagnostic(const AndamentoSnapshot *, size_t index, AndamentoText *out);
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
enum { ANDAMENTO_EFFECT_FOCUS, ANDAMENTO_EFFECT_MATERIALIZE, ANDAMENTO_EFFECT_INSPECT };
typedef struct {
    uint32_t kind;
    uint64_t request_id, workspace_id;
    AndamentoText entity_kind, entity_id, name, recipe;
    uint32_t has_cwd;
    AndamentoText cwd;
} AndamentoEffect;
size_t andamento_effects_count(const AndamentoEffects *);
uint32_t andamento_effects_get(const AndamentoEffects *, size_t index, AndamentoEffect *out);
void andamento_effects_release(AndamentoEffects *);
void andamento_string_free(char *);
void andamento_destroy(Andamento *);
/* Optional managed-primary content reconciliation; additive to ABI 2.
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
void andamento_content_release(AndamentoContentPlan *);

#ifdef __cplusplus
}
#endif
#endif
