# Header Niche Absorption Design

## Goal

Use spare group-header width as a dense projection of the first visible child path, without changing the underlying render tree, grouping identity, collapse state, or child ordering.

## Core Model

A group header can expose a bounded `niche`: the remaining inline width after rendering the group's own header fields and the inspect affordance. The niche is offered to child candidates in visual order and can absorb only a prefix of those children.

Absorption is recursive, but conservative:

- If direct tabs at a level are absorbed, they are batched into one tab-segment run and no child groups from that same level are absorbed into that header.
- If no direct tabs at a level are absorbed, the first child group may be absorbed as a group-fragment, then any remaining niche may be offered to that absorbed group's children.
- A collapsed group can be absorbed as a group-fragment, but its children are not recursively absorbed.
- Anything that does not fit remains in the normal child projection below the header.
- Same-level tabs must render as a single adjacent segment run so Zellij tabbar segment joins remain correct.
- Runs from different levels are separated by the rail border character, not spaces.

This means the first implementation can produce:

```text
▼ repo ─  overview  shell 
  ▼ branch-a
```

or:

```text
▼ repo ─ branch-a ─  agent-1  agent-2 
```

but not the mixed same-level form:

```text
▼ repo ─  overview  shell  ─ branch-a
```

That restriction is intentional for readability and can be revisited after seeing the projection in real sessions.

## Rendering Shape

The renderer should treat absorption as a projection over `RenderNode` children:

1. Build the normal group header text.
2. Compute the niche range before the inspect glyph/tail.
3. Ask a pure-ish absorption helper for:
   - styled niche text fragments,
   - relative hit boxes,
   - a list of consumed direct child indexes or tab indexes.
4. Draw the niche fragments into the header row.
5. Render the remaining children below as usual.

The first slice should keep this inside `andamento-rail`; it does not need controller changes or new config. It should reuse the existing compact segment renderer and hit payloads rather than inventing a second segment renderer.

## Interaction Semantics

Absorbed tabs switch to their tab, using the same `HitAction::SwitchTab` as compact strips. Absorbed group fragments can start as non-clickable display text if that keeps the first slice smaller, but the preferred behavior is to preserve the same group toggle/inspect affordance later.

Collapse stays meaningful:

- a collapsed absorbed group displays only its group fragment.
- an expanded absorbed group may donate children into the remaining niche.

## Boundaries

This does not implement:

- adaptive priority/pinned duplication.
- body/content slot rendering.
- arbitrary mixed same-level absorption.
- image-backed segment rendering.
- a user-facing config surface.

Those remain follow-on work once the projection model is proven.
