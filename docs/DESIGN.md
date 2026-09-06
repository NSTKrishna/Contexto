---
version: "1.0"
project: "Contexto — Universal Developer Context Manager"
archetype: "Linear / Raycast Developer Dark"
description: |
  A hyper-refined, keyboard-first, high-density dark design system for the Contexto
  Tauri desktop app and VS Code extension webviews. Synthesized from the Linear
  (lavender-indigo accent, deep obsidian surfaces, charcoal panel hairlines) and
  Raycast (Inter typography with ss03, command-palette density, hairline 1px borders)
  design systems from the VoltAgent/awesome-design-md standard.

  Feels like a native macOS developer tool — an organic extension of the terminal,
  VS Code, and Activity Monitor. Distraction-free, data-dense, and precise.
ai_agent_prompt: |
  Build this UI component according to the rules in docs/DESIGN.md.
  Use the 'Linear/Raycast Developer Dark' design system with semantic CSS variables
  (--bg-surface, --text-primary, --border-subtle, --status-accent).
  Ensure 4px spacing density, keyboard navigability, 12px monospace source badges,
  transitions ≤120ms ease-out, and no saturated bright colors.
---

# DESIGN.md — Contexto Universal Developer Context Manager

Design system specification for the **Tauri desktop app** and **VS Code extension webviews**.  
Archetype: **"Linear / Raycast Developer Dark"** — Hyper-dense, keyboard-first, distraction-free.

Sources: [linear.app DESIGN.md](https://github.com/VoltAgent/awesome-design-md/tree/main/design-md/linear.app) · [Raycast DESIGN.md](https://github.com/VoltAgent/awesome-design-md/tree/main/design-md/raycast)

---

## 1. Visual Theme & Atmosphere

- **Core Mood:** Precision instrument, technical control room, completely unobtrusive. Feels like an organic extension of macOS Terminal and VS Code Dark+.
- **Surface Philosophy:** Deep charcoal and true obsidian. No jarring pure whites or saturated primary colors. Layers are differentiated through subtle 1px border contrast (`rgba(255,255,255,0.06–0.12)`) and ambient box shadows — never through heavy background color jumps.
- **Glassmorphism:** Minimal and functional only. Translucent sidebars (`backdrop-filter: blur(12px)`, 85% opacity) over desktop content. Never decorative.
- **Tone of Voice:** Dense, technical, quietly luxurious — like a Bloomberg Terminal rebuilt by the Linear team.

---

## 2. Color Palette & Semantic Roles

All colors **must** use semantic CSS variables. Never hardcode hex values in component styles.

```css
:root {
  /* =========================================================================
   * Background Canvas — Linear-grade obsidian surfaces
   * ========================================================================= */
  --bg-canvas:            #090a0f;   /* Deepest background — window chrome */
  --bg-surface:           #11131a;   /* Cards, panels, sidebar */
  --bg-surface-elevated:  #181b24;   /* Modals, popovers, dropdowns */
  --bg-surface-hover:     #1f2330;   /* Interactive item hover state */
  --bg-surface-active:    #282d3e;   /* Active / selected item */

  /* =========================================================================
   * Borders & Dividers — Elevation through hairlines, not color
   * ========================================================================= */
  --border-subtle:        rgba(255, 255, 255, 0.06);  /* Level 1 cards, feed */
  --border-muted:         rgba(255, 255, 255, 0.12);  /* Level 2 modals, hover */
  --border-focus:         #5e6ad2;                    /* Linear-indigo focus ring */
  --border-strong:        rgba(255, 255, 255, 0.20);  /* High-contrast separators */

  /* =========================================================================
   * Text & Content
   * ========================================================================= */
  --text-primary:         #f0f2f5;   /* High-contrast headings, active values */
  --text-secondary:       #9499aa;   /* Descriptions, meta text, inactive icons */
  --text-muted:           #5d6275;   /* Timestamps, shortcut hints, placeholders */
  --text-code:            #e2b714;   /* Inline code, terminal arguments, paths */
  --text-on-accent:       #ffffff;   /* Text on --status-accent buttons */

  /* =========================================================================
   * Semantic Status Colors — Desaturated and purposeful
   * ========================================================================= */
  --status-idle:          #5d6275;   /* Paused, inactive daemon */
  --status-active:        #10b981;   /* Recording, healthy, git clean */
  --status-warning:       #f59e0b;   /* Token budget warning, unsaved changes */
  --status-error:         #ef4444;   /* Secrets redacted, daemon down, test failure */
  --status-accent:        #6366f1;   /* Primary actions, active task highlight */
  --status-accent-hover:  #818cf8;   /* Accent button hover */
  --status-accent-muted:  rgba(99, 102, 241, 0.15); /* Accent background tint */

  /* =========================================================================
   * Source Badge Colors — Monochrome, semantic only
   * Per the Do's & Don'ts: badges stay monochrome or semantic, never saturated.
   * ========================================================================= */
  --badge-terminal:       #5d6275;   /* [TERM] — neutral */
  --badge-git:            #f59e0b;   /* [GIT]  — amber (change signal) */
  --badge-ide:            #6366f1;   /* [IDE]  — accent (primary workflow) */
  --badge-fs:             #5d6275;   /* [FS]   — neutral */
  --badge-note:           #10b981;   /* [NOTE] — green (manual, intentional) */
  --badge-mcp:            #818cf8;   /* [MCP]  — light accent (AI layer) */
}
```

---

## 3. Typography Rules & Scale

### Font Families

```css
:root {
  --font-sans: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
  --font-mono: 'JetBrains Mono', 'SF Mono', 'Menlo', 'Consolas', monospace;

  /* Inter stylistic set ss03 — disambiguates l/1/I (critical for code contexts) */
  --font-feature-inter: 'calt', 'kern', 'liga', 'ss03';
}
```

### Type Scale

| Role | Size | Weight | Family | Tracking | Usage |
|------|------|--------|--------|----------|-------|
| Window Title | 14px | 600 | Sans | 0 | Desktop titlebar |
| Section Header | 12px | 600 | Sans | +0.05em | Uppercase section labels |
| Body | 13px | 400 | Sans | -0.01em | Reading, summaries |
| Body Strong | 13px | 500 | Sans | 0 | Labels, item titles |
| Code / Terminal | 12px | 400 | Mono | 0 | Command output, paths, diffs |
| Meta / Badges | 11px | 500 | Mono | +0.04em | Source badges `[GIT]`, timestamps |
| Shortcut Hints | 11px | 400 | Mono | 0 | `⌘⇧P`, `j/k` hints |
| Display | 20px | 600 | Sans | -0.03em | Modal headings, empty states |

---

## 4. Component Specifications

### 4.1 Context Stream Item (Event Card)

The primary repeating unit of the UI — one per captured `ContextEvent`.

```
┌─────────────────────────────────────────────────────────┐
│  [GIT] ● 2m ago                             ⌘K to copy  │
│  Committed: "feat: add ring buffer ingestion"            │
│  3 files changed, 47 insertions(+), 2 deletions(-)      │
└─────────────────────────────────────────────────────────┘
```

**Anatomy:**
- **Container:** `background: var(--bg-surface)`, `border: 1px solid var(--border-subtle)`, `border-radius: 6px`, `padding: 8px 12px`
- **Hover:** `border-color: var(--border-muted)`, `background: var(--bg-surface-hover)` — no transform/translate jumps
- **Active/Selected:** `background: var(--bg-surface-active)`, `border-color: var(--border-focus)`
- **Source Badge:** 2px colored left-border pill + `font-family: var(--font-mono)`, `font-size: 10px`, uppercase, `color: var(--badge-{source})`
- **Timestamp:** `var(--text-muted)`, relative format ("2m ago"). On hover, tooltip shows ISO-8601
- **Summary:** Max 3 lines with CSS `line-clamp: 3`. `font-size: 13px`, `color: var(--text-secondary)`
- **Redacted indicator:** If `was_redacted: true`, show `[REDACTED KEY]` badge in `var(--status-error)` at 80% opacity

**Keyboard navigation:** Arrow keys or `j`/`k` move between cards. `Enter` expands. `Esc` collapses.

---

### 4.2 Status Bar (Daemon & Recording Indicator)

Fixed at the bottom of the window (desktop) or top of the webview panel (VS Code).

```
● Recording  ctxd v0.1.0  |  127 events  |  3.2MB  |  ⌘⇧P to pause
```

**Specs:**
- **Height:** 28px, fixed positioning
- **Background:** `var(--bg-canvas)`, `border-top: 1px solid var(--border-subtle)`
- **Pulsing dot:** 6px circle. `var(--status-active)` (green) when capturing. `var(--status-warning)` (amber) when paused. CSS `@keyframes pulse` with `box-shadow` — never `opacity` (avoid layout recalc)
- **Font:** 11px monospace, `var(--text-muted)`, pipe `|` separators
- **Transition:** `≤120ms ease-out` on all state changes

---

### 4.3 Command Palette / Search Modal (`ctx search`)

Floating overlay — the primary power-user entry point.

```
┌──────────────────────────────────────────────────────────┐
│ 🔍  Search context...                      [tag:decision] │
├──────────────────────────────────────────────────────────┤
│  [GIT]  Committed ring buffer impl               2h ago  │
│  [IDE]  Opened crates/ctx-core/src/lib.rs        2h ago  │
│  [TERM] cargo test --workspace                   3h ago  │
│  [NOTE] Decided: FTS5 over sqlite-vss            5h ago  │
└──────────────────────────────────────────────────────────┘
  ESC to close  ·  ↑↓ to navigate  ·  Enter to expand
```

**Specs:**
- **Position:** Centered horizontally, `top: 15%` of viewport
- **Width:** 600px (fixed), `max-width: calc(100vw - 48px)` for narrow views
- **Background:** `var(--bg-surface-elevated)`, `box-shadow: 0 16px 36px rgba(0,0,0,0.55)`, `border: 1px solid var(--border-muted)`, `border-radius: 8px`
- **Input:** Borderless, `font-size: 14px`, `var(--font-sans)`, instant autofocus on open, `color: var(--text-primary)`
- **Filter badges:** Tag-style pills with `background: var(--status-accent-muted)`, `color: var(--status-accent)`. Example: `tag:decision`, `repo:ctx`, `source:git`
- **Result list:** Virtualized (no pagination). Each row: 36px height, same anatomy as Event Card but denser

---

### 4.4 Token Budget Indicator

Shows how much of the LLM context window the current context occupies.

```
Context: ████████░░░░░░░░  3,847 / 8,192 tokens  (47%)
```

- **Bar:** CSS `<progress>` or `<div>` with `width` transition `≤120ms`
- **Color:** `var(--status-active)` < 70%, `var(--status-warning)` 70–90%, `var(--status-error)` > 90%
- **Typography:** 11px mono, `var(--text-muted)`

---

## 5. Layout Principles & Spacing

### Grid Base: 4px unit

All spacing must be a multiple of 4px:

| Token | Value | Use |
|-------|-------|-----|
| `--space-1` | 4px | Icon padding, tight gaps |
| `--space-2` | 8px | Card internal padding (vertical) |
| `--space-3` | 12px | Card internal padding (horizontal) |
| `--space-4` | 16px | Section gaps, form fields |
| `--space-6` | 24px | Major section spacing |
| `--space-8` | 32px | Panel margins |
| `--space-12` | 48px | Large section rhythm |

**Card density:** `padding: var(--space-2) var(--space-3)` (8px 12px).  
**List item gap:** 6px (between cards in the feed).

---

### Window Constraints

| Context | Default Size | Minimum |
|---------|-------------|---------|
| Desktop App | 960 × 640px | 680 × 480px |
| VS Code Sidebar | 300px wide | 280px |
| VS Code Panel | 100% width | 400px |

---

## 6. Depth & Elevation System

Elevation is achieved **strictly** through border brightness and ambient box shadows — **never** through pure background color changes alone.

| Level | Context | Style |
|-------|---------|-------|
| 0 Canvas | Window background | `background: var(--bg-canvas)` — flat |
| 1 Cards / Feed | Event cards, list rows | `border: 1px solid var(--border-subtle)` |
| 2 Sidebar / Panels | Left nav, context inspector | `border-right: 1px solid var(--border-subtle)` + `backdrop-filter: blur(12px)` |
| 3 Modals / Dropdowns | Command palette, popovers | `box-shadow: 0 12px 30px rgba(0,0,0,0.5)`, `border: 1px solid var(--border-muted)` |
| 4 Tooltips | Hover overlays | `box-shadow: 0 4px 12px rgba(0,0,0,0.4)`, `border: 1px solid var(--border-strong)` |

---

## 7. Motion & Animation

**Rule: All UI transitions must be `≤ 120ms ease-out`. Slower animations are banned.**

```css
:root {
  --transition-fast:   80ms ease-out;   /* Hover states, border color */
  --transition-base:   120ms ease-out;  /* Card expand/collapse, modal open */
  --transition-slow:   200ms ease-out;  /* Allowed only for progress bars */
}
```

- **Pulse animation** (status dot): CSS `@keyframes` on `box-shadow` scale — not opacity or transform
- **Modal entry:** `transform: translateY(-8px) → translateY(0)` + `opacity: 0 → 1`, duration `var(--transition-base)`
- **Card expand:** `max-height` transition with `overflow: hidden` — not `height: auto` (layout thrash)

---

## 8. Responsive & Density Behavior

### Desktop Full Width (> 800px): Two-Pane Layout

```
┌─────────────────────────┬──────────────────────────────────┐
│  Active Tasks           │  Live Context Feed               │
│  Project Selector       │                                  │
│  ─────────────────────  │  [GIT] Committed ring buffer...  │
│  Layer Hierarchy        │  [IDE] Opened ctx-core/lib.rs    │
│  ● Layer 1: Events      │  [TERM] cargo test               │
│  ○ Layer 2: Tasks       │  [NOTE] Decided: FTS5 over vss   │
│  ○ Layer 3: Project     │                                  │
│                         │  ── Semantic Query Inspector ──  │
│  [Status Bar]           │  Query: "sqlx migration"         │
└─────────────────────────┴──────────────────────────────────┘
```

### Narrow Sidebar (< 400px / VS Code Sidebar): Single Column

Collapses to a single card feed with:
- Top dropdown: Project / Task selector
- Quick search bar (opens full Command Palette)
- Card feed with source badge badges

---

## 9. Do's and Don'ts

### ✅ DO

- Show precise timestamps in relative format with hover for ISO-8601 (`2m ago` → tooltip: `2026-09-06T14:15:00Z`)
- Provide keyboard navigation (`↑↓`, `j/k`, `Enter` to expand, `Esc` to close) across all lists
- Show explicit visual markers when an item had a secret redacted: `[REDACTED KEY]` badge in `var(--status-error)`
- Use `font-variant-numeric: tabular-nums` on all numeric displays (event counts, token budget)
- Support `prefers-reduced-motion` — disable all animations when set

### ❌ DON'T

- Use multi-color saturation for source badges — keep them monochrome or semantic (see §2)
- Use slow transition animations — all UI transitions must be `≤ 120ms ease-out`
- Paginate with page numbers — use smooth virtualized scrolling (`@tanstack/virtual` or CSS container queries)
- Hardcode hex values in component styles — always use semantic CSS variables
- Show a loading spinner for search — results should appear incrementally (FTS5 is sub-millisecond)
- Use `transform: translateY()` on card hover — causes layout jitter in dense lists

---

## 10. AI Agent Prompt Guide

When asking AI coding agents (Antigravity, Cursor, Claude Code) to build UI components, include this header in your prompt:

```
Build this UI component according to the rules in docs/DESIGN.md.
Use the 'Linear/Raycast Developer Dark' design system with semantic CSS variables
(--bg-surface, --text-primary, --border-subtle, --status-accent).
Ensure 4px spacing density, keyboard navigability, 12px monospace source badges,
transitions ≤120ms ease-out, and no saturated bright colors.
Font families: Inter (--font-sans) for UI, JetBrains Mono (--font-mono) for code/badges.
```
