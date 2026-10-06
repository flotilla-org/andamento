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
    for (size_t i = 0; i < andamento_snapshot_node_count(s); i++) {
        AndamentoNode n; assert(andamento_snapshot_node(s, i, &n));
        if (n.is_section && eq(n.key, "andamento.unplaced-workspaces")) {
            empty_fallback++;
            for (size_t j = i+1; j < andamento_snapshot_node_count(s); j++) {
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
        } else if (eq(n.key, "andamento.unplaced-workspaces")) {
            assert(andamento_snapshot_region_hints(s, i, &hints));
            assert(hints.default_host.len == 0 && hints.has_order == 0);
            synthetic++;
        }
    }
    assert(synthetic && entity);
    andamento_snapshot_release(s); andamento_destroy(h);
}

int main(int argc, char **argv) {
    assert(argc == 4 && andamento_abi_version() == 2);
    check_typed_details(argv[3]);
    check_workdirs();
    check_region_hints();
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
