# miniperf UI prototype

React + shadcn mock of a redesigned miniperf GUI. All data is generated (seeded PRNG) —
four fake recordings of the same OpenMP particle sim on a Tiger Lake laptop, one per
scenario: Top-Down, Snapshot, Memory, Roofline. Switch recordings in the header; each
scenario gates its own view set, mirroring `mperf_data::scenario_ui`.

```sh
npm install
npm run dev
```

Layout variants (header toggle): A · Studio (VTune-like tabs + dock), B · Tracks
(nSight-like master timeline + details panel), C · Workbench (split panes + Ctrl+K).
Theme toggle next to it. Deep links:
`?recording=mem&variant=tracks&view=memory&dark=1&t0=4.5&t1=6.2&q=gather`.

Views: Summary, Hotspots (+caller/callee), Flame Graph (cycles / instructions / alloc
bytes), Flame Scope, Timeline (process counters + separated uncore track group), Cores
(per-CPU heat, concurrency histogram, thread balance), Top-Down, Resources (USE with
explicit absolute metrics), Memory (bandwidth vs capacity, miss-ratio curve, strides,
reuse), Roofline (calibrated ceilings + loop table), Source/Asm (linked source ↔
disassembly with heat gutters, no per-line metric columns).

Everything derives from one sample store through one global filter (time × threads ×
modules × symbol), so brushing any timeline rescopes every view. That single-source
architecture is the point of the prototype, not just the looks.
