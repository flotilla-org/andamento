/* A C host against the generated header: one happy path per entry point,
 * and what only C sees: struct layouts, ownership, NULL handling and error
 * strings. Behaviour is tested in Rust. */
#include "andamento.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define T(s) ((AndamentoText){(const uint8_t *)(s), sizeof(s) - 1})
#define PROVIDER T("sub-1")
static char *error;

static void ok(uint32_t result) {
    if (!result) fprintf(stderr, "ABI error: %s\n", error ? error : "(none)");
    assert(result && error == NULL);
}
static void fails(uint32_t result) {
    assert(!result && error);
    andamento_string_free(error);
    error = NULL;
}
static int eq(AndamentoText a, const char *b) {
    return a.data && a.len == strlen(b) && memcmp(a.data, b, a.len) == 0;
}
static AndamentoText text(const char *s) { return (AndamentoText){(const uint8_t *)s, strlen(s)}; }
static char *read_file(const char *path) {
    FILE *f = fopen(path, "rb");
    assert(f);
    assert(fseek(f, 0, SEEK_END) == 0);
    long len = ftell(f);
    assert(len >= 0);
    rewind(f);
    char *data = malloc((size_t)len + 1);
    assert(data && fread(data, 1, (size_t)len, f) == (size_t)len);
    data[len] = 0;
    fclose(f);
    return data;
}
static Andamento *create(const char *config) {
    Andamento *h = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(h && !error);
    return h;
}
static void apply_lines(Andamento *h, const char *path, uint64_t now) {
    char *patches = read_file(path);
    for (char *line = strtok(patches, "\n"); line; line = strtok(NULL, "\n"))
        ok(andamento_apply_patch_json(h, now, PROVIDER, text(line), &error));
    free(patches);
}
static AndamentoSnapshot *snapshot(Andamento *h) {
    AndamentoSnapshot *s = andamento_snapshot_acquire(h, &error);
    assert(s && !error);
    return s;
}
static size_t find(AndamentoSnapshot *s, const char *kind, AndamentoNode *out) {
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        assert(andamento_snapshot_node(s, i, out));
        if (!out->is_section && eq(out->entity.kind, kind)) return i;
    }
    assert(!"missing node");
    return ANDAMENTO_NONE;
}
static AndamentoWorkspaceId uuid(uint8_t tail) {
    return (AndamentoWorkspaceId){{0x01, 0x92, 0x0a, 0x6b, 0x7c, 0x3d, 0x7e, 0x4f,
                                   0x8a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x6a, tail}};
}
static int same(AndamentoWorkspaceId a, AndamentoWorkspaceId b) {
    return memcmp(a.bytes, b.bytes, sizeof a.bytes) == 0;
}

/* Facts, topology, the snapshot, dispatch, effects and completion. */
static void check_sidebar(const char *config_path, const char *patches_path) {
    assert(andamento_abi_version() == 4);
    char *config = read_file(config_path);
    Andamento *h = create(config);
    ok(andamento_configure(h, text(config), &error));
    free(config);
    apply_lines(h, patches_path, 100);
    /* Every entity names its provider; "local" is Andamento's own. */
    fails(andamento_apply_patch_json(h, 100, T(""), T("{}"), &error));
    fails(andamento_apply_patch_json(h, 100, T("local"), T("{\"target\":{\"kind\":\"root\"},\"source_id\":\"x\",\"set\":{}}"), &error));
    AndamentoFact facts[] = {{.key = T("display.label"), .kind = ANDAMENTO_FACT_TEXT, .text = T("Pee"),
                              .has_ttl = 1, .ttl_ms = 50},
                             {.key = T("gone"), .kind = ANDAMENTO_FACT_UNSET}};
    ok(andamento_apply_entity(h, 100, PROVIDER, T("project"), T("q"), T("fixture"), facts, 2, &error));
    AndamentoFact twice[] = {facts[0], facts[0]};
    fails(andamento_apply_entity(h, 100, PROVIDER, T("project"), T("q"), T("fixture"), twice, 2, &error));
    AndamentoWorkspaceId notes = uuid(0x7b), worker = uuid(0x7c);
    ok(andamento_workspace_register(h, notes, &error));
    assert(andamento_workspace_registered(h, notes, &error) && !error);
    AndamentoWorkspace open[] = {{notes, 0, T("Notes"), 1}};
    ok(andamento_observe(h, open, 1, NULL, 0, &error));
    fails(andamento_observe(h, NULL, 1, NULL, 0, &error));
    AndamentoFact host = {.key = T(".host.kind"), .kind = ANDAMENTO_FACT_TEXT, .text = T(".workspace")};
    ok(andamento_apply_workspace(h, 100, PROVIDER, notes, T("host"), &host, 1, &error));
    ok(andamento_tick(h, 101, &error));

    AndamentoSnapshot *s = snapshot(h);
    ok(andamento_snapshot_is_current(h, s, &error));
    AndamentoNode section, n;
    assert(andamento_snapshot_node(s, 0, &section) && section.is_section && section.parent == ANDAMENTO_NONE);
    assert(section.entity.kind.len == 0 && section.entity.kind.data != NULL);
    assert(!andamento_snapshot_node(s, ANDAMENTO_NONE, &n) && !andamento_snapshot_node(s, 0, NULL));
    size_t vessel = find(s, "vessel", &n);
    assert(eq(n.entity.provider, "sub-1") && eq(n.entity.id, "v") && n.state == ANDAMENTO_LATENT);
    assert(n.loop_key.len && n.copy_url == ANDAMENTO_NONE && !n.stale);
    AndamentoField field;
    assert(andamento_snapshot_field(s, n.first_field, &field) && field.class_ <= ANDAMENTO_FIELD_PRIORITY);
    AndamentoControl control = {0};
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        AndamentoNode m;
        assert(andamento_snapshot_node(s, i, &m));
        for (size_t j = 0; j < m.control_count; ++j)
            if (andamento_snapshot_control(s, m.first_control + j, &control) &&
                control.kind == ANDAMENTO_CONTROL_DISPLAY_VARIABLE) goto found;
    }
    assert(!"no display control");
found:
    assert(eq(control.variable, "show-issues") && control.persist && control.value_kind == ANDAMENTO_VALUE_BOOL);
    AndamentoText diagnostic;
    for (size_t i = 0; i < andamento_snapshot_diagnostic_count(s); ++i)
        assert(andamento_snapshot_diagnostic(s, i, &diagnostic) && diagnostic.len);

    /* Activating v materializes it; completing gives its workspace. */
    ok(andamento_dispatch(h, s, n.activate, &error));
    fails(andamento_dispatch(h, s, ANDAMENTO_NONE, &error));
    AndamentoEffects *effects = andamento_effects_take(h, &error);
    assert(effects && !error && andamento_effects_count(effects) == 1);
    AndamentoEffect e;
    assert(andamento_effects_get(effects, 0, &e) && e.kind == ANDAMENTO_EFFECT_MATERIALIZE);
    assert(eq(e.entity.provider, "sub-1") && eq(e.entity.id, "v") && eq(e.recipe, "printf hello"));
    assert(!andamento_effects_get(effects, 1, &e));
    ok(andamento_complete(h, e.request_id, ANDAMENTO_COMPLETE_MATERIALIZE, worker, T(""), &error));
    andamento_effects_release(effects);
    andamento_snapshot_release(s); /* the dispatch made it stale */
    AndamentoWorkspace both[] = {{notes, 0, T("Notes"), 0}, {worker, 1, T("Worker"), 1}};
    AndamentoPane pane = {worker, 7, ANDAMENTO_PANE_TERMINAL, 1, 1, 0};
    ok(andamento_observe(h, both, 2, &pane, 1, &error));
    AndamentoWorkdir dir = {worker, T("/repo")};
    ok(andamento_observe_workdirs(h, &dir, 1, &error));

    s = andamento_snapshot_acquire_details(h, &error);
    assert(s && !error);
    vessel = find(s, "vessel", &n);
    assert(n.state == ANDAMENTO_LIVE && same(n.workspace_id, worker));
    AndamentoEntity v = {T("sub-1"), T("vessel"), T("v")};
    size_t detail = andamento_snapshot_detail_find(s, v);
    AndamentoDetail card;
    assert(andamento_snapshot_detail(s, detail, &card) && card.has_workspace && same(card.workspace_id, worker));
    AndamentoDetailAction action;
    assert(andamento_snapshot_detail_action(s, detail, 0, &action) && eq(action.intent, "focus-workspace"));
    AndamentoDetailField detail_field;
    assert(andamento_snapshot_detail_field(s, detail, 0, &detail_field));
    AndamentoEntity other = {T("sub-2"), T("vessel"), T("v")};
    assert(andamento_snapshot_detail_find(s, other) == ANDAMENTO_NONE);
    assert(andamento_snapshot_detail_request(h, s, v, &error) == detail && !error);
    ok(andamento_dispatch(h, s, card.activate, &error));
    effects = andamento_effects_take(h, &error);
    assert(andamento_effects_get(effects, 0, &e) && e.kind == ANDAMENTO_EFFECT_FOCUS && same(e.workspace_id, worker));
    ok(andamento_complete(h, e.request_id, ANDAMENTO_COMPLETE_FOCUS, e.workspace_id, T(""), &error));
    fails(andamento_complete(h, e.request_id, 9, worker, T(""), &error));
    andamento_effects_release(effects);
    fails(andamento_copy_subject_url(h, (AndamentoEntity){T(""), T("vessel"), T("v")}, &error));

    /* The snapshot outlives the sidebar; its text stays readable. */
    andamento_destroy(h);
    assert(andamento_snapshot_node(s, vessel, &n) && eq(n.entity.id, "v"));
    andamento_snapshot_release(s);
}

/* Records, display variables, local entities and sibling orders. */
static void check_dashboard(const char *config_path, const char *patches_path) {
    char *config = read_file(config_path);
    Andamento *h = create(config);
    apply_lines(h, patches_path, 100);
    ok(andamento_set_display_variable(h, T("show-issues"), T("false"), &error));
    fails(andamento_set_display_variable(h, T("show-issues"), T("maybe"), &error));
    AndamentoLocalFact label = {.key = T("display.label"), .kind = ANDAMENTO_FACT_TEXT, .text = T("Mine")};
    ok(andamento_local_set(h, T(".section"), T("s1"), &label, 1, &error));
    AndamentoLocalFact group[] = {
        {.key = T(".section"), .kind = ANDAMENTO_FACT_ENTITY, .entity = {T("local"), T(".section"), T("s1")}},
        {.key = T(".position"), .kind = ANDAMENTO_FACT_INTEGER, .integer = 2},
    };
    ok(andamento_local_set(h, T(".group"), T("g1"), group, 2, &error));
    group[0].entity.provider = T("");
    fails(andamento_local_set(h, T(".group"), T("g2"), group, 1, &error));
    fails(andamento_local_set(h, T("vessel"), T("v"), &label, 1, &error));
    ok(andamento_local_remove(h, T(".group"), T("never"), &error));
    AndamentoSnapshot *s = snapshot(h);
    AndamentoNode n;
    find(s, "vessel", &n);
    AndamentoEntity order[] = {{T("sub-1"), T("vessel"), T("v")}};
    ok(andamento_set_sibling_order(h, n.loop_key, order, 1, &error));
    fails(andamento_set_sibling_order(h, T("not a loop key"), order, 1, &error));
    /* What the Dashboard keeps: local entities with their facts, and orders. */
    AndamentoDashboard *kept = andamento_dashboard_get(h, &error);
    assert(kept && !error && kept->generation == andamento_record_generation(h, T("dashboard"), &error));
    assert(kept->local_count == 2 && kept->order_count == 1);
    const AndamentoLocalEntity *g1 = &kept->locals[0];
    assert(eq(g1->entity.provider, "local") && eq(g1->entity.kind, ".group") && g1->fact_count == 2);
    assert(eq(g1->facts[0].key, ".position") && g1->facts[0].kind == ANDAMENTO_FACT_INTEGER && g1->facts[0].integer == 2);
    assert(g1->facts[1].kind == ANDAMENTO_FACT_ENTITY && eq(g1->facts[1].entity.id, "s1"));
    assert(eq(kept->locals[1].facts[0].text, "Mine"));
    assert(kept->orders[0].loop_key.len == n.loop_key.len &&
           memcmp(kept->orders[0].loop_key.data, n.loop_key.data, n.loop_key.len) == 0);
    assert(kept->orders[0].entity_count == 1 && eq(kept->orders[0].entities[0].id, "v"));
    andamento_dashboard_free(kept);
    andamento_dashboard_free(NULL);
    andamento_snapshot_release(s);
    ok(andamento_workspace_register(h, uuid(0x7d), &error));

    AndamentoBytes names = {0};
    ok(andamento_record_names(h, &names, &error));
    const char *expected = "dashboard\nworkspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7d";
    assert(names.len == strlen(expected) && memcmp(names.data, expected, names.len) == 0);
    andamento_bytes_free(names);
    AndamentoText workspace = T("workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7d");
    uint64_t generation = andamento_record_generation(h, T("dashboard"), &error);
    assert(generation && !error && andamento_record_generation(h, T("dashboard"), &error) == generation);
    assert(andamento_record_generation(h, T("workspace/9"), &error) == 0);
    fails(0);
    AndamentoBytes dashboard = {0}, record = {0}, none = {0};
    ok(andamento_record_get(h, T("dashboard"), &dashboard, &error));
    ok(andamento_record_get(h, workspace, &record, &error));
    fails(andamento_record_get(h, T("nonsense"), &none, &error));
    fails(andamento_record_get(h, T("dashboard"), NULL, &error));
    assert(none.data == NULL);

    Andamento *fresh = create(config);
    free(config);
    ok(andamento_record_put(fresh, T("dashboard"), (AndamentoText){dashboard.data, dashboard.len}, &error));
    ok(andamento_record_put(fresh, workspace, (AndamentoText){record.data, record.len}, &error));
    assert(andamento_workspace_registered(fresh, uuid(0x7d), &error));
    fails(andamento_record_put(fresh, T("dashboard"), T("{"), &error));
    ok(andamento_workspace_forget(fresh, uuid(0x7d), &error));
    ok(andamento_workspace_forget(fresh, uuid(0x7d), &error));
    andamento_bytes_free(dashboard);
    andamento_bytes_free(record);
    andamento_bytes_free(none);
    andamento_destroy(fresh);
    andamento_destroy(h);
}

/* Providers: two subscriptions' same entity, staleness and retraction. */
static void check_providers(void) {
    Andamento *h = create("region \"tree\" root-template=\"flotilla/region/tree\" placement=\"tree\"\n"
                          "placement \"tree\" { for \"p\" kind=\"project\"; }\n");
    AndamentoFact label = {.key = T("display.label"), .kind = ANDAMENTO_FACT_TEXT, .text = T("One"),
                           .has_ttl = 1, .ttl_ms = 50};
    ok(andamento_apply_entity(h, 100, T("sub-1"), T("project"), T("p"), T("x"), &label, 1, &error));
    ok(andamento_apply_entity(h, 100, T("sub-2"), T("project"), T("p"), T("x"), &label, 1, &error));
    ok(andamento_provider_set_stale(h, T("sub-1"), 1, &error));
    ok(andamento_tick(h, 1000, &error));
    AndamentoSnapshot *s = snapshot(h);
    size_t projects = 0;
    for (size_t i = 0; i < andamento_snapshot_node_count(s); ++i) {
        AndamentoNode n;
        assert(andamento_snapshot_node(s, i, &n));
        if (n.is_section) continue;
        projects++;
        assert(eq(n.entity.provider, "sub-1") && n.stale); /* sub-2's expired */
    }
    assert(projects == 1);
    andamento_snapshot_release(s);
    ok(andamento_provider_retract(h, T("sub-1"), &error));
    fails(andamento_provider_retract(h, T(""), &error));
    andamento_destroy(h);
}

/* Slots, plans, resolutions, the overlay and arrangements. */
static void check_workspace(const char *config_path) {
    char *config = read_file(config_path);
    Andamento *h = create(config);
    free(config);
    AndamentoWorkspaceId ws = uuid(0x7e);
    ok(andamento_workspace_register(h, ws, &error));
    AndamentoWorkspace open = {ws, 0, T("Scratch"), 1};
    ok(andamento_observe(h, &open, 1, NULL, 0, &error));
    uint64_t revision = andamento_workspace_content_revision(h, &error);
    AndamentoText argv[] = {T("htop"), T("-d")};
    AndamentoViewSpec spec = {.content = ANDAMENTO_SLOT_COMMAND, .argv = argv, .argc = 2,
                              .has_presentation = 1, .presentation = T("terminal")};
    ok(andamento_slot_set(h, ws, T("u:1"), &spec, ANDAMENTO_REBIND_KEEP_PREVIOUS, &error));
    AndamentoViewSpec facet = {.content = ANDAMENTO_SLOT_FACET, .entity = {T("sub-1"), T("vessel"), T("v")},
                               .facet = T("primary")};
    ok(andamento_slot_set(h, ws, T("u:2"), &facet, ANDAMENTO_REBIND_ASK, &error));
    facet.entity.provider = T("");
    fails(andamento_slot_set(h, ws, T("u:3"), &facet, ANDAMENTO_REBIND_ASK, &error));
    fails(andamento_slot_set(h, ws, T("u:3"), &spec, 7, &error));
    fails(andamento_slot_set(h, uuid(0x01), T("u:3"), &spec, ANDAMENTO_REBIND_REPLACE, &error));
    assert(andamento_workspace_content_revision(h, &error) > revision);

    AndamentoSlots *slots = andamento_slots_get(h, ws, &error);
    assert(slots && !error && slots->slot_count == 2);
    const AndamentoSlot *slot = &slots->slots[0];
    assert(eq(slot->key, "u:1") && slot->flags == 0 && !slot->has_saved);
    assert(slot->spec.argc == 2 && eq(slot->spec.argv[1], "-d") && eq(slot->spec.presentation, "terminal"));
    assert(eq(slots->slots[1].spec.entity.provider, "sub-1"));
    andamento_slots_free(slots);
    assert(!andamento_slots_get(h, uuid(0x01), &error));
    fails(0);

    /* Plan, validate, complete; save a portable resolution for it. */
    AndamentoSlotContent *plan = andamento_slot_plan(h, ws, T("u:1"), T(""), &error);
    assert(plan && !error && plan->state == ANDAMENTO_CONTENT_UPDATING);
    assert(plan->recipe.argc == 2 && !plan->has_saved);
    assert(andamento_slot_valid(h, ws, T("u:1"), plan->token, &error));
    assert(andamento_slot_complete(h, ws, T("u:1"), plan->token, 1, &error));
    AndamentoResolutionField fields[] = {{T("session"), T("S")}, {T("host"), T("feta")}};
    uint64_t generation = 0;
    assert(andamento_slot_resolution_set(h, ws, T("u:1"), T("cleat-session"), fields, 2, plan->resolution, 0,
                                         &generation, &error) == ANDAMENTO_COMMITTED && generation);
    assert(andamento_slot_resolution_set(h, ws, T("u:1"), T("cleat-session"), fields, 1, plan->resolution, 0,
                                         &generation, &error) == ANDAMENTO_STALE && !error);
    assert(andamento_slot_resolution_set(h, ws, T("u:1"), T("Bad Kind"), fields, 2, plan->resolution,
                                         generation, NULL, &error) == ANDAMENTO_INVALID);
    fails(0);
    AndamentoSlotContent *current = andamento_slot_plan(h, ws, T("u:1"), plan->resolution, &error);
    assert(current && current->state == ANDAMENTO_CONTENT_CURRENT);
    assert(current->has_saved && current->saved.field_count == 2 && eq(current->saved.fields[0].name, "host"));
    andamento_slot_plan_free(current);
    andamento_slot_plan_free(plan);
    slots = andamento_slots_get(h, ws, &error);
    assert(slots->slots[0].has_saved && slots->slots[0].saved.generation == generation);
    andamento_slots_free(slots);
    assert(andamento_slot_resolution_clear(h, ws, T("u:1"), generation, &generation, &error) == ANDAMENTO_COMMITTED);
    ok(andamento_slot_retry(h, ws, T("u:1"), &error));
    assert(!andamento_slot_release_previous(h, ws, T("u:1"), &error) && !error);

    /* The overlay and the user's name and mood. */
    ok(andamento_workspace_set_name(h, ws, 1, T("Build"), &error));
    ok(andamento_workspace_set_mood(h, ws, 0, T(""), &error));
    ok(andamento_slot_remove(h, ws, T("u:2"), &error));
    ok(andamento_slot_reattach(h, ws, T("u:1"), &error));
    AndamentoOverlay *overlay = andamento_overlay_get(h, ws, &error);
    assert(overlay && !error && !overlay->has_baseline && eq(overlay->name, "Build"));
    assert(!overlay->has_mood && overlay->mood.data && overlay->note_count == 0);
    andamento_overlay_free(overlay);
    assert(!andamento_overlay_get(h, uuid(0x01), &error));
    fails(0);

    /* The arrangement: panels name their parents; commit, stale, invalid. */
    AndamentoWorkspaceArrangement *a = andamento_arrangement_get(h, ws, &error);
    assert(a && !error && a->doc.panel_count == 0 && a->generation == 0);
    andamento_arrangement_free(a);
    AndamentoText tabs[] = {T("u:1")};
    AndamentoDocPanel panels[] = {
        {ANDAMENTO_NONE, T("root"), 1.0, ANDAMENTO_PANEL_SPLIT, ANDAMENTO_AXIS_ROW, NULL, 0, ANDAMENTO_NONE},
        {0, T("1"), 1.0, ANDAMENTO_PANEL_TABS, 0, tabs, 1, 0},
    };
    AndamentoArrangementDoc doc = {panels, 2};
    a = andamento_arrangement_commit(h, ws, &doc, 0, &error);
    assert(a && !error && a->status == ANDAMENTO_COMMITTED && a->owned && a->doc.panel_count == 2);
    assert(a->doc.panels[1].parent == 0 && eq(a->doc.panels[1].tabs[0], "u:1") && a->doc.panels[1].selected == 0);
    uint64_t at = a->generation;
    andamento_arrangement_free(a);
    a = andamento_arrangement_commit(h, ws, &doc, 0, &error);
    assert(a && !error && a->status == ANDAMENTO_STALE && a->generation == at);
    andamento_arrangement_free(a);
    panels[1].parent = 1;
    assert(!andamento_arrangement_commit(h, ws, &doc, at, &error));
    fails(0);
    assert(andamento_arrangement_resolve(h, ws, ANDAMENTO_ARRANGEMENT_KEEP, at, &generation, &error) ==
           ANDAMENTO_COMMITTED);

    /* The sidebar's arrangement: the same panels, a dock and floating ones. */
    AndamentoSidebarArrangement *sidebar = andamento_sidebar_arrangement_get(h, &error);
    assert(sidebar && !error && sidebar->doc.dock.panel_count && sidebar->note_count);
    assert(sidebar->generation == andamento_sidebar_arrangement_generation(h, &error));
    AndamentoSidebarArrangement *committed =
        andamento_sidebar_arrangement_commit(h, &sidebar->doc, sidebar->generation, &error);
    assert(committed && !error && committed->status == ANDAMENTO_COMMITTED && committed->owned);
    andamento_sidebar_arrangement_free(committed);
    committed = andamento_sidebar_reset(h, sidebar->generation, &error);
    assert(committed && !error && committed->status == ANDAMENTO_STALE);
    generation = committed->generation;
    andamento_sidebar_arrangement_free(committed);
    andamento_sidebar_arrangement_free(sidebar);
    assert(!andamento_sidebar_restore_section(h, T("undeclared"), generation, &error));
    fails(0);
    committed = andamento_sidebar_reset(h, generation, &error);
    assert(committed && committed->status == ANDAMENTO_COMMITTED && !committed->owned);
    andamento_sidebar_arrangement_free(committed);

    andamento_slots_free(NULL);
    andamento_slot_plan_free(NULL);
    andamento_overlay_free(NULL);
    andamento_arrangement_free(NULL);
    andamento_sidebar_arrangement_free(NULL);
    andamento_destroy(h);
}

/* Handles, NULLs and poisoning. */
static void check_handles(void) {
    assert(!andamento_create(NULL, 1, &error));
    fails(0);
    assert(!andamento_create((const uint8_t *)"region", 6, &error));
    fails(0);
    fails(andamento_tick(NULL, 0, &error));
    assert(andamento_snapshot_node_count(NULL) == 0 && andamento_effects_count(NULL) == 0);
    andamento_snapshot_release(NULL);
    andamento_effects_release(NULL);
    andamento_destroy(NULL);
    andamento_string_free(NULL);
    andamento_bytes_free((AndamentoBytes){0});
    Andamento *h = andamento_create(NULL, 0, NULL);
    assert(h && andamento_tick(h, 0, NULL));
    andamento_destroy(h);
}

int main(int argc, char **argv) {
    assert(argc == 3);
    check_sidebar(argv[1], argv[2]);
    check_dashboard(argv[1], argv[2]);
    check_providers();
    check_workspace(argv[1]);
    check_handles();
    puts("C ABI smoke test passed");
    return 0;
}
