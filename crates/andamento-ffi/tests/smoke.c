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
int main(int argc, char **argv) {
    assert(argc == 3 && andamento_abi_version() == 2);
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

    /* Typed ingress preserves byte strings and expiry without a JSON decoder. */
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
    assert(expired_label.len != 9 || memcmp(expired_label.data, "new\0label", 9) != 0);
    assert(!andamento_snapshot_node(expired, ANDAMENTO_NONE, &parent));
    assert(!andamento_snapshot_node(expired, 0, NULL));
    expected_error(andamento_observe(h, NULL, 1, NULL, 0, &error));
    const uint8_t bad_utf8[] = {255};
    expected_error(andamento_configure(h, (AndamentoText){bad_utf8, 1}, &error));
    expected_error(andamento_tick(NULL, 0, &error));
    /* A click belongs to the displayed geometry. A tick or metadata update
     * arriving before dispatch rejects it; never retarget old coordinates. */
    for (int metadata_update = 0; metadata_update < 2; ++metadata_update) {
        AndamentoSnapshot *displayed = snapshot(h);
        AndamentoNode intended = find_node(displayed, "vessel");
        if (metadata_update) {
            AndamentoFact rename = {.key=T("display.label"), .kind=ANDAMENTO_FACT_TEXT,
                                     .text=T("a different arrangement")};
            ok(andamento_apply_entity(h, 112, T("vessel"), T("v"), T("fixture"), &rename, 1, &error));
        } else {
            ok(andamento_tick(h, 112, &error));
        }
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
