# UI design strategy

Status: product policy. It governs how interface work is done during Alpha and
how the final interface is designed and then implemented. It is not a design,
and it commits to no layout, colour or component.

## Summary

| | During Alpha | After design freeze |
| --- | --- | --- |
| **Development UI** | Allowed | Replaced by the approved design |
| **Final UI** | Deferred | Implemented from an approved specification |
| **Major visual polish** | Deferred | Part of the design workflow below |

## 1. Development UI (Alpha)

When a feature needs to be seen or exercised during Alpha, Claude may build a
simple, functional development interface for it. Claude may make temporary,
reasonable decisions about:

- basic layouts
- forms
- tables
- temporary navigation
- buttons
- basic spacing
- simple visual hierarchy

Development UI should be clean and usable. It is **not** the final visual
language.

Two rules keep a temporary screen from turning into a permanent decision:

1. **Temporary UI decisions must not become architectural constraints.** A
   layout chosen to exercise a feature commits nothing about how the final
   product presents that feature.
2. **Backend and API design must not depend on a temporary screen layout.** The
   API is shaped by the domain and by [`API.md`](API.md), never by what a
   development screen happened to need. An endpoint that exists only because one
   screen was arranged a certain way is a defect.

Every surface, development or final, still implements the states in
[`UX-STATES.md`](UX-STATES.md). "Temporary" never means "only renders success".

## 2. The final Atrium interface

A dedicated UX and design phase happens **after the main product workflows are
real enough to design against, and before the public-Beta visual freeze**.
Designing earlier would mean designing against guesses about workflows that do
not yet exist.

### The intended feeling

- simple
- calm
- polished
- appliance-like
- cohesive
- approachable by non-technical users
- powerful without exposing complexity by default

### The dashboard's one question

> **"Is my system healthy, and is there anything that needs my attention?"**

The dashboard answers that question first. It must not become a wall of equally
important metrics. It prefers, in roughly this order:

- overall system health
- problems that need attention
- important active operations
- storage state
- service and app state
- useful next actions

Detailed metrics stay available through **progressive disclosure**, one step
away and never in the way. This follows principle 5 of [`PRODUCT.md`](PRODUCT.md):
beginner friendly, expert capable.

### References, not templates

Atrium may learn from the finished-appliance clarity of UGREEN UGOS Pro and
Synology DSM. It must **not** copy their layouts, branding, iconography or
interactions. Atrium needs its own visual identity. A reference is a quality
bar, not a template.

## 3. Final design workflow

```
Product intent
  -> UX / information architecture
  -> visual exploration
  -> approved screens
  -> Atrium Design System
  -> implementation specification
  -> Claude implementation
  -> screenshot / visual comparison
  -> polish
```

### After a design freeze

Once a screen or component is declared **design-frozen**, Claude is no longer
its visual designer. Claude implements the supplied design. Claude must not
silently invent a replacement layout, spacing system, colour, hierarchy,
navigation or interaction pattern.

**When the specification is incomplete, Claude reports the missing design
decision.** Claude does not fill the gap with its own. A gap that someone
notices is a question for the designer; a gap that is quietly filled is drift.

## 4. What this policy does not do

- It builds no interface. No UI work is part of M1.
- It chooses no framework, component library, colour or typeface.
- It does not change the prototype's existing desktop interface, whose dark
  appearance remains the initial and fallback theme.
