# mperf-gui rebuild: the Tracks UI in GPUI

Status: proposed · Prototype: `ui-prototype/` (React mock, variant B "Tracks" approved)
Scope: the current GUI is replaced wholesale. Only the data/analysis layer survives.

## 1. Goal

Rebuild `mperf-gui` around the approved Tracks design: a persistent master timeline that scopes
every view, one global filter, scenario-gated detail tabs, a selection side panel, and closable
source/disassembly tabs. Equal priority: an architecture where adding a future view is a
mechanical exercise (one derive + one view + one registry entry), and a visual standard that
matches the prototype (dense, consistent tokens, light + dark).

Non-goals for this effort: new collection capabilities (tracked separately in §10), the CLI/TUI,
and remote/live profiling.

## 2. What we are building (UX contract)

Shell, top to bottom:

1. **Title bar** — recording chip (name + scenario badge, switcher over recent recordings), theme toggle.
2. **Filter bar** — time-range chip · thread multi-select · module multi-select · symbol substring
   input · selection chip · coverage stat ("18% of 33.4K samples in scope") · Clear all.
3. **Master timeline** (collapsible) — per-thread activity lanes (92px label gutter, density = alpha,
   filtered-out threads dimmed) + 2–3 scenario-pinned counter tracks sharing the gutter and the
   full-run x-domain. Drag anywhere = set the global time filter; double-click = clear. The active
   range renders as a shaded selection on every track. Uncore tracks form a visually separated
   group labeled "system-wide — follows the time filter, ignores thread/symbol filters".
4. **Detail tab strip** — scenario-gated static tabs + dynamic closable source tabs (monospace
   label + ×) + side-panel toggle.
5. **Active view** (inventory in §7).
6. **Selection panel** (right, 300px, collapsible) — selected function, "source" button when asm
   exists, IPC/LLC-MPKI/BE-stall tiles, TMA mini-bar, callers/callees sandwich.
7. **Status bar** — recording · scenario · CPU · duration/samples/Hz · ● filtered / ○ unfiltered.

Filter semantics (the load-bearing rule): **one** `GlobalFilter { time, threads, modules, symbol }`
scopes every view; `Selection { frame }` is highlight-only and never scopes data. The symbol query
both filters (stacks containing a match) and highlights (flame frames not matching render dimmed).
Every timeline brushes the same time filter. The coverage stat always tells the user how much data
is in scope. Double-click on a hotspot row opens a closable source tab.

## 3. Current state: what dies, what survives

15.8k LOC today. Verdict per area (from the code audit):

**Survives (≈8.5k LOC, with edits)**
- `profile.rs` (1003) — sample/observation loading, eager in-memory materialization, confidence
  scaling. Loading already runs on the background executor (`main.rs:316-342`). Keep.
- `profile_analysis.rs` (2009) — `CallTree` (+`icicle_layout`), `FunctionAnalysis`,
  `FunctionDetails`, `FlameScopeHeatmap`, `CpuUtilizationHeatmap`, `CounterTracks`,
  `TimeOrderTimeline`, `SampleFilter`. This is the crown jewel; it already takes
  `(&ProfileData, &SampleFilter)` and is pure. Keep; change `Rc` payloads to `Arc` (they must
  become `Send` for background compute) and extend `SampleFilter` (§4).
- `model.rs` (667), `snapshot.rs` (770), `memory.rs` (333), `roofline.rs` (465), `metrics.rs`
  (412), `source.rs` (81), `recent.rs` (99), `flamegraph.rs` (232, folded-file support may go
  with flamelens) — loaders for summary/TMA/USE/memory/roofline tables. Keep, reorganize under
  `session/`.
- `views/flame_canvas.rs` (493) — the only engineered chart in the app: row-indexed layout,
  content-mask culling, sub-pixel merge, binary-search hit-test, bounded label shaping
  (`shape_line` + char cap), hitbox + phase-correct mouse events, notify-only-on-change hover.
  This is the seed of the chart framework; its patterns generalize to everything.

**Dies (≈7.3k LOC)**
- All of `views/` except `flame_canvas.rs`, plus `theme.rs` and the chrome/state plumbing in
  `main.rs`. Specific liabilities the rebuild removes:
  - `stack_timeline.rs` renders one positioned div per stack segment — O(segments) elements.
  - Six charts are naive quad loops with no hitbox, no hover, no tooltip, no drag.
  - Drag-to-select a time range does not exist anywhere; all selection is click-one-bucket.
  - Analyses run synchronously on the UI thread inside `render_*`, behind 1-entry `RefCell`
    caches holding non-`Send` `Rc`s (`analysis_cache.rs`).
  - One 33-field `MperfGui` entity; chart hover triggers whole-app re-render
    (`hovered_frame`, `roofline_chart_size` live in global state).
  - `theme.rs` is 14 dark-only `u32` constants; three duplicated `mix_rgb` helpers and five
    per-view chart palettes exist.
  - The root element carries global drag flags for sidebar/panel/column resizing
    (`main.rs:689-714`) — the anti-pattern to avoid in the new splitter/brush code.
- Dependency `flamelens` and the folded-file/SVG flamegraph path: the flame graph renders from
  `CallTree::icicle_layout` on our own canvas.

## 4. Target architecture

Strict one-directional layering. Nothing above L2 knows gpui; nothing in L4 computes.

    recording dir (perf.db, info.json, …)
        │  load once on background executor; lazy per-function asm queries
        ▼
    L1 session::Session      immutable in-memory model of one recording
        │  pure fns of (&Session, &GlobalFilter)
        ▼
    L2 analysis::*           derived datasets; Arc payloads; unit-testable without gpui
        │  AnalysisHub: cache + background compute + generations
        ▼
    L3 state::Workspace      gpui entities: filter, selection, tabs, panels
        │  observe / notify
        ▼
    L4 views::* + charts::* + ui::*   dumb renderers

### Module layout

    mperf-gui/src/
      main.rs                 thin: boot, window, assets, keymap registration
      session/                L1 — mod, load (perf.db readers, moved from model/profile/snapshot/
                              memory/roofline/metrics), sources (lazy asm/source loader)
      analysis/               L2 — profile_analysis split by topic + new derives (§6) + hub.rs
      state/                  L3 — workspace.rs (GlobalFilter, Selection, tab/panel state, recents)
      theme.rs                token struct, light+dark, Global
      ui/                     widget kit: tab_bar, splitter, table, text_input, dropdown, chip,
                              meter, stat_tile, tooltip, section, badge
      charts/                 framework: frame, brush, hover, text_cache, ramp; chart types:
                              lanes, tracks, heatmap, flame, stacked_area, bars, scatter_loglog
      views/                  shell: filter_bar, master_timeline, tab_strip, selection_panel,
                              status_bar; views: summary, hotspots, flame, flamescope, cores,
                              tma, resources, memory, roofline, source

### State model (replaces the 33-field god-entity)

- `Entity<Workspace>` — session handle, `GlobalFilter`, `Selection`, open tabs
  (`Vec<TabId>` where `TabId = View(ViewId) | Source(FrameId)`), active tab, panel geometry,
  master-timeline collapsed flag. All filter mutations go through `Workspace::set_filter`, which
  bumps a generation, invalidates the hub, and `cx.notify()`s.
- One child `Entity` per open view holding **view-local** state only: flame zoom path + hover,
  table sort column, roofline selection, source pin line. Hover updates notify the view entity,
  not the workspace — chart hover must never re-render the app.
- `GlobalFilter { time: Option<TimeRange>, threads: Option<SmallBitSet>, modules: Option<SmallVec>,
  symbol: String }`. `SampleFilter` in L2 grows `modules` + `symbol` (stack-contains-match
  semantics, matching the prototype); the existing `frame_ids` mechanism is retired in favor of
  highlight-only selection.

### Analysis pipeline (the biggest behavioral change)

`analysis::hub::AnalysisHub`, owned by `Workspace`:

- `hub.get::<A>(key) -> AnalysisState<Arc<A::Output>>` where
  `AnalysisState = Ready(Arc) | Computing { prev: Option<Arc> } | NotRequested`.
- On miss: `cx.background_spawn` the pure L2 function against `Arc<Session>`; tag with the filter
  generation; on completion, store and notify. Stale generations are dropped on arrival.
- Views render the last completed snapshot; while a newer one computes they keep rendering it at
  slightly reduced opacity. No skeletons, no blocking, no UI-thread compute — ever.
- Cache is a small per-kind LRU (4 entries) keyed by `FilterKey`, so toggling a filter off and on
  is instant. (Today: 1 entry, so every toggle recomputes.)
- Only **visible** views request analyses; opening a recording computes nothing until the first
  tab renders.
- Symbol typing debounced ~120ms. Brush **preview** during drag is a local overlay in the view
  entity; the filter commits on mouse-up only.

Prerequisite: `profile_analysis.rs` outputs move `Rc → Arc` and drop interior mutability so they
are `Send + Sync`. Mechanical but touches the whole file; do it first (M0).

## 5. Foundation subsystems

### 5.1 Theme v2

A `Theme` struct registered as a gpui `Global`, with **light and dark** instances, following the
OS via `Window::observe_window_appearance` (+ manual override persisted with recents). Tokens:

- Surfaces/chrome/text/border/accent (semantic, as today but structured).
- **Viz tokens ported verbatim from the prototype palette** (validated for CVD in both modes):
  `series[8]` (light: `#2a78d6 #eb6834 #1baf7a #eda100 #e87ba4 #008300 #4a3aa7 #e34948`, dark
  variants as in `ui-prototype/src/index.css`), `status {good, warn, serious, critical}`,
  sequential blue ramp (13 steps) for heatmaps/gutters, module-kind mapping
  (user=series1, lib=series3, runtime=series7, kernel=series2), TMA category colors
  (retiring=s3, bad-spec=s5, frontend=s4, backend=s2).
- One `mix()`/ramp helper; the five per-view palettes and three `mix_rgb` copies are deleted.
- Metrics (heights, gutters: lane 15px, flame row 17–18px, label gutter 92px, panel 300px).

### 5.2 Chart framework (`charts/`) — generalizing `flame_canvas.rs`

Every chart is a `canvas()` wrapped by a common scaffold:

- **`ChartFrame`** — prepaint inserts a hitbox (`window.insert_hitbox`), paint gets plot rect +
  gutter split, draws hairline grid/axis ticks from the theme, sets cursor via
  `set_cursor_style(style, &hitbox)`. Mouse events registered in paint with
  `window.on_mouse_event::<E>` gated on `DispatchPhase::Bubble` + `hitbox.is_hovered` — the
  `flame_canvas.rs` pattern, never the global-root-flags pattern.
- **`BrushController`** — shared drag→`TimeRange` logic: anchor on down, preview overlay on move
  (notify view entity only), commit `Workspace::set_filter` on up; double-click clears. Also paints
  the committed selection band. Used by: thread lanes, counter tracks, flame scope, CPU heat lanes.
- **`HoverModel`** — hit-test result + tooltip. `canvas()` has no `ElementId`, so element-level
  `.tooltip()` is unavailable inside plots; tooltips render via `deferred(anchored().position(p))`
  from hover state in the view entity (or `Window::set_tooltip` for simple text). Notify only on
  hovered-id change.
- **`TextCache`** — memoized `shape_line` results keyed by (string, px, color); measure-based
  elide via `LineLayout::index_for_x`. Canvas text everywhere (axis labels included) — no more
  sibling-div label columns that need shared scroll handles.
- **Painters** — quads (`paint_quad`), polylines (`PathBuilder::stroke`), **filled areas**
  (`PathBuilder::fill` — available, unused today) for sparklines/stacked areas, dashed rules
  (`dash_array`), rotated labels via the existing SVG asset trick (roofline roofs).
- Chart types built on this: `Lanes` (threads/CPUs; density or utilization coloring),
  `CounterTrack` (line+area, gutter label + live value), `Heatmap` (flame scope), `Flame`
  (evolved `FlameChart` with weight function), `StackedArea` (TMA over time), `Bars`
  (histograms), `LogLogScatter` (roofline).

Budgets: paint is culled against `content_mask` and merges sub-pixel data (flame already does);
a lane chart paints O(visible buckets), never O(samples).

### 5.3 Widget kit (`ui/`) — gpui has none of these

| Widget | Mechanics |
|---|---|
| `TabBar` | closable tabs; reuse the proven close pattern (`stop_propagation` on mouse-down + `on_click`, `chrome.rs:119-126`); overflow scrolls; middle-click close |
| `Splitter` | per-widget hitbox drag (no global flags); powers selection panel + any future split |
| `VirtualTable` | `uniform_list` + sticky header via `UniformListDecoration` (the proper hook, unused today) or the proven sibling-header-in-shared-`overflow_x` container; sortable headers; column widths + resize handles (port `bottom_panel.rs:540-559` logic); cell renderers incl. inline-bar cells; scrollbar as a list decoration |
| `TextInput` | single-line, for the symbol filter: `FocusHandle` + `on_key_down` + IME plumbing from gpui `input.rs`; v1 scope = insert/backspace/delete/arrows/home/end/select-all |
| `DropdownMenu` | `deferred(anchored())` popover, checkbox rows, outside-click dismiss |
| `Chip`, `Badge`, `Meter`, `StatTile`, `InfoTooltip`, `CollapsibleSection` | small; `InfoTooltip` = `.id()`'d icon + `.tooltip(AnyView)` (the Top-Down (i) pattern from the prototype) |

A `--gallery` debug window renders every widget and chart type with fake data in both themes —
it is where look-and-feel gets iterated without loading recordings, and it doubles as the visual
regression checklist.

## 6. Derived data: existing vs new

| Need (view) | Source | Status |
|---|---|---|
| Thread activity lanes (master timeline) | samples bucketed per thread | new, trivial derive |
| Counter tracks + pinned sets + uncore flag | `CounterTracks` | extend: `scope: Process/System`, per-scenario pinned ids |
| Call tree / flame / icicle | `CallTree`, `icicle_layout` | reuse; add weight fn (cycles=1, instructions=per-sample counter with confidence scaling, alloc=bytes when data exists) |
| Flame scope heatmap | `FlameScopeHeatmap` | reuse; add drag-select |
| Hotspots + per-function metrics | `FunctionAnalysis` (has IPC/LLC/MPKI/stall) | reuse |
| Callers/callees | `FunctionDetails` | reuse |
| Per-CPU occupancy lanes | `CpuUtilizationHeatmap` | reuse |
| Concurrency histogram (elapsed time at N busy CPUs) | cpu_observations | new derive |
| Thread balance (busy %, sync share, migrations) | samples (sync = barrier/futex frames) + cpu_observations transitions | new derive |
| TMA hierarchy + dominant path | `tma_summary` + `TMAInfo` metric names (dot-nesting) | new loader/shaping; must handle vendor shapes (Arm has no bad-spec/memory split) |
| TMA over time | `tma_intervals` | loader exists; new stacked-area chart |
| Per-function TMA mini-bars | `VIEW tma` | new loader (one query at load) |
| USE resources with explicit metrics + scope/source | `snapshot_summary`/`snapshot_resource_samples`/`snapshot_findings` | loaders exist; reshape to the prototype's metric-row model (bandwidth vs capacity split) |
| Memory panels (miss-ratio, strides, reuse, working set) | `memory_*` tables | loaders exist |
| Roofline ceilings + loops | calibration + `VIEW roofline` | loaders exist |
| Source/asm heat listing | `assembly_lines` ⋈ `assembly_samples` + source files | data exists; **lazy per-function query** — the sqlite connection is currently dropped after load, so `session::sources` re-opens per query on the background executor |

## 7. Views (build order and notes)

1. **Hotspots** — `VirtualTable`; module badge, self% inline bar, metric columns gated on TMA
   presence, TMA mini-bar cell; click=select, dblclick=open source tab.
2. **Flame Graph** — evolved `FlameChart`; toolbar "STACKS [top-down|bottom-up] · WEIGHT
   [cycles|instructions|alloc]"; symbol-filter dimming; dblclick zoom; tooltip via HoverModel.
3. **Flame Scope** — Heatmap + BrushController + canvas axis text.
4. **Cores** — CPU `Lanes` + concurrency `Bars` + balance `VirtualTable`.
5. **Top-Down** — hierarchy rows (indent, branch-colored bars, (i) `InfoTooltip` per metric —
   descriptions from `TmaMetric.desc`, "dominant" badge) + `StackedArea` intervals + per-function
   mini-bar list.
6. **Resources** — cards with metric rows + meters + `PathBuilder::fill` sparklines + findings.
7. **Summary** — stat tiles + scenario-conditional blocks, every block links to its view.
8. **Memory** — stat tiles + counter tracks + miss-ratio curve + stride/reuse `Bars` + working-set table + verdict box.
9. **Roofline** — `LogLogScatter` (small dots: r≈2px+√share·5px, hollow=scalar, faded=low-confidence),
   detail panel with quality badge + "open source", loops `VirtualTable`. Rewrite kills the
   current render-feedback loop (`roofline_chart_size` written from paint); label layout computes
   inside paint from bounds.
10. **Source/Asm** — split panes, `uniform_list` per side (fixes today's unvirtualized source
    view), heat gutters from the sequential ramp, % chips ≥10%, event-share chips on hot
    instructions, hover-link + click-pin between sides, summary box. Opened only as dynamic tabs.

**Scenario gating**: the view registry declares `is_available(&Session) -> bool` per view (data-
presence driven, like today's `visualization_available()`); the tab strip renders available views
in a canonical order. `mperf_data::ScenarioUi` stays as-is for the TUI — the GUI no longer
consumes it, which avoids a recording-format change. Adding a future view = implement
`analysis::Foo` + `views::foo` + one registry entry; the shell, filter, caching, theming, and
tab management come for free.

## 8. Keyboard & polish (gpui actions — completely unused today)

`actions!` + `KeyBinding` + per-pane `FocusHandle`s: Cmd/Ctrl+W close tab, Ctrl+Tab / Ctrl+Shift+Tab
cycle, Cmd/Ctrl+F focus symbol input, Esc clear selection → clear filter (two-stage), ↑/↓ + Enter in
tables, ←/→/+/- pan/zoom time on the master timeline, Cmd/Ctrl+K command palette (port of the
Workbench palette: views, functions, threads, clear-filters) as the last polish item.

## 9. Milestones (each one PR-sized, buildable, demoable)

- **M0 — Foundations.** Theme v2 (light+dark, viz tokens, gallery window). `charts/` core
  (ChartFrame, BrushController, HoverModel, TextCache, painters). `ui/` kit v1. `Rc→Arc` in
  `profile_analysis.rs`; `AnalysisHub` with background compute + LRU; `Workspace`/`GlobalFilter`
  entities. Old GUI still boots untouched. *Accept: gallery shows all widgets/charts in both
  themes; `cargo test` covers hub generations + brush math.*
- **M1 — Shell cutover.** New chrome: title bar, filter bar (with TextInput + dropdowns), master
  timeline (lanes + pinned tracks + brush), tab strip, selection panel (tiles + callers/callees),
  status bar, Hotspots view. **Old `views/`, old `theme.rs`, chrome state, and flamelens are
  deleted in this PR.** *Accept: open a real recording; brush/thread/module/symbol filters rescope
  hotspots + lanes live; coverage stat correct; selection panel follows clicks.*
- **M2 — Core views.** Flame Graph (weights), Flame Scope, full Timeline view (all counter tracks,
  uncore group), Summary v1, dynamic Source/Asm tabs with dblclick-from-hotspots. *Accept: the
  prototype's flagship flow — brush a checkpoint, see flame reshape, dblclick `gather_neighbors`,
  read the hot loads.*
- **M3 — Analytic views.** Top-Down (hierarchy + intervals + per-function), Resources (USE v2
  cards + findings), Cores (lanes + concurrency histogram + balance). New derives land with unit
  tests against `truth` fixtures where applicable.
- **M4 — Deep dives.** Memory, Roofline. Summary blocks for both.
- **M5 — Polish.** Keyboard + palette, light-theme QA pass over every view, perf pass against
  budgets (§11), docs/screenshots, dead-code sweep.
- **C-track (collector, parallel, unlocks prototype features that mock data faked):**
  1. allocation-site tracking (LD_PRELOAD) → alloc-weight flame;
  2. sched_switch/BPF occupancy → exact Cores view instead of sample-inferred;
  3. PEBS/IBS per-instruction attribution → event-share chips on asm become measured, not aggregated;
  4. uncore IMC + RAPL sampling in all scenarios → system track group everywhere.

Dependency chain: M0 → M1 → {M2, M3, M4 in any order} → M5. C-track items are independent.

## 10. Performance budgets

- Load: 1M-sample recording fully materialized < 2s on the background executor (today's loader
  already qualifies; keep it).
- Filter change: cached < 16ms to paint; cold recompute of the visible view's analyses < 300ms at
  1M samples, off-thread, previous frame held meanwhile.
- Brush drag: 60fps — preview is paint-only; zero analysis work until mouse-up.
- Hover: notifies one view entity; whole-app re-render on hover is a regression test.
- Element counts: charts are canvases; tables are `uniform_list`; nothing renders O(data) divs
  (`stack_timeline.rs` is the cautionary tale).

## 11. Testing

- L2 stays gpui-free → plain `cargo test`: derives against `truth` fixtures, filter semantics,
  hub generation/staleness, brush math, ramp/contrast helpers.
- Port and extend the existing `flame_canvas.rs` hit-test tests to the chart framework.
- The gallery window is the visual checklist (both themes, dense data, empty states).
- Manual acceptance script per milestone (recorded in this doc's PR descriptions).

## 12. Risks

- **`Rc→Arc` ripple** through `profile_analysis.rs` (2009 LOC) — mechanical but must land first;
  isolate in its own commit inside M0.
- **TextInput scope creep** — it exists to type a symbol substring; hold the v1 line (no IME
  perfection, no multi-line, no undo).
- **Canvas tooltips** — `canvas()` can't carry `.tooltip()`; the deferred/anchored path is proven
  in gpui but new to this codebase; build once in HoverModel, reuse everywhere.
- **sqlite crate re-open for lazy asm queries** — connection is dropped after load today; per-query
  re-open on the background executor is simple but should be measured on large `assembly_lines`
  tables (index on `(module_path, symbol)` if needed — postprocess owns the schema).
- **TMA shape variance** (Intel slots / AMD cycles / Arm 3-category) — the hierarchy view must be
  driven by `TMAInfo` metric names, not a hardcoded 4-way tree; the prototype's tree is the Intel
  special case.
- **gpui 0.2.x API drift** — pin the version for the rebuild; upgrade as its own change.
