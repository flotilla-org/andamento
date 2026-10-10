/* An actual native fixture consumer: no local request/snapshot JSON schema. */
#include "andamento.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define T(s) ((AndamentoText){(const uint8_t *)(s), sizeof(s)-1})
static char *error;
static void ok(uint32_t result) {
    if (!result) fprintf(stderr, "ABI error: %s\n", error ? error : "(none)");
    assert(result && error == NULL);
}
static int eq(AndamentoText a, const char *b) {
    return a.len == strlen(b) && memcmp(a.data, b, a.len) == 0;
}
static char *read_file(const char *path) {
    FILE *f = fopen(path, "rb"); assert(f);
    assert(fseek(f, 0, SEEK_END) == 0);
    long len = ftell(f); assert(len >= 0); rewind(f);
    char *data = malloc((size_t)len + 1); assert(data);
    assert(fread(data, 1, (size_t)len, f) == (size_t)len);
    data[len] = 0; fclose(f); return data;
}
static AndamentoSnapshot *snapshot(Andamento *h) {
    AndamentoSnapshot *s = andamento_snapshot_acquire(h, &error);
    assert(s && !error); return s;
}
static AndamentoNode find_node(AndamentoSnapshot *s, const char *kind) {
    AndamentoNode n;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, &n));
        if (!n.is_section && eq(n.entity_kind, kind)) return n;
    }
    assert(!"missing node"); return (AndamentoNode){0};
}
static AndamentoControl find_control(AndamentoSnapshot *s) {
    AndamentoNode n; AndamentoControl c;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, &n));
        for (size_t j = 0; j < n.control_count; ++j) {
            assert(andamento_snapshot_control(s, n.first_control+j, &c));
            if (c.kind == ANDAMENTO_CONTROL_DISPLAY_VARIABLE) return c;
        }
    }
    assert(!"missing control"); return (AndamentoControl){0};
}
static AndamentoEffects *take(Andamento *h) {
    AndamentoEffects *e = andamento_effects_take(h, &error); assert(e && !error); return e;
}
static void expected_error(uint32_t result) {
    assert(!result && error); andamento_string_free(error); error = NULL;
}
static void check_workdirs(void) {
    const char *config = "region \"tree\" root-template=\"title\" placement=\"tree\"\n"
        "template \"title\" { field \"label\" source=\"literal\" value=\"Test\"; }\n"
        "placement \"tree\" { for \"item\" kind=\"item\" { field \"label\" key=\"display.label\"; }; }\n";
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && !error);
    ok(andamento_apply_patch_json(h, 0, T("{\"target\":{\"kind\":\"entity\",\"value\":{\"kind\":\"item\",\"id\":\"i\"}},\"source_id\":\"test\",\"set\":{\"git.root\":{\"value\":{\"type\":\"text\",\"value\":\"/repo\"}},\"action.primary.recipe\":{\"value\":{\"type\":\"text\",\"value\":\"exec sh\"}}}}"), &error));
    AndamentoWorkspace ws = {7, 0, T("terminal"), 1};
    ok(andamento_observe(h, &ws, 1, NULL, 0, &error));
    AndamentoWorkdir dir = {7, T("/repo")};
    ok(andamento_observe_workdirs(h, &dir, 1, &error));
    AndamentoSnapshot *s = andamento_snapshot_acquire_details(h, &error);
    assert(s && !error);
    assert(find_node(s, "item").state == ANDAMENTO_LIVE);
    /* Structured cards retain live preview identity outside their field roles,
     * and dispatch exactly the same workspace control as the flat tree. */
    size_t detail = andamento_snapshot_detail_find(s, T("item"), T("i"));
    assert(detail != ANDAMENTO_NONE);
    AndamentoDetail card;
    assert(andamento_snapshot_detail(s, detail, &card));
    assert(card.has_workspace && card.workspace_id == 7);
    AndamentoDetailAction action;
    assert(andamento_snapshot_detail_action(s, detail, 0, &action));
    assert(eq(action.intent, "focus-workspace"));
    assert(eq(action.entity.kind, "item") && eq(action.entity.id, "i"));
    assert(action.action == card.activate);
    AndamentoDetailField field;
    assert(andamento_snapshot_detail_field(s, detail, 0, &field));
    assert(field.role == ANDAMENTO_DETAIL_TITLE);
    assert(field.has_value && eq(field.text, "i"));
    assert(andamento_snapshot_detail_count(s) == 1);
    andamento_snapshot_release(s);
    expected_error(andamento_observe_workdirs(h, NULL, 1, &error));
    ok(andamento_observe_workdirs(h, NULL, 0, &error));
    s = snapshot(h);
    assert(find_node(s, "item").state == ANDAMENTO_LATENT);
    andamento_snapshot_release(s);
    andamento_destroy(h);
}
/* A host-owned sibling order reorders one loop run by its opaque loop key,
 * leaving unnamed items in data order; an empty list restores data order. */
static void item_ids(AndamentoSnapshot *s, char *out) {
    AndamentoNode n; *out = 0;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, &n));
        if (!n.is_section && eq(n.entity_kind, "item")) strncat(out, (const char *)n.entity_id.data, n.entity_id.len);
    }
}
static void check_sibling_order(void) {
    const char *config = "region \"tree\" root-template=\"title\" placement=\"tree\"\n"
        "template \"title\" { field \"label\" source=\"literal\" value=\"Test\"; }\n"
        "placement \"tree\" { for \"item\" kind=\"item\" { field \"label\" key=\"display.label\"; }; }\n";
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && !error);
    const char *ids[] = {"a", "b", "c"};
    for (int i = 0; i < 3; ++i) {
        char patch[256];
        snprintf(patch, sizeof(patch), "{\"target\":{\"kind\":\"entity\",\"value\":{\"kind\":\"item\",\"id\":\"%s\"}},"
            "\"source_id\":\"test\",\"set\":{\"display.label\":{\"value\":{\"type\":\"text\",\"value\":\"%s\"}}}}", ids[i], ids[i]);
        ok(andamento_apply_patch_json(h, 0, (AndamentoText){(const uint8_t *)patch, strlen(patch)}, &error));
    }
    AndamentoSnapshot *s = snapshot(h);
    char order[16]; item_ids(s, order); assert(strcmp(order, "abc") == 0);
    AndamentoText loop; size_t index = 0;
    for (AndamentoNode n; andamento_snapshot_node(s, index, &n) && n.is_section; ++index) {}
    assert(andamento_snapshot_node_loop_key(s, index, &loop) && loop.len);
    char *key = malloc(loop.len); assert(key); memcpy(key, loop.data, loop.len);
    AndamentoText owned = {(const uint8_t *)key, loop.len};
    andamento_snapshot_release(s); /* the key text outlives its snapshot */
    /* b is unnamed, so it follows its data-order predecessor a. */
    AndamentoEntity c_first[] = {{T("item"), T("c")}, {T("item"), T("a")}, {T("item"), T("gone")}};
    ok(andamento_set_sibling_order(h, owned, c_first, 3, &error));
    s = snapshot(h); item_ids(s, order); assert(strcmp(order, "cab") == 0);
    andamento_snapshot_release(s);
    expected_error(andamento_set_sibling_order(h, T("not a loop key"), c_first, 1, &error));
    expected_error(andamento_set_sibling_order(h, owned, NULL, 1, &error));
    ok(andamento_set_sibling_order(h, owned, NULL, 0, &error));
    s = snapshot(h); item_ids(s, order); assert(strcmp(order, "abc") == 0);
    andamento_snapshot_release(s);
    free(key);
    andamento_destroy(h);
}
/* All six kinds, including a related issue absent from the visible tree,
 * expose typed roles and observation data through the actual C layout. */
static void check_typed_details(const char *path) {
    Andamento *h = andamento_create(NULL, 0, &error); assert(h && !error);
    char *patches = read_file(path);
    for (char *line = strtok(patches, "\n"); line; line = strtok(NULL, "\n"))
        ok(andamento_apply_patch_json(h, 42, (AndamentoText){(uint8_t *)line, strlen(line)}, &error));
    free(patches);
    AndamentoSnapshot *s = andamento_snapshot_acquire_details(h, &error);
    assert(s && !error);
    assert(andamento_snapshot_detail_count(s) == 6);
    const char *kinds[] = {"change_request", "issue", "convoy", "role", "project", "worktree"};
    for (size_t k = 0; k < 6; k++) {
        char id[80]; snprintf(id, sizeof(id), "%s:identity / #", kinds[k]);
        size_t index = andamento_snapshot_detail_find(s, (AndamentoText){(const uint8_t *)kinds[k], strlen(kinds[k])},
            (AndamentoText){(const uint8_t *)id, strlen(id)});
        assert(index != ANDAMENTO_NONE);
        AndamentoDetail d; assert(andamento_snapshot_detail(s, index, &d));
        assert(d.now_ms == 42 && d.error.len == 0 && d.error.data != NULL);
        unsigned roles = 0;
        for (size_t i = 0; i < d.field_count; i++) {
            AndamentoDetailField f; assert(andamento_snapshot_detail_field(s, index, i, &f));
            roles |= 1u << f.role;
            if (eq(f.name, "identity")) assert(f.role == ANDAMENTO_DETAIL_IDENTITY);
            if (eq(f.name, "label")) assert(f.role == ANDAMENTO_DETAIL_TITLE);
            if (eq(f.name, "state")) assert(f.role == ANDAMENTO_DETAIL_STATE);
            if (eq(f.name, "related")) assert(f.role == ANDAMENTO_DETAIL_RELATION);
            if (eq(f.name, "summary")) {
                assert(f.role == ANDAMENTO_DETAIL_FACT && eq(f.label, "Summary"));
                assert(f.has_value && !f.text.len && f.has_observation && f.observed_at_ms == 42);
                assert(f.has_ttl && f.ttl_ms == 100 && !f.stale && eq(f.source_id, "fixture-producer"));
            }
            if (k == 5 && eq(f.name, "branch")) assert(!f.has_value && !f.has_observation && f.text.data != NULL && f.source_id.data != NULL);
            if (k == 5 && eq(f.name, "related")) {
                AndamentoDetailRelation r;
                assert(andamento_snapshot_detail_relation(s, index, i, 0, NULL, 0, &r));
                assert(eq(r.entity.kind, "issue") && eq(r.entity.id, "issue:identity / #"));
                assert(eq(r.display_text, "Display issue") && r.detail != ANDAMENTO_NONE);
                AndamentoEntity navigation[] = {r.entity};
                assert(!andamento_snapshot_detail_relation(s, index, i, 0, navigation, 1, &r));
            }
        }
        assert(roles == 31);
    }
    /* Navigation reaches the catalog even without an issue placement. */
    for (size_t i = 0; i < andamento_snapshot_node_count(s); i++) {
        AndamentoNode n; assert(andamento_snapshot_node(s, i, &n));
        assert(!eq(n.entity_kind, "issue"));
    }
    andamento_snapshot_release(s); andamento_destroy(h);
}

/* Defaults cross the C ABI without changing AndamentoNode's layout. */
static void check_region_hints(void) {
    const char *config = "region \"hinted\" root-template=\"flotilla/region/tree\" default-host=\"sidebar\" order=-10\n"
                         "region \"legacy\" root-template=\"flotilla/region/tree\"\n";
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h);
    AndamentoSnapshot *s = andamento_snapshot_acquire(h, &error);
    AndamentoRegionHints hints;
    assert(andamento_snapshot_region_hints(s, 0, &hints));
    assert(eq(hints.default_host, "sidebar") && hints.has_order && hints.order == -10);
    assert(andamento_snapshot_region_hints(s, 1, &hints));
    assert(hints.default_host.len == 0 && hints.has_order == 0);
    assert(!andamento_snapshot_region_hints(s, ANDAMENTO_NONE, &hints));
    assert(!andamento_snapshot_region_hints(s, 0, NULL));
    /* With no unplaced workspaces the fallback section is still exposed, empty:
       hosts anchor workspace creation to its header. */
    unsigned empty_fallback = 0;
    size_t count = andamento_snapshot_node_count(s);
    for (size_t i = 0; i < count; i++) {
        AndamentoNode n; assert(andamento_snapshot_node(s, i, &n));
        if (n.is_section && eq(n.key, ".unplaced")) {
            empty_fallback++;
            for (size_t j = i+1; j < count; j++) {
                AndamentoNode child; assert(andamento_snapshot_node(s, j, &child));
                assert(child.parent != i);
            }
        }
    }
    assert(empty_fallback == 1);
    andamento_snapshot_release(s);
    /* Uncovered inventory becomes a synthetic unhinted section plus a node. */
    AndamentoWorkspace ws = {42, 0, T("unplaced"), 1};
    ok(andamento_observe(h, &ws, 1, NULL, 0, &error));
    s = andamento_snapshot_acquire(h, &error);
    unsigned synthetic = 0, entity = 0;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); i++) {
        AndamentoNode n; assert(andamento_snapshot_node(s, i, &n));
        if (!n.is_section) {
            assert(!andamento_snapshot_region_hints(s, i, &hints));
            entity++;
        } else if (eq(n.key, ".unplaced")) {
            assert(andamento_snapshot_region_hints(s, i, &hints));
            assert(hints.default_host.len == 0 && hints.has_order == 0);
            synthetic++;
        }
    }
    assert(synthetic && entity);
    andamento_snapshot_release(s); andamento_destroy(h);
}

/* ABI 3: host-supplied 128-bit Workspace IDs through every call that takes or
 * returns one. ABI 2 fields read 0 for wide IDs and the embedded n otherwise. */
static AndamentoWorkspaceId uuid(uint8_t tail) {
    AndamentoWorkspaceId id = {{0x01, 0x92, 0x0a, 0x6b, 0x7c, 0x3d, 0x7e, 0x4f,
                                0x8a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x6a, tail}};
    return id;
}
static int same(AndamentoWorkspaceId a, AndamentoWorkspaceId b) {
    return memcmp(a.bytes, b.bytes, sizeof a.bytes) == 0;
}
static size_t node_index(AndamentoSnapshot *s, const char *kind) {
    AndamentoNode n;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, &n));
        if (!n.is_section && eq(n.entity_kind, kind)) return i;
    }
    assert(!"missing node"); return ANDAMENTO_NONE;
}
static void check_abi3(const char *config_path, const char *patches_path) {
    char *config = read_file(config_path);
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && !error); free(config);
    char *patches = read_file(patches_path);
    for (char *line = strtok(patches, "\n"); line; line = strtok(NULL, "\n"))
        ok(andamento_apply_patch_json(h, 100, (AndamentoText){(uint8_t *)line, strlen(line)}, &error));
    free(patches);
    AndamentoWorkspaceId notes = uuid(0x7b), worker = uuid(0x7c), out;
    AndamentoWorkspaceId legacy = {{0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 42}};
    /* A workspace the user made: registered by the host, named by both patch forms. */
    ok(andamento_workspace_register(h, notes, &error));
    assert(andamento_workspace_registered(h, notes, &error) && !error);
    AndamentoWorkspace3 ws[] = {{notes, 0, T("Notes"), 1}, {legacy, 1, T("Legacy"), 0}};
    ok(andamento_observe3(h, ws, 2, NULL, 0, &error));
    ok(andamento_apply_patch_json(h, 100, T("{\"target\":{\"kind\":\"tab\",\"value\":\"01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b\"},"
        "\"source_id\":\"host\",\"set\":{\".host.kind\":{\"value\":{\"type\":\"text\",\"value\":\".workspace\"}}}}"), &error));
    AndamentoFact host_id = {.key=T(".host.id"), .kind=ANDAMENTO_FACT_TEXT, .text=T("notes")};
    ok(andamento_apply_workspace(h, 100, notes, T("host"), &host_id, 1, &error));
    AndamentoSnapshot *s = snapshot(h);
    size_t i = node_index(s, ".workspace");
    AndamentoNode n; assert(andamento_snapshot_node(s, i, &n));
    assert(eq(n.entity_id, "notes") && n.state == ANDAMENTO_LIVE && n.workspace_id == 0);
    assert(andamento_snapshot_node_workspace(s, i, &out) && same(out, notes));
    assert(!andamento_snapshot_node_workspace(s, 0, &out)); /* a section */
    /* The embedded ID is ABI 2's 42. */
    assert(andamento_snapshot_node(s, i + 1, &n) && eq(n.entity_id, "42") && n.workspace_id == 42);
    /* Materialize, completing with the ID the host generated. */
    ok(andamento_dispatch(h, s, find_node(s, "vessel").activate, &error));
    andamento_snapshot_release(s);
    AndamentoEffects *effects = take(h);
    AndamentoEffect e; assert(andamento_effects_get(effects, 0, &e) && e.kind == ANDAMENTO_EFFECT_MATERIALIZE);
    assert(!andamento_effects_workspace(effects, 0, &out));
    ok(andamento_complete3(h, e.request_id, ANDAMENTO_COMPLETE_MATERIALIZE, worker, T(""), &error));
    andamento_effects_release(effects);
    assert(andamento_workspace_registered(h, worker, &error));
    AndamentoWorkspace3 all[] = {{notes, 0, T("Notes"), 0}, {worker, 1, T("Worker"), 1}};
    AndamentoPane3 pane = {worker, 7, ANDAMENTO_PANE_TERMINAL, 1, 1, 0};
    ok(andamento_observe3(h, all, 2, &pane, 1, &error));
    AndamentoWorkdir3 dir = {worker, T("/repo")};
    ok(andamento_observe_workdirs3(h, &dir, 1, &error));
    s = andamento_snapshot_acquire_details(h, &error); assert(s && !error);
    i = node_index(s, "vessel");
    assert(andamento_snapshot_node(s, i, &n) && n.state == ANDAMENTO_LIVE && n.workspace_id == 0);
    assert(andamento_snapshot_node_workspace(s, i, &out) && same(out, worker));
    size_t detail = andamento_snapshot_detail_find(s, T("vessel"), T("v"));
    AndamentoDetail card; assert(andamento_snapshot_detail(s, detail, &card));
    assert(card.has_workspace && card.workspace_id == 0);
    assert(andamento_snapshot_detail_workspace(s, detail, &out) && same(out, worker));
    ok(andamento_dispatch(h, s, n.activate, &error));
    andamento_snapshot_release(s);
    effects = take(h);
    assert(andamento_effects_get(effects, 0, &e) && e.kind == ANDAMENTO_EFFECT_FOCUS && e.workspace_id == 0);
    assert(andamento_effects_workspace(effects, 0, &out) && same(out, worker));
    ok(andamento_complete3(h, e.request_id, ANDAMENTO_COMPLETE_FOCUS, out, T(""), &error));
    andamento_effects_release(effects);
    /* Content reconciliation is keyed by the same IDs. */
    AndamentoContentPlan *plan = andamento_content_plan3(h, worker, T("vessel"), T("v"),
        T("target"), T("printf hello"), 0, T(""), &error);
    assert(plan && !error);
    AndamentoContent content; assert(andamento_content_get(plan, &content));
    assert(!andamento_content_valid3(h, worker, content.token + 1, &error) && !error);
    assert(!andamento_content_complete3(h, worker, content.token + 1, 1, &error) && !error);
    ok(andamento_content_retry3(h, worker, &error));
    andamento_content_release(plan);
    /* Deleting a workspace forgets it; forgetting twice is harmless. */
    ok(andamento_workspace_forget(h, notes, &error));
    ok(andamento_workspace_forget(h, notes, &error));
    assert(!andamento_workspace_registered(h, notes, &error) && !error);
    andamento_destroy(h);
}

/* ABI 3 records: export a sidebar's state and import it into a fresh one
 * before it observes anything, with direct display and local-entity setters. */
static int has(AndamentoBytes b, const char *needle) {
    size_t n = strlen(needle);
    for (size_t i = 0; i + n <= b.len; ++i)
        if (memcmp(b.data + i, needle, n) == 0) return 1;
    return 0;
}
static Andamento *fixture(const char *config_path, const char *patches_path) {
    char *config = read_file(config_path);
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && !error); free(config);
    if (patches_path) {
        char *patches = read_file(patches_path);
        for (char *line = strtok(patches, "\n"); line; line = strtok(NULL, "\n"))
            ok(andamento_apply_patch_json(h, 100, (AndamentoText){(uint8_t *)line, strlen(line)}, &error));
        free(patches);
    }
    return h;
}
static void check_records(const char *config_path, const char *patches_path) {
    Andamento *h = fixture(config_path, patches_path);
    AndamentoSnapshot *s = snapshot(h);
    assert(find_control(s).checked);
    andamento_snapshot_release(s);
    /* Display variables are set directly: no snapshot, no retry. */
    ok(andamento_set_display_variable(h, T("show-issues"), T("false"), &error));
    expected_error(andamento_set_display_variable(h, T("show-issues"), T("maybe"), &error));
    expected_error(andamento_set_display_variable(h, T("missing"), T("true"), &error));
    s = snapshot(h);
    assert(!find_control(s).checked);
    andamento_snapshot_release(s);
    /* Local sections, groups and pins are Andamento's. */
    AndamentoLocalFact label = {.key=T("display.label"), .kind=ANDAMENTO_FACT_TEXT, .text=T("Mine")};
    ok(andamento_local_set(h, T(".section"), T("s1"), &label, 1, &error));
    AndamentoLocalFact group[] = {
        {.key=T("display.label"), .kind=ANDAMENTO_FACT_TEXT, .text=T("Pinned")},
        {.key=T(".section"), .kind=ANDAMENTO_FACT_ENTITY, .entity={T(".section"), T("s1")}},
        {.key=T(".position"), .kind=ANDAMENTO_FACT_INTEGER, .integer=2},
    };
    ok(andamento_local_set(h, T(".group"), T("g1"), group, 3, &error));
    ok(andamento_local_set(h, T(".group"), T("gone"), NULL, 0, &error));
    ok(andamento_local_remove(h, T(".group"), T("gone"), &error));
    ok(andamento_local_remove(h, T(".group"), T("never"), &error));
    expected_error(andamento_local_set(h, T("vessel"), T("v"), &label, 1, &error));
    AndamentoLocalFact twice[] = {label, label};
    expected_error(andamento_local_set(h, T(".group"), T("g2"), twice, 2, &error));
    /* A workspace the user made gets a record. */
    AndamentoWorkspaceId notes = uuid(0x7b);
    ok(andamento_workspace_register(h, notes, &error));
    AndamentoWorkspace3 ws = {notes, 0, T("Notes"), 1};
    ok(andamento_observe3(h, &ws, 1, NULL, 0, &error));
    AndamentoBytes names = {0};
    ok(andamento_record_names(h, &names, &error));
    assert(names.len == strlen("dashboard\nworkspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b"));
    assert(memcmp(names.data, "dashboard\nworkspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b", names.len) == 0);
    andamento_bytes_free(names);
    AndamentoText workspace_name = T("workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b");
    uint64_t generation = andamento_record_generation(h, T("dashboard"), &error);
    assert(generation && !error);
    assert(andamento_record_generation(h, T("dashboard"), &error) == generation);
    ok(andamento_tick(h, 200, &error));
    assert(andamento_record_generation(h, T("dashboard"), &error) == generation);
    assert(andamento_record_generation(h, workspace_name, &error) && !error);
    assert(andamento_record_generation(h, T("workspace/9"), &error) == 0 && error);
    andamento_string_free(error); error = NULL;
    AndamentoBytes dashboard = {0}, workspace = {0}, again = {0};
    ok(andamento_record_export(h, T("dashboard"), &dashboard, &error));
    ok(andamento_record_export(h, workspace_name, &workspace, &error));
    assert(has(dashboard, "andamento-record \"dashboard\" version=5"));
    assert(has(dashboard, "display \"show-issues\" false"));
    assert(has(dashboard, "local \".group\" \"g1\" provider=\"local\""));
    assert(!has(dashboard, "\"gone\""));
    expected_error(andamento_record_export(h, T("nonsense"), &again, &error));
    expected_error(andamento_record_export(h, T("dashboard"), NULL, &error));
    assert(again.data == NULL);
    /* A fresh sidebar imports before observing anything or receiving a fact. */
    Andamento *fresh = fixture(config_path, NULL);
    ok(andamento_record_import(fresh, T("dashboard"), (AndamentoText){dashboard.data, dashboard.len}, &error));
    ok(andamento_record_import(fresh, workspace_name, (AndamentoText){workspace.data, workspace.len}, &error));
    assert(andamento_workspace_registered(fresh, notes, &error) && !error);
    ok(andamento_record_export(fresh, T("dashboard"), &again, &error));
    assert(again.len == dashboard.len && memcmp(again.data, dashboard.data, again.len) == 0);
    andamento_bytes_free(again);
    s = snapshot(fresh);
    assert(!find_control(s).checked);
    andamento_snapshot_release(s);
    /* Another version, or another record's name, changes nothing. */
    uint64_t imported = andamento_record_generation(fresh, T("dashboard"), &error);
    expected_error(andamento_record_import(fresh, T("dashboard"),
        T("andamento-record \"dashboard\" version=6"), &error));
    expected_error(andamento_record_import(fresh, T("dashboard"), (AndamentoText){workspace.data, workspace.len}, &error));
    assert(andamento_record_generation(fresh, T("dashboard"), &error) == imported);
    andamento_bytes_free(dashboard); andamento_bytes_free(workspace);
    andamento_bytes_free((AndamentoBytes){0});
    andamento_destroy(fresh); andamento_destroy(h);
}

/* The project node from provider, or a zeroed node. */
static AndamentoNode project_from(AndamentoSnapshot *s, const char *provider, uint32_t *stale) {
    AndamentoNode n; AndamentoText p;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, &n));
        if (n.is_section || !eq(n.entity_kind, "project")) continue;
        assert(andamento_snapshot_node_provider(s, i, &p, stale));
        if (eq(p, provider)) return n;
    }
    return (AndamentoNode){0};
}
static size_t count_kind(AndamentoSnapshot *s, const char *kind) {
    size_t count = 0; AndamentoNode n;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, &n));
        count += !n.is_section && eq(n.entity_kind, kind);
    }
    return count;
}
static void check_providers(const char *config_path) {
    Andamento *h = fixture(config_path, NULL);
    /* The same kind and ID from two subscriptions are two entities. */
    AndamentoFact project[] = {
        {.key=T("flotilla.project"), .kind=ANDAMENTO_FACT_TEXT, .text=T("p"),
         .has_ttl=1, .ttl_ms=50},
        {.key=T("display.label"), .kind=ANDAMENTO_FACT_TEXT, .text=T("One"),
         .has_ttl=1, .ttl_ms=50},
    };
    ok(andamento_apply_entity_from(h, 100, T("sub-1"), T("project"), T("p"), T("fixture"), project, 2, &error));
    project[1].text = T("Two");
    ok(andamento_apply_entity_from(h, 100, T("sub-2"), T("project"), T("p"), T("fixture"), project, 2, &error));
    expected_error(andamento_apply_entity_from(h, 100, T(""), T("project"), T("p"), T("fixture"), project, 2, &error));
    /* An ABI 2 call keeps working, under the default provider. */
    project[1].text = T("Local"); project[0].has_ttl = project[1].has_ttl = 0;
    ok(andamento_apply_entity(h, 100, T("project"), T("p"), T("fixture"), project, 2, &error));
    AndamentoSnapshot *s = snapshot(h);
    assert(count_kind(s, "project") == 3);
    uint32_t stale = 7;
    assert(eq(project_from(s, "sub-1", &stale).label, "One") && stale == 0);
    assert(eq(project_from(s, "sub-2", NULL).label, "Two"));
    assert(eq(project_from(s, "local", NULL).label, "Local"));
    AndamentoText key_one = project_from(s, "sub-1", NULL).key, key_two = project_from(s, "sub-2", NULL).key;
    assert(key_one.len != key_two.len || memcmp(key_one.data, key_two.data, key_one.len));
    AndamentoText provider;
    assert(!andamento_snapshot_node_provider(s, 0, &provider, NULL)); /* a section */
    andamento_snapshot_release(s);
    /* A stale provider's facts outlive their TTL and are flagged. */
    ok(andamento_provider_set_stale(h, T("sub-1"), 1, &error));
    ok(andamento_tick(h, 1000, &error));
    s = snapshot(h);
    assert(eq(project_from(s, "sub-1", &stale).label, "One") && stale == 1);
    assert(project_from(s, "sub-2", NULL).label.data == NULL); /* expired */
    andamento_snapshot_release(s);
    /* Fresh again, the lease restarts now. */
    ok(andamento_provider_set_stale(h, T("sub-1"), 0, &error));
    ok(andamento_tick(h, 1040, &error));
    s = snapshot(h);
    assert(eq(project_from(s, "sub-1", &stale).label, "One") && stale == 0);
    andamento_snapshot_release(s);
    /* Retraction removes a provider's facts in one call. */
    ok(andamento_provider_retract(h, T("sub-1"), &error));
    s = snapshot(h);
    assert(project_from(s, "sub-1", NULL).label.data == NULL);
    assert(eq(project_from(s, "local", NULL).label, "Local"));
    andamento_snapshot_release(s);
    /* A patch never names its own provider: the host's is stamped over it. */
    ok(andamento_apply_patch_json_from(h, 1040, T("sub-3"), T("{\"target\":{\"kind\":\"entity\",\"value\":{\"provider\":\"forged\",\"kind\":\"project\",\"id\":\"q\"}},\"source_id\":\"test\",\"set\":{\"display.label\":{\"value\":{\"type\":\"text\",\"value\":\"Q\"}}}}"), &error));
    /* The default provider is the host's to set. */
    expected_error(andamento_set_default_provider(h, T(""), &error));
    ok(andamento_set_default_provider(h, T("sub-4"), &error));
    project[1].text = T("Four");
    ok(andamento_apply_entity(h, 1040, T("project"), T("p"), T("fixture"), project, 2, &error));
    s = andamento_snapshot_acquire_details(h, &error); assert(s && !error);
    assert(eq(project_from(s, "sub-3", NULL).label, "Q"));
    assert(project_from(s, "forged", NULL).label.data == NULL);
    assert(eq(project_from(s, "sub-4", NULL).label, "Four"));
    /* Details: ABI 2 finds by the default provider, ABI 3 by any. */
    size_t four = andamento_snapshot_detail_find(s, T("project"), T("p"));
    assert(four != ANDAMENTO_NONE);
    assert(andamento_snapshot_detail_provider(s, four, &provider) && eq(provider, "sub-4"));
    size_t local = andamento_snapshot_detail_find3(s, (AndamentoEntity3){T("local"), T("project"), T("p")});
    assert(local != ANDAMENTO_NONE && local != four);
    assert(andamento_snapshot_detail_find3(s, (AndamentoEntity3){T("sub-1"), T("project"), T("p")}) == ANDAMENTO_NONE);
    andamento_snapshot_release(s);
    andamento_destroy(h);
}

/* ABI 3 slots and arrangement documents: the host adds slots, plans their
 * content, and commits whole arrangements without invalidating the snapshot. */
static void check_slots(const char *config_path, const char *patches_path) {
    Andamento *h = fixture(config_path, patches_path);
    AndamentoWorkspaceId ws = uuid(0x7d);
    ok(andamento_workspace_register(h, ws, &error));
    AndamentoWorkspace3 open = {ws, 0, T("Scratch"), 1};
    ok(andamento_observe3(h, &open, 1, NULL, 0, &error));
    AndamentoSnapshot *s = snapshot(h);
    uint64_t content_revision = andamento_workspace_content_revision(h, &error);
    assert(!error);
    /* Slots the user adds are in their own namespace. */
    AndamentoText argv[] = {T("htop"), T("-d")};
    AndamentoViewSpec spec = {.content = ANDAMENTO_SLOT_COMMAND, .argv = argv, .argc = 2,
                              .has_cwd = 1, .cwd = T("/tmp"),
                              .has_presentation = 1, .presentation = T("terminal")};
    ok(andamento_slot_set(h, ws, T("u:1"), &spec, ANDAMENTO_REBIND_KEEP_PREVIOUS, &error));
    AndamentoViewSpec url = {.content = ANDAMENTO_SLOT_URL, .url = T("https://example.com")};
    ok(andamento_slot_set(h, ws, T("u:2"), &url, ANDAMENTO_REBIND_REPLACE, &error));
    AndamentoViewSpec facet = {.content = ANDAMENTO_SLOT_FACET,
                               .entity = {T(""), T("vessel"), T("v")}, .facet = T("primary")};
    ok(andamento_slot_set(h, ws, T("u:3"), &facet, ANDAMENTO_REBIND_ASK, &error));
    expected_error(andamento_slot_set(h, ws, T("nope"), &url, ANDAMENTO_REBIND_REPLACE, &error));
    expected_error(andamento_slot_set(h, ws, T("u:4"), &url, 7, &error));
    expected_error(andamento_slot_set(h, uuid(0x01), T("u:1"), &url, ANDAMENTO_REBIND_REPLACE, &error));
    assert(andamento_workspace_content_revision(h, &error) > content_revision);
    AndamentoSlots *slots = andamento_slots_acquire(h, ws, &error);
    assert(slots && !error && andamento_slots_count(slots) == 3);
    AndamentoSlot slot;
    assert(andamento_slots_get(slots, 0, &slot) && eq(slot.key, "u:1"));
    assert(slot.rebind == ANDAMENTO_REBIND_KEEP_PREVIOUS && !slot.in_baseline && !slot.detached);
    assert(slot.spec.content == ANDAMENTO_SLOT_COMMAND && slot.spec.argc == 2);
    assert(eq(slot.spec.argv[1], "-d") && slot.spec.has_cwd && eq(slot.spec.cwd, "/tmp"));
    assert(slot.spec.has_presentation && eq(slot.spec.presentation, "terminal"));
    assert(andamento_slots_get(slots, 2, &slot) && slot.spec.content == ANDAMENTO_SLOT_FACET);
    assert(eq(slot.spec.entity.provider, "local") && eq(slot.spec.facet, "primary"));
    assert(!andamento_slots_get(slots, 3, &slot));
    /* A local recipe resolves to itself: plan, commit, acknowledge. */
    AndamentoSlotPlan *plan = andamento_slot_plan(h, ws, T("u:1"), T(""), &error);
    assert(plan && !error);
    AndamentoSlotContent content; assert(andamento_slot_plan_get(plan, &content));
    assert(content.state == ANDAMENTO_CONTENT_UPDATING && !content.has_target);
    assert(content.recipe.content == ANDAMENTO_SLOT_COMMAND && content.recipe.argc == 2);
    assert(eq(content.recipe.argv[0], "htop"));
    assert(content.rebind == ANDAMENTO_REBIND_KEEP_PREVIOUS && !content.has_previous);
    assert(andamento_slot_valid(h, ws, T("u:1"), content.token, &error));
    assert(andamento_slot_complete(h, ws, T("u:1"), content.token, 1, &error));
    AndamentoSlotPlan *current = andamento_slot_plan(h, ws, T("u:1"), content.resolution, &error);
    assert(current && !error);
    AndamentoSlotContent now; assert(andamento_slot_plan_get(current, &now));
    assert(now.state == ANDAMENTO_CONTENT_CURRENT && now.token == 0);
    andamento_slot_plan_release(current);
    andamento_slot_plan_release(plan);
    assert(!andamento_slot_release_previous(h, ws, T("u:1"), &error) && !error);
    ok(andamento_slot_retry(h, ws, T("u:1"), &error));
    assert(!andamento_slot_plan(h, ws, T("u:9"), T(""), &error));
    andamento_string_free(error); error = NULL;
    /* Commit an arrangement leaving u:3 out: it is placed in the first tab panel. */
    AndamentoTab tabs[] = {{T("u:1"), 0, 0}, {T("u:2"), 0, 0}};
    AndamentoPanel panels[] = {
        {ANDAMENTO_NONE, T("root"), 1.0, ANDAMENTO_PANEL_SPLIT, ANDAMENTO_AXIS_ROW, 0, 0, ANDAMENTO_NONE},
        {0, T("1"), 0.6, ANDAMENTO_PANEL_TABS, 0, 0, 2, 1},
        {0, T("2"), 0.4, ANDAMENTO_PANEL_TABS, 0, 0, 0, ANDAMENTO_NONE},
    };
    uint64_t generation = 99;
    assert(andamento_set_arrangement(h, ws, panels, 3, tabs, 2, 0, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_COMMITTED && !error && generation == 1);
    /* The sidebar's snapshot is still current. */
    ok(andamento_snapshot_is_current(h, s, &error));
    /* A stale generation is rejected without effect, and without an error. */
    generation = 99;
    assert(andamento_set_arrangement(h, ws, panels, 1, tabs, 0, 0, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_STALE && !error && generation == 1);
    /* So is a tab for a slot that doesn't exist, or a malformed panel tree. */
    AndamentoTab unknown = {T("u:9"), 0, 0};
    AndamentoPanel lone = {ANDAMENTO_NONE, T("1"), 1.0, ANDAMENTO_PANEL_TABS, 0, 0, 1, 0};
    expected_error(andamento_set_arrangement(h, ws, &lone, 1, &unknown, 1, 1, &generation, &error));
    AndamentoPanel orphan[] = {panels[0], panels[1]};
    orphan[1].parent = ANDAMENTO_NONE;
    expected_error(andamento_set_arrangement(h, ws, orphan, 2, tabs, 2, 1, &generation, &error));
    AndamentoArrangement *a = andamento_arrangement_acquire(h, ws, &error);
    assert(a && !error);
    AndamentoArrangementInfo info; assert(andamento_arrangement_info(a, &info));
    assert(info.generation == 1 && info.owned && info.panel_count == 3 && info.tab_count == 3);
    AndamentoPanel panel; assert(andamento_arrangement_panel(a, 1, &panel));
    assert(panel.parent == 0 && eq(panel.id, "1") && panel.weight == 0.6);
    assert(panel.kind == ANDAMENTO_PANEL_TABS && panel.tab_count == 3 && panel.selected == 1);
    AndamentoTab tab; assert(andamento_arrangement_tab(a, 2, &tab));
    assert(eq(tab.slot, "u:3") && tab.placed && !tab.gone);
    andamento_arrangement_release(a);
    /* Removing a slot keeps its tab, reported gone. */
    ok(andamento_slot_remove(h, ws, T("u:2"), &error));
    a = andamento_arrangement_acquire(h, ws, &error);
    assert(andamento_arrangement_tab(a, 1, &tab) && eq(tab.slot, "u:2") && tab.gone);
    andamento_arrangement_release(a);
    ok(andamento_snapshot_is_current(h, s, &error));
    /* Both are in the workspace record. */
    AndamentoBytes record = {0};
    ok(andamento_record_export(h, T("workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7d"), &record, &error));
    assert(has(record, "version=5"));
    assert(has(record, "slot \"u:1\" rebind=\"keep-previous\" presentation=\"terminal\""));
    assert(has(record, "arrangement generation=1 owned=true"));
    andamento_bytes_free(record);
    andamento_slots_release(slots);
    andamento_slots_release(NULL); andamento_arrangement_release(NULL); andamento_slot_plan_release(NULL);
    andamento_snapshot_release(s);
    andamento_destroy(h);
}

/* ABI 3: the Workspace Overlay: tombstones, overrides and flags, soft
 * overrides that leave the provider's structure flowing, the proposal
 * export, and a Dashboard pin to a View. */
static int note(AndamentoArrangement *a, uint32_t kind, const char *key);
static int overlay_note(AndamentoOverlay *o, uint32_t kind, const char *key) {
    AndamentoOverlayInfo info; assert(andamento_overlay_info(o, &info));
    AndamentoOverlayNote n;
    for (size_t i = 0; i < info.note_count; i++) {
        assert(andamento_overlay_note(o, i, &n));
        if (n.kind == kind && eq(n.key, key)) return 1;
    }
    return 0;
}
static size_t dashboard_notes(Andamento *h, uint32_t kind, const char *key) {
    AndamentoDashboardOverlay *d = andamento_dashboard_overlay_acquire(h, &error);
    assert(d && !error);
    AndamentoDashboardOverlayInfo info; assert(andamento_dashboard_overlay_info(d, &info));
    assert(info.template_version.len == 16);
    size_t found = 0;
    AndamentoDashboardNote n;
    for (size_t i = 0; i < info.note_count; i++) {
        assert(andamento_dashboard_overlay_note(d, i, &n));
        if (n.kind == kind && eq(n.key, key)) found++;
    }
    assert(!andamento_dashboard_overlay_note(d, info.note_count, &n));
    andamento_dashboard_overlay_release(d);
    return found;
}
static void check_overlay(const char *config_path) {
    Andamento *h = fixture(config_path, NULL);
    const char *record =
        "andamento-record \"workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7e\" version=5 {\n"
        "    baseline version=\"1\" {\n"
        "        slot \"a\" { shell \"top\"; }\n"
        "        slot \"b\" rebind=\"keep-previous\" { shell \"make\"; }\n"
        "        hint { split \"main\" axis=\"row\" { tabs \"left\" selected=\"a\" { tab \"a\"; }; "
        "tabs \"right\" selected=\"b\" { tab \"b\"; }; }; }\n"
        "    }\n"
        "    arrangement generation=1 { split \"main\" axis=\"row\" { tabs \"left\" selected=\"a\" "
        "{ tab \"a\"; }; tabs \"right\" selected=\"b\" { tab \"b\"; }; }; }\n"
        "}";
    ok(andamento_record_import(h, T("workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7e"),
        (AndamentoText){(const uint8_t *)record, strlen(record)}, &error));
    AndamentoWorkspaceId ws = uuid(0x7e);
    /* Override b, change a's policy only: flags say which. */
    AndamentoViewSpec mine = {.content = ANDAMENTO_SLOT_COMMAND, .command = T("make check")};
    ok(andamento_slot_set(h, ws, T("b"), &mine, ANDAMENTO_REBIND_KEEP_PREVIOUS, &error));
    AndamentoViewSpec top = {.content = ANDAMENTO_SLOT_COMMAND, .command = T("top")};
    ok(andamento_slot_set(h, ws, T("a"), &top, ANDAMENTO_REBIND_ASK, &error));
    AndamentoSlots *slots = andamento_slots_acquire(h, ws, &error);
    assert(slots && andamento_slots_count(slots) == 2);
    assert(andamento_slots_flags(slots, 0) == ANDAMENTO_SLOT_FLAG_REBIND);
    assert(andamento_slots_flags(slots, 1) == ANDAMENTO_SLOT_FLAG_DETACHED);
    assert(andamento_slots_flags(slots, 2) == 0 && andamento_slots_flags(NULL, 0) == 0);
    andamento_slots_release(slots);
    /* A divider resize is a soft override: not owned. */
    AndamentoTab tabs[] = {{T("a"), 0, 0}, {T("b"), 0, 0}};
    AndamentoPanel panels[] = {
        {ANDAMENTO_NONE, T("main"), 1.0, ANDAMENTO_PANEL_SPLIT, ANDAMENTO_AXIS_ROW, 0, 0, ANDAMENTO_NONE},
        {0, T("left"), 3.0, ANDAMENTO_PANEL_TABS, 0, 0, 1, 0},
        {0, T("right"), 1.0, ANDAMENTO_PANEL_TABS, 0, 1, 1, 0},
    };
    uint64_t generation = 0;
    AndamentoArrangement *a = andamento_arrangement_acquire(h, ws, &error);
    AndamentoArrangementInfo info; assert(andamento_arrangement_info(a, &info));
    andamento_arrangement_release(a);
    assert(andamento_set_arrangement(h, ws, panels, 3, tabs, 2, info.generation, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_COMMITTED && !error);
    a = andamento_arrangement_acquire(h, ws, &error);
    assert(andamento_arrangement_info(a, &info) && !info.owned);
    assert(andamento_arrangement_flags(a) == ANDAMENTO_ARRANGEMENT_FLAG_SOFT);
    andamento_arrangement_release(a);
    /* Removing a baseline slot tombstones it; the derived arrangement drops its panel. */
    ok(andamento_slot_remove(h, ws, T("a"), &error));
    AndamentoOverlay *o = andamento_overlay_acquire(h, ws, &error);
    assert(o && !error && overlay_note(o, ANDAMENTO_OVERLAY_TOMBSTONED, "a"));
    andamento_overlay_release(o);
    a = andamento_arrangement_acquire(h, ws, &error);
    assert(andamento_arrangement_info(a, &info) && info.tab_count == 1);
    /* The soft override on the gone panel is kept and flagged. */
    assert(andamento_arrangement_flags(a) == (ANDAMENTO_ARRANGEMENT_FLAG_SOFT | ANDAMENTO_ARRANGEMENT_FLAG_UNRESOLVED));
    assert(note(a, ANDAMENTO_SECTION_UNRESOLVED, "left"));
    andamento_arrangement_release(a);
    /* Following the provider drops soft overrides. */
    assert(andamento_arrangement_resolve(h, ws, ANDAMENTO_ARRANGEMENT_FOLLOW, info.generation, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_COMMITTED && !error && generation > info.generation);
    assert(andamento_arrangement_resolve(h, ws, ANDAMENTO_ARRANGEMENT_KEEP, info.generation, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_STALE && !error);
    expected_error(andamento_arrangement_resolve(h, ws, 9, generation, &generation, &error));
    /* Name and mood are edits too. */
    ok(andamento_workspace_set_name(h, ws, 1, T("Build"), &error));
    ok(andamento_workspace_set_mood(h, ws, 1, T("calm"), &error));
    ok(andamento_workspace_set_mood(h, ws, 0, T(""), &error));
    o = andamento_overlay_acquire(h, ws, &error);
    AndamentoOverlayInfo oi; assert(andamento_overlay_info(o, &oi));
    assert(oi.has_baseline && !oi.primary_only && eq(oi.baseline_version, "1"));
    assert(oi.has_name && eq(oi.name, "Build") && !oi.has_mood && !oi.owned);
    assert(oi.edit_count == 3); /* a's tombstone, b's override, the name */
    andamento_overlay_release(o);
    andamento_overlay_release(NULL);
    assert(!andamento_overlay_acquire(h, uuid(0x01), &error) && error);
    andamento_string_free(error); error = NULL;
    /* The proposal: the edit set and its baseline version. */
    AndamentoBytes proposal = {0};
    ok(andamento_overlay_export(h, ws, &proposal, &error));
    assert(has(proposal, "overlay-proposal workspace=\"01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7e\" baseline=\"1\""));
    assert(has(proposal, "edit \"a\" {") && has(proposal, "tombstone"));
    assert(has(proposal, "edit \"b\" {") && has(proposal, "override {"));
    assert(has(proposal, "name \"Build\""));
    andamento_bytes_free(proposal);
    /* A Dashboard pin to b: flagged when b goes, kept until removed. */
    AndamentoLocalFact view = {.key=T(".view"), .kind=ANDAMENTO_FACT_TEXT,
                               .text=T("01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7e/b")};
    ok(andamento_local_set(h, T(".ref"), T("pin"), &view, 1, &error));
    assert(dashboard_notes(h, ANDAMENTO_DASHBOARD_PIN, "pin") == 0);
    ok(andamento_slot_remove(h, ws, T("b"), &error));
    assert(dashboard_notes(h, ANDAMENTO_DASHBOARD_PIN, "pin") == 1);
    ok(andamento_local_remove(h, T(".ref"), T("pin"), &error));
    assert(dashboard_notes(h, ANDAMENTO_DASHBOARD_PIN, "pin") == 0);
    andamento_dashboard_overlay_release(NULL);
    andamento_destroy(h);
}

/* ABI 3: the Dashboard's sidebar arrangement: placed by hints, committed at
 * a generation, closed by leaving out, restored, flagged, reset. */
static int note(AndamentoArrangement *a, uint32_t kind, const char *key) {
    AndamentoSectionNote n;
    for (size_t i = 0; i < andamento_arrangement_note_count(a); i++) {
        assert(andamento_arrangement_note(a, i, &n));
        if (n.kind == kind && eq(n.key, key)) return 1;
    }
    return 0;
}
static void check_sidebar_arrangement(void) {
    const char *config = "region \"b\" root-template=\"flotilla/region/tree\" default-host=\"sidebar\" order=20\n"
                         "region \"a\" root-template=\"flotilla/region/tree\" default-host=\"sidebar\" order=10\n"
                         "region \"f\" root-template=\"flotilla/region/tree\" default-host=\"floating\" order=5\n";
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && !error);
    AndamentoSnapshot *s = snapshot(h);
    assert(andamento_sidebar_arrangement_generation(h, &error) == 1 && !error);
    AndamentoArrangement *a = andamento_sidebar_arrangement_acquire(h, &error);
    assert(a && !error);
    AndamentoArrangementInfo info; assert(andamento_arrangement_info(a, &info));
    /* A column split holding a, b and .unplaced; then f's floating panel. */
    assert(info.generation == 1 && !info.owned && info.panel_count == 5 && info.tab_count == 4);
    assert(andamento_arrangement_floating_first(a) == 4);
    AndamentoPanel panel; AndamentoTab tab;
    assert(andamento_arrangement_panel(a, 0, &panel) && panel.parent == ANDAMENTO_NONE);
    assert(panel.kind == ANDAMENTO_PANEL_SPLIT && panel.axis == ANDAMENTO_AXIS_COLUMN);
    assert(andamento_arrangement_panel(a, 1, &panel) && panel.parent == 0 && panel.selected == 0);
    assert(andamento_arrangement_tab(a, panel.first_tab, &tab) && eq(tab.slot, "a") && tab.placed);
    assert(andamento_arrangement_panel(a, 4, &panel) && panel.parent == ANDAMENTO_NONE);
    assert(andamento_arrangement_tab(a, panel.first_tab, &tab) && eq(tab.slot, "f"));
    assert(note(a, ANDAMENTO_SECTION_PLACED, ".unplaced") && !note(a, ANDAMENTO_SECTION_CLOSED, "a"));
    andamento_arrangement_release(a);
    /* The user puts b first and closes the workspace fallback; f stays floating. */
    AndamentoTab tabs[] = {{T("b"), 0, 0}, {T("a"), 0, 0}, {T("f"), 0, 0}};
    AndamentoPanel panels[] = {
        {ANDAMENTO_NONE, T("root"), 1.0, ANDAMENTO_PANEL_SPLIT, ANDAMENTO_AXIS_COLUMN, 0, 0, ANDAMENTO_NONE},
        {0, T("1"), 1.0, ANDAMENTO_PANEL_TABS, 0, 0, 1, 0},
        {0, T("2"), 1.0, ANDAMENTO_PANEL_TABS, 0, 1, 1, 0},
        {ANDAMENTO_NONE, T("3"), 1.0, ANDAMENTO_PANEL_TABS, 0, 2, 1, 0},
    };
    uint64_t generation = 99;
    assert(andamento_set_sidebar_arrangement(h, panels, 4, 3, tabs, 3, 1, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_COMMITTED && !error && generation == 2);
    ok(andamento_snapshot_is_current(h, s, &error));
    /* Stale: no change, no error. */
    assert(andamento_set_sidebar_arrangement(h, panels, 4, 3, tabs, 3, 1, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_STALE && !error && generation == 2);
    /* An unknown section, or a floating panel under a docked one, is invalid. */
    AndamentoTab unknown[] = {{T("nope"), 0, 0}, {T("a"), 0, 0}, {T("f"), 0, 0}};
    expected_error(andamento_set_sidebar_arrangement(h, panels, 4, 3, unknown, 3, 2, &generation, &error));
    AndamentoPanel crossed[] = {panels[0], panels[1], panels[2], panels[3]};
    crossed[3].parent = 0;
    expected_error(andamento_set_sidebar_arrangement(h, crossed, 4, 3, tabs, 3, 2, &generation, &error));
    a = andamento_sidebar_arrangement_acquire(h, &error);
    assert(andamento_arrangement_info(a, &info) && info.owned && info.generation == 2);
    assert(note(a, ANDAMENTO_SECTION_CLOSED, ".unplaced"));
    andamento_arrangement_release(a);
    /* Restore it: by its hint, after a. */
    assert(andamento_sidebar_restore_section(h, T(".unplaced"), 2, &generation, &error)
        == ANDAMENTO_ARRANGEMENT_COMMITTED && !error && generation == 3);
    expected_error(andamento_sidebar_restore_section(h, T("undeclared"), 3, &generation, &error));
    /* A template without b flags its tab and keeps it. */
    const char *without_b = "region \"a\" root-template=\"flotilla/region/tree\" order=10\n"
                            "region \"f\" root-template=\"flotilla/region/tree\" default-host=\"floating\"\n";
    ok(andamento_configure(h, (AndamentoText){(const uint8_t *)without_b, strlen(without_b)}, &error));
    a = andamento_sidebar_arrangement_acquire(h, &error);
    assert(andamento_arrangement_tab(a, 0, &tab) && eq(tab.slot, "b") && tab.gone);
    assert(note(a, ANDAMENTO_SECTION_UNRESOLVED, "b") && note(a, ANDAMENTO_SECTION_RESTORED, ".unplaced") == 0);
    andamento_arrangement_release(a);
    AndamentoBytes record = {0};
    ok(andamento_record_export(h, T("dashboard"), &record, &error));
    assert(has(record, "sidebar generation=3 owned=true"));
    andamento_bytes_free(record);
    /* Reset places what is declared by its hints and forgets b. */
    assert(andamento_sidebar_reset(h, 3, &generation, &error) == ANDAMENTO_ARRANGEMENT_COMMITTED
        && generation == 4);
    a = andamento_sidebar_arrangement_acquire(h, &error);
    assert(andamento_arrangement_info(a, &info) && !info.owned && info.tab_count == 3);
    assert(andamento_arrangement_note_count(a) > 0 && !note(a, ANDAMENTO_SECTION_UNRESOLVED, "b"));
    andamento_arrangement_release(a);
    /* A workspace's arrangement has no floating panels or notes. */
    assert(andamento_arrangement_floating_first(NULL) == 0 && andamento_arrangement_note_count(NULL) == 0);
    andamento_snapshot_release(s);
    andamento_destroy(h);
}

int main(int argc, char **argv) {
    /* Everything but check_abi3 is an ABI 2 host, which ABI 3 keeps working. */
    assert(argc == 4 && andamento_abi_version() == 3);
    check_abi3(argv[1], argv[2]);
    check_records(argv[1], argv[2]);
    check_slots(argv[1], argv[2]);
    check_sidebar_arrangement();
    check_overlay(argv[1]);
    check_typed_details(argv[3]);
    check_workdirs();
    check_sibling_order();
    check_region_hints();
    check_providers(argv[1]);
    char *config = read_file(argv[1]);
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    Andamento *other = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && other && !error); free(config);
    char *patches = read_file(argv[2]);
    for (char *line = strtok(patches, "\n"); line; line = strtok(NULL, "\n"))
        ok(andamento_apply_patch_json(h, 100, (AndamentoText){(uint8_t *)line, strlen(line)}, &error));
    free(patches); /* all input borrowed only during its call */
    AndamentoSnapshot *s = snapshot(h);
    assert(andamento_snapshot_diagnostic_count(s) == 0);
    AndamentoNode project = find_node(s, "project"), vessel = find_node(s, "vessel");
    assert(eq(project.label, "Project P"));
    assert(eq(vessel.layout, "inline") && vessel.state == ANDAMENTO_LATENT && vessel.openable);
    AndamentoNode parent; assert(andamento_snapshot_node(s, vessel.parent, &parent));
    assert(eq(parent.key, "") == 0 && eq(parent.entity_kind, "project"));
    /* Loop keys are opaque and snapshot-owned, with an empty key for sections.
     * Aliases in separate regions must not share a loop invocation. */
    AndamentoText tree_loop = {0}, attention_loop = {0};
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        AndamentoNode node; AndamentoText loop;
        assert(andamento_snapshot_node(s, i, &node));
        assert(andamento_snapshot_node_loop_key(s, i, &loop));
        if (node.is_section) assert(loop.len == 0);
        else assert(loop.len > 0);
        if (eq(node.entity_kind, "vessel")) {
            if (!tree_loop.len) tree_loop = loop;
            else attention_loop = loop;
        }
    }
    assert(tree_loop.len && attention_loop.len);
    assert(tree_loop.len != attention_loop.len || memcmp(tree_loop.data, attention_loop.data, tree_loop.len));
    assert(!andamento_snapshot_node_loop_key(s, andamento_snapshot_node_count(s), &tree_loop));
    assert(!andamento_snapshot_node_loop_key(s, 0, NULL));
    int status = 0;
    for (size_t i = 0; i < vessel.field_count; ++i) {
        AndamentoField field; assert(andamento_snapshot_field(s, vessel.first_field+i, &field));
        if (eq(field.text, "waiting")) status = 1;
    }
    assert(status);
    expected_error(andamento_dispatch(other, s, project.toggle, &error));
    expected_error(andamento_dispatch(h, s, ANDAMENTO_NONE, &error));
    expected_error(andamento_apply_patch_json(h, 100, T("invalid"), &error));
    expected_error(andamento_configure(h, T("not { valid"), &error));
    /* Invalid input leaves the current snapshot's actions usable. */
    ok(andamento_dispatch(h, s, project.toggle, &error));
    expected_error(andamento_dispatch(h, s, project.toggle, &error)); /* stale */
    AndamentoSnapshot *collapsed = snapshot(h);
    assert(find_node(collapsed, "project").collapsed);
    assert(!project.collapsed && eq(project.label, "Project P")); /* old data stable */
    AndamentoControl c = find_control(collapsed);
    assert(c.value_kind == 1 && c.checked && eq(c.label, "Issues"));
    ok(andamento_dispatch(h, collapsed, c.action, &error));
    andamento_snapshot_release(collapsed);
    collapsed = snapshot(h);
    assert(!find_control(collapsed).checked);
    vessel = find_node(collapsed, "vessel");
    ok(andamento_dispatch(h, collapsed, vessel.activate, &error));
    AndamentoEffects *effects = take(h); assert(andamento_effects_count(effects) == 1);
    AndamentoEffect e; assert(andamento_effects_get(effects, 0, &e));
    assert(e.kind == ANDAMENTO_EFFECT_MATERIALIZE && eq(e.recipe, "printf hello") && eq(e.entity_id, "v"));
    AndamentoEffects *empty = take(h); assert(andamento_effects_count(empty) == 0); andamento_effects_release(empty);
    /* Failure surfaces a diagnostic and permits a fresh attempt. */
    ok(andamento_complete(h, e.request_id, ANDAMENTO_COMPLETE_ERROR, 0, T("cancelled"), &error));
    andamento_effects_release(effects);
    andamento_snapshot_release(collapsed); collapsed = snapshot(h);
    assert(andamento_snapshot_diagnostic_count(collapsed) == 1);
    AndamentoText diagnostic; assert(andamento_snapshot_diagnostic(collapsed, 0, &diagnostic));
    assert(eq(diagnostic, "vessel:v: cancelled"));
    ok(andamento_dispatch(h, collapsed, find_node(collapsed, "vessel").activate, &error));
    effects = take(h); assert(andamento_effects_get(effects, 0, &e));
    assert(e.kind == ANDAMENTO_EFFECT_MATERIALIZE);
    ok(andamento_complete(h, e.request_id, ANDAMENTO_COMPLETE_MATERIALIZE, 42, T(""), &error));
    ok(andamento_complete(h, e.request_id, ANDAMENTO_COMPLETE_ERROR, 0, T("late duplicate"), &error));
    AndamentoWorkspace ws = {42, 0, T("Worker"), 1};
    AndamentoPane pane = {42, 7, ANDAMENTO_PANE_TERMINAL, 1, 1, 0};
    ok(andamento_observe(h, &ws, 1, &pane, 1, &error));
    andamento_snapshot_release(collapsed); collapsed = snapshot(h);
    vessel = find_node(collapsed, "vessel");
    assert(vessel.state == ANDAMENTO_LIVE && vessel.workspace_id == 42 && vessel.selected);
    ok(andamento_dispatch(h, collapsed, vessel.activate, &error));
    empty = take(h); assert(andamento_effects_count(empty) == 1);
    AndamentoEffect focus; assert(andamento_effects_get(empty, 0, &focus));
    assert(focus.kind == ANDAMENTO_EFFECT_FOCUS && focus.workspace_id == 42);
    ok(andamento_complete(h, focus.request_id, ANDAMENTO_COMPLETE_FOCUS, 0, T(""), &error));
    andamento_effects_release(empty);

    /* Typed ingress preserves bytes; open workspace paths retain expired ancestor labels. */
    AndamentoFact f = {.key=T("display.label"), .kind=ANDAMENTO_FACT_TEXT,
                      .text=T("new\0label"), .has_ttl=1, .ttl_ms=10};
    ok(andamento_apply_entity(h, 100, T("project"), T("p"), T("fixture"), &f, 1, &error));
    AndamentoSnapshot *typed = snapshot(h);
    AndamentoNode updated = find_node(typed, "project");
    assert(updated.label.len == 9 && memcmp(updated.label.data, "new\0label", 9) == 0);
    assert(eq(project.label, "Project P"));
    f.kind = 999;
    expected_error(andamento_apply_entity(h, 100, T("project"), T("p"), T("fixture"), &f, 1, &error));
    ok(andamento_tick(h, 111, &error));
    AndamentoSnapshot *expired = snapshot(h);
    AndamentoText expired_label = find_node(expired, "project").label;
    assert(expired_label.len == 9 && memcmp(expired_label.data, "new\0label", 9) == 0);
    assert(find_node(expired, "vessel").workspace_id == 42);
    assert(!andamento_snapshot_node(expired, ANDAMENTO_NONE, &parent));
    assert(!andamento_snapshot_node(expired, 0, NULL));
    expected_error(andamento_observe(h, NULL, 1, NULL, 0, &error));
    const uint8_t bad_utf8[] = {255};
    expected_error(andamento_configure(h, (AndamentoText){bad_utf8, 1}, &error));
    expected_error(andamento_tick(NULL, 0, &error));
    assert(!andamento_snapshot_is_current(h, typed, &error) && !error);
    assert(!andamento_snapshot_is_current(h, NULL, &error) && !error);
    /* Empty ticks preserve actions, but a recipe-only change must invalidate
     * them just like a change to displayed geometry. */
    for (int metadata_update = 0; metadata_update < 2; ++metadata_update) {
        AndamentoSnapshot *displayed = snapshot(h);
        AndamentoNode intended = find_node(displayed, "vessel");
        ok(andamento_tick(h, 112, &error));
        ok(andamento_snapshot_is_current(h, displayed, &error));
        if (metadata_update) {
            AndamentoFact rename = {.key=T("display.label"), .kind=ANDAMENTO_FACT_TEXT,
                                     .text=T("a different arrangement")};
            ok(andamento_apply_entity(h, 112, T("vessel"), T("v"), T("fixture"), &rename, 1, &error));
        } else {
            AndamentoFact recipe = {.key=T("action.primary.recipe"), .kind=ANDAMENTO_FACT_TEXT,
                                     .text=T("printf changed-recipe")};
            ok(andamento_apply_entity(h, 112, T("vessel"), T("v"), T("fixture"), &recipe, 1, &error));
        }
        assert(!andamento_snapshot_is_current(h, displayed, &error) && !error);
        expected_error(andamento_dispatch(h, displayed, intended.activate, &error));
        AndamentoEffects *rejected = take(h);
        assert(andamento_effects_count(rejected) == 0);
        andamento_effects_release(rejected);
        andamento_snapshot_release(displayed);
        /* Discard the captured click. A fresh interaction uses a fresh frame. */
        AndamentoSnapshot *fresh = snapshot(h);
        ok(andamento_dispatch(h, fresh, find_node(fresh, "vessel").activate, &error));
        AndamentoEffects *accepted = take(h);
        assert(andamento_effects_count(accepted) == 1);
        AndamentoEffect target; assert(andamento_effects_get(accepted, 0, &target));
        assert(target.kind == ANDAMENTO_EFFECT_FOCUS && target.workspace_id == 42);
        ok(andamento_complete(h, target.request_id, ANDAMENTO_COMPLETE_FOCUS, 0, T(""), &error));
        andamento_effects_release(accepted);
        andamento_snapshot_release(fresh);
    }
    andamento_destroy(h); andamento_destroy(other);
    /* Snapshot and effect allocations outlive the sidebar. */
    assert(eq(project.label, "Project P") && eq(e.recipe, "printf hello"));
    assert(andamento_snapshot_node_count(s) > 0);
    andamento_effects_release(effects);
    andamento_snapshot_release(s); andamento_snapshot_release(collapsed);
    andamento_snapshot_release(typed); andamento_snapshot_release(expired);
    andamento_destroy(NULL); andamento_snapshot_release(NULL);
    andamento_effects_release(NULL); andamento_string_free(NULL);
    puts("Typed C fixture: rendering, controls, activation, completion, lifetimes and errors passed");
    return 0;
}
