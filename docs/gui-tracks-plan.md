# mperf-gui rebuild: the Tracks UI in GPUI

Status: proposed · Prototype: `ui-prototype/` (React mock, variant B "Tracks" approved)
Scope: the current GUI is replaced wholesale. Only the data/analysis layer survives.

## 1. Goal

Rebuild `mperf-gui` around the approved Tracks design: a persistent master timeline that scopes
every view, one global filter, scenario-gated detail tabs, a selection side panel, and closable
source/disassembly tabs. Equal priority: an architecture where adding a future view is a
mechanical exercise (one derive + one view + one registry entry), and a visual standard that
matches the prototype (dense, consistent tokens, light + dark).

The GUI stays cross-platform: macOS, Linux, and Windows are all first-class. No
platform-specific UI code outside `main.rs` boot and the paths/keymap helpers (Cmd on macOS ↔
Ctrl elsewhere); anything platform-gated needs a reason written next to it.

The GUI also stops being a pure viewer: it can launch new recordings, locally and on remote
hosts over ssh (§8). It does so by orchestrating the `mperf` CLI, never by linking collector
code.

Non-goals for this effort: new collection capabilities (tracked separately in §10), the CLI/TUI,
and **live** view of a recording in progress — results open when the run completes.

## 2. What we are building (UX contract)

Shell, top to bottom:

1. **Title bar** — recording chip (name + scenario badge, switcher over recent recordings),
   "New profile…" button (§8) with a run chip while a recording is in progress, theme toggle.
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
      runner/                 profile launcher (§8): RunSpec, Target (Local|Ssh), process
                              orchestration on the background executor; gpui-free except spawn
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
- **Switching recordings** (title-bar switcher): one window, one session at a time.
  `Workspace::open(path)` loads the new `Session` on the background executor behind a loading
  chip in the title bar; on success it swaps the `Arc<Session>`, resets filter/selection, closes
  source tabs, keeps static-tab choice when the new scenario still offers it, and bumps a
  **session generation** — the hub drops every cached entry and any in-flight compute tagged
  with the old generation is discarded on arrival. On failure the current session stays and a
  `Dialog` shows the error; a recording that fails to load is not added to recents.

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

### 5.1 Theme v2 — a shadcn port, not an original design

A `Theme` struct registered as a gpui `Global`, with **light and dark** instances, following the
OS via `Window::observe_window_appearance` (+ manual override persisted in config, §13). The UI
look is a faithful port of the prototype's shadcn theme — token values are copied, not
reinterpreted:

- **Semantic tokens ported verbatim from `ui-prototype/src/index.css`**: `background`,
  `foreground`, `card`, `popover`, `primary`, `secondary`, `muted`, `accent`, `destructive`,
  `border`, `input`, `ring` (+ paired `*_foreground`), light and dark. Convert oklch → `Hsla`
  once (build-time script or hardcoded results with the oklch source in a comment).
- **Radius scale**: `radius = 10px` base with sm=0.6×, md=0.8×, lg=1×, xl=1.4× — widgets take
  radii from the scale, never ad-hoc pixels.
- **Interaction-state spec** (this is where the shadcn feel lives; every widget uses one shared
  helper, no per-widget improvisation): hover = `muted` wash, active = 1px downward press
  translate, focus-visible = 3px ring at `ring/50` outside the border, disabled = 50% opacity +
  no pointer events, open/expanded menus hold their hover state.
- **Fonts**: Geist Variable bundled through the gpui asset source as the UI font; the existing
  monospace stays for symbols/code/numbers.
- **Icons**: the lucide subset the prototype uses (`ui-prototype/public/icons.svg`, ~24 glyphs)
  exported as individual SVG assets and rendered via `svg()`, tinted with `text_color`.
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

Every widget is a port of the prototype's shadcn component: same variants, same sizes, same
states, colors and radii only from §5.1 tokens. When in doubt, open the corresponding
`ui-prototype/src/components/ui/*.tsx` and copy its decisions. **The kit is a hard prerequisite:
no view or shell work starts until the gallery passes parity review (M0a).**

| Widget | shadcn source | Mechanics |
|---|---|---|
| `Button` | button.tsx | variants default/outline/ghost/destructive; sizes sm/default/icon; shared interaction states |
| `TabBar` | tabs.tsx | closable tabs; reuse the proven close pattern (`stop_propagation` on mouse-down + `on_click`, `chrome.rs:119-126`); overflow scrolls; middle-click close |
| `Splitter` | resizable.tsx | per-widget hitbox drag (no global flags); powers selection panel + any future split |
| `VirtualTable` | table.tsx | `uniform_list` + sticky header via `UniformListDecoration` (the proper hook, unused today) or the proven sibling-header-in-shared-`overflow_x` container; sortable headers; column widths + resize handles (port `bottom_panel.rs:540-559` logic); cell renderers incl. inline-bar cells; scrollbar as a list decoration |
| `TextInput`, `InputGroup` | input.tsx, input-group.tsx | single-line: `FocusHandle` + `on_key_down` + IME plumbing from gpui `input.rs`; v1 scope = insert/backspace/delete/arrows/home/end/select-all; `InputGroup` adds leading icon + clear button (the symbol filter) |
| `DropdownMenu` | dropdown-menu.tsx | `deferred(anchored())` popover, checkbox rows, outside-click dismiss |
| `SegmentedControl` | toggle-group.tsx | single-select group for chart toolbars (STACKS, WEIGHT) |
| `Dialog` | dialog.tsx | modal over dimmed scrim via `deferred`; recording switcher + load errors |
| `Scrollbar` | scroll-area.tsx | thin overlay thumb, fades when idle; used by tables and scrollable panes |
| `EmptyState` | — | centered icon + one-line reason + optional action; the single way any view renders "no data" |
| `Chip`, `Badge`, `Meter`, `StatTile`, `Card`, `InfoTooltip`, `CollapsibleSection` | badge/progress/card/tooltip | small; `InfoTooltip` = `.id()`'d icon + `.tooltip(AnyView)` (the Top-Down (i) pattern from the prototype) |
| `CommandPalette` | command.tsx | M5 only; Dialog + TextInput + filtered list |

A `--gallery` debug window renders every widget and chart type with fake data in both themes —
it is where look-and-feel gets iterated without loading recordings, and it doubles as the visual
regression checklist. Gallery acceptance = side-by-side parity with the running React prototype
(both themes, hover/focus/disabled states, empty states).

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

**Degradation rules** (recordings predating C-track data, or scenarios that never record it —
gating is always data-presence driven, per feature, not per view):

- Flame WEIGHT toggle offers only weights whose data exists (no alloc data → no "alloc" option).
- Cores: without sched events, occupancy is sample-inferred and the view says so in its header
  ("inferred from samples"); with them, the label disappears.
- Source/Asm event-share chips render only for events with per-instruction attribution;
  otherwise the heat gutter alone.
- Uncore track group appears only when system-wide counters exist in the recording.
- A view with zero rows after loading renders `EmptyState` with the reason ("no memory
  observations in this recording"), never a blank pane.

C-track features that add tables/event types announce themselves via feature entries in
`info.json`, so `is_available` checks stay cheap declarations instead of probing sqlite.

## 8. Profile runner — the GUI records, locally and over ssh

The structural change: the GUI launches recordings instead of only opening them. It shells out
to the `mperf` CLI (`mperf record -s <scenario> -o <dir> [--duration …] [--pid … | -- cmd]`) —
the CLI remains the single owner of collection, permissions, and postprocessing, and the GUI
stays buildable on platforms that cannot collect (a Windows GUI drives remote Linux hosts).

### `runner/` module

- `Target = Local | Ssh { host: String }` · `RunSpec { target, scenario, command | pid, cwd,
  env, duration, output_name }` · `RunHandle { status, log_tail, cancel }`.
- Runs execute on the background executor; `RunHandle` feeds an `Entity<ActiveRun>` (title-bar
  run chip → expandable log panel: elapsed time, stderr tail, Stop button). Stop = SIGINT to the
  process group (over ssh: `ssh host kill -INT`), which lets mperf finalize the recording.
- Output lands in a managed recordings directory (platform data dir, §13) unless the user picks
  a path. On success the recording is added to recents and opened; on failure the log panel
  stays with the error.

### Local

Spawn `mperf` found next to the GUI binary or on PATH; version handshake via `mperf --version`
before the first run. Local target is hidden on platforms where the collector does not exist
(Windows).

### Ssh

- Transport is the system OpenSSH client (`ssh`/`scp` — present on macOS, Linux, Windows 10+),
  spawned with `BatchMode=yes`. The user's `~/.ssh/config`, agents, and jump hosts work for
  free; the GUI never touches passwords or keys — if auth needs interaction, the run fails with
  "set up key-based auth for <host>".
- Flow: probe remote (`uname -sm`, `mperf --version` on the remote PATH) → if mperf is missing
  or version-mismatched, upload the matching static build from the GUI's bundled toolchain dir
  (`scp`) → run record **and postprocess on the remote host** (symbols and target binaries live
  there) → pull the result directory (`scp -C`; rsync when available) into the managed
  recordings dir → open as a normal `Session`. The pulled copy notes its origin host in
  `info.json` metadata.
- Saved hosts (plus per-host mperf path override) live in user config (§13) and populate the
  target picker.

### UI (design approved in the prototype)

- **Wizard `Dialog`, three steps** — 1 · Target (selectable cards: local, saved ssh hosts,
  one-off `user@host`), 2 · Workload (launch-command vs attach; launch = command + cwd + env,
  attach = a filterable process picker sorted by CPU utilization, listed from the selected
  target via `ps`/`ssh ps`), 3 · Recording (scenario cards with blurbs, duration, and a "will
  run" summary showing the exact `mperf record` command plus provisioning/pull notes). Per-step
  validation gates Next; Start on the last step.
- **Terminal-style bottom drawer** while a run is active: header = stage stepper
  (connect → upload mperf → record → postprocess → pull results) + elapsed time, body =
  monospace log tail, footer = spec summary + Stop / Open recording / Dismiss. The drawer is an
  overlay — every view stays fully usable and the run continues in the background; closing the
  drawer never cancels the run.
- **Run chip in the title bar** (replaces the "New profile" button while a run exists) shows
  stage + elapsed and toggles the drawer; green/red states for finished/failed.

One run at a time in v1; the recording opens when the run finishes (§1 non-goal covers live
view). The runner UI was prototyped in `ui-prototype` and signed off before R1 implements it
in gpui.

## 9. Keyboard & polish (gpui actions — completely unused today)

`actions!` + `KeyBinding` + per-pane `FocusHandle`s: Cmd/Ctrl+W close tab, Ctrl+Tab / Ctrl+Shift+Tab
cycle, Cmd/Ctrl+F focus symbol input, Esc clear selection → clear filter (two-stage), ↑/↓ + Enter in
tables, ←/→/+/- pan/zoom time on the master timeline, Cmd/Ctrl+K command palette (port of the
Workbench palette: views, functions, threads, clear-filters) as the last polish item.

## 10. Milestones

All milestones land on the `gui_redesign` branch and merge to `main` **at once** when M5 is
done — the mid-rebuild view-regression window is a non-issue by construction. Each milestone is
still a separately reviewable, buildable, demoable unit.

- **M0a — shadcn port (the hard prerequisite).** Theme v2 (§5.1: semantic tokens, radius scale,
  interaction states, Geist, icons, viz tokens) + `ui/` widget kit (§5.3) + gallery window. No
  data-layer dependency; starts immediately. *Accept: gallery passes side-by-side parity review
  against the React prototype, both themes, incl. hover/focus/disabled and empty states.*
- **M0b — chart framework.** `charts/` core: ChartFrame, BrushController, HoverModel, TextCache,
  painters, the chart types with gallery pages. *Accept: gallery charts hit budgets (§11) on
  dense fake data; brush math + hit-test under `cargo test`.*
- **M0c — data plumbing.** `Rc→Arc` in `profile_analysis.rs` (own commit); `AnalysisHub` with
  background compute + LRU + generations; `Workspace`/`GlobalFilter` entities; `session/`
  reorganization. Old GUI still boots untouched. *Accept: `cargo test` covers hub
  generations/staleness + filter semantics.*
- **M1 — Shell cutover.** New chrome: title bar (incl. recording switcher + loading/error
  states), filter bar (with TextInput + dropdowns), master timeline (lanes + pinned tracks +
  brush), tab strip, selection panel (tiles + callers/callees), status bar, Hotspots view. **Old
  `views/`, old `theme.rs`, chrome state, and flamelens are deleted here.** *Accept: open a real
  recording; brush/thread/module/symbol filters rescope hotspots + lanes live; coverage stat
  correct; selection panel follows clicks; switching recordings mid-compute is safe.*
- **M2 — Core views.** Flame Graph (weights), Flame Scope, full Timeline view (all counter tracks,
  uncore group), Summary v1, dynamic Source/Asm tabs with dblclick-from-hotspots. *Accept: the
  prototype's flagship flow — brush a checkpoint, see flame reshape, dblclick `gather_neighbors`,
  read the hot loads.*
- **M3 — Analytic views.** Top-Down (hierarchy + intervals + per-function), Resources (USE v2
  cards + findings), Cores (lanes + concurrency histogram + balance). New derives land with unit
  tests against `truth` fixtures where applicable.
- **M4 — Deep dives.** Memory, Roofline. Summary blocks for both.
- **M5 — Polish.** Keyboard + palette, light-theme QA pass over every view, perf pass against
  budgets (§11), platform QA on macOS + Windows + Linux, docs/screenshots, dead-code sweep, and
  **`ui-prototype/` is deleted** — the gallery is the design reference from here on.
- **R1 — Local runner.** `runner/` module, New-profile dialog, run chip + log panel, managed
  recordings dir, config file v1 (§13). Needs M0a widgets + M1 shell; independent of M2–M4.
  *Accept: record a local run from the GUI end-to-end and land in the open recording.*
- **R2 — Ssh runner.** Remote probe/provision/pull flow, saved hosts in config. *Accept: record
  on a remote Linux host from a macOS GUI end-to-end, incl. the mperf-missing path.*
- **C-track (collector, parallel, unlocks prototype features that mock data faked):**
  1. allocation-site tracking (LD_PRELOAD) → alloc-weight flame;
  2. sched_switch/BPF occupancy → exact Cores view instead of sample-inferred;
  3. PEBS/IBS per-instruction attribution → event-share chips on asm become measured, not aggregated;
  4. uncore IMC + RAPL sampling in all scenarios → system track group everywhere.

Dependency chain: M0a → M0b → M1; M0c → M1; M1 → {M2, M3, M4, R1 in any order} → M5; R1 → R2.
C-track items are independent of the GUI chain and never block it (§7 degradation rules cover
their absence); start item 2 around M2 so Cores in M3 can render real occupancy.

## 11. Performance budgets

- Load: 1M-sample recording fully materialized < 2s on the background executor (today's loader
  already qualifies; keep it).
- Filter change: cached < 16ms to paint; cold recompute of the visible view's analyses < 300ms at
  1M samples, off-thread, previous frame held meanwhile.
- Brush drag: 60fps — preview is paint-only; zero analysis work until mouse-up.
- Hover: notifies one view entity; whole-app re-render on hover is a regression test.
- Element counts: charts are canvases; tables are `uniform_list`; nothing renders O(data) divs
  (`stack_timeline.rs` is the cautionary tale).

## 12. Testing

- L2 stays gpui-free → plain `cargo test`: derives against `truth` fixtures, filter semantics,
  hub generation/staleness, brush math, ramp/contrast helpers.
- Port and extend the existing `flame_canvas.rs` hit-test tests to the chart framework.
- The gallery window is the visual checklist (both themes, dense data, empty states).
- Manual acceptance script per milestone (recorded in this doc's PR descriptions).

## 13. Persistence & configuration

Two files, two lifecycles, both with atomic temp-file+rename writes (the `recent.rs` pattern):

- **User config — `config.toml`** in the platform config dir (`$XDG_CONFIG_HOME/mperf` on
  Linux, `~/Library/Application Support/mperf` on macOS, `%APPDATA%\mperf` on Windows;
  `MPERF_GUI_STATE_DIR` override stays for tests). Hand-editable, read at boot, documented in
  the README: theme override (`system|light|dark`), managed recordings directory, ssh hosts
  (`[[remote]] host / mperf_path`), pinned counter-track overrides per scenario, keybinding
  overrides (M5). Unknown keys warn on stderr and are preserved on rewrite, never a crash.
- **UI state — `ui-state.json`** in the platform state dir (where `recent.rs` writes today;
  `recent.rs` grows into `state/persist.rs`). Machine-written, debounced (~1s): recents, window
  geometry, panel widths, master-timeline collapsed flag, last active view per scenario. Corrupt
  or missing state falls back to defaults silently.

The managed recordings directory defaults to the platform data dir
(`$XDG_DATA_HOME/mperf/recordings` and equivalents).

## 14. Risks

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
- **gpui on Windows** is the least-exercised backend — smoke-build all three platforms in CI
  from M0a on, not at M5; platform bugs found late are redesign bugs.
- **Remote provisioning** (R2) — arch/libc mismatches for the uploaded static binary, remote
  perf permissions (`perf_event_paranoid`, macOS kperf needs sudo), and multi-GB result pulls.
  Mitigate: probe before run, surface the remote's own error text verbatim, `scp -C`/rsync, and
  a size warning before pulling.
- **Runner scope creep** — no live view, no run queue, no host fleet management in v1; one run,
  one host, results on completion.
