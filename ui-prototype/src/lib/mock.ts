import { mulberry32, type Rng } from "./prng"
import type {
  CounterSeries,
  FrameInfo,
  MemoryAnalysis,
  ProfileData,
  RooflineData,
  Sample,
  Scenario,
  SessionMeta,
  SourceListing,
  StackInfo,
  ThreadInfo,
  ThreadStats,
  TmaInterval,
  TmaNode,
  UseResource,
} from "./model"

const DURATION = 12.6
const CHECKPOINTS = [3.9, 6.4, 8.9]
const CP_LEN = 0.38
const GIB = 1024 ** 3

function inCheckpoint(t: number): boolean {
  return CHECKPOINTS.some((c) => t >= c && t < c + CP_LEN)
}

interface Builder {
  frames: FrameInfo[]
  frameIds: Map<string, number>
  stacks: StackInfo[]
  stackIds: Map<string, number>
}

function frame(b: Builder, name: string, module: string, kind: FrameInfo["kind"]): number {
  const key = module + "!" + name
  let id = b.frameIds.get(key)
  if (id === undefined) {
    id = b.frames.length
    b.frames.push({ name, module, kind })
    b.frameIds.set(key, id)
  }
  return id
}

function stack(b: Builder, frames: number[]): number {
  const key = frames.join(",")
  let id = b.stackIds.get(key)
  if (id === undefined) {
    id = b.stacks.length
    b.stacks.push({ id, frames })
    b.stackIds.set(key, id)
  }
  return id
}

function weightedPick(rng: Rng, weights: [number, number][]): number {
  let total = 0
  for (const [, w] of weights) total += w
  let x = rng() * total
  for (const [id, w] of weights) {
    x -= w
    if (x <= 0) return id
  }
  return weights[weights.length - 1][0]
}

const phaseOf = (t: number) => (t < 1.4 ? "init" : t >= 11.2 ? "finish" : inCheckpoint(t) ? "cp" : "loop")

export function generateProfile(scenario: Scenario): ProfileData {
  const rng = mulberry32(0x5eed ^ scenario.length)
  const b: Builder = { frames: [], frameIds: new Map(), stacks: [], stackIds: new Map() }

  const P = (n: string) => frame(b, n, "physim", "user")
  const C = (n: string) => frame(b, n, "libc.so.6", "lib")
  const M = (n: string) => frame(b, n, "libm.so.6", "lib")
  const G = (n: string) => frame(b, n, "libgomp.so.1", "runtime")
  const K = (n: string) => frame(b, n, "[kernel]", "kernel")

  const main = P("main")
  const runSim = P("run_simulation")
  const simStep = P("sim_step")
  const forces = P("compute_forces")
  const gather = P("gather_neighbors")
  const hashL = P("hash_lookup")
  const cellI = P("cell_interactions")
  const vecDot = P("vec3_dot")
  const applyF = P("apply_forces")
  const integrate = P("integrate")
  const updPos = P("update_positions")
  const buildGrid = P("build_grid")
  const rebin = P("rebin_particles")
  const morton = P("morton_encode")
  const checkpoint = P("checkpoint")
  const serialize = P("serialize_state")
  const reduceS = P("reduce_stats")
  const writeOut = P("write_output")
  const parseCfg = P("parse_config")
  const loadScene = P("load_scene")
  const allocPools = P("alloc_pools")
  const expf = M("expf")
  const sqrtf = M("sqrtf")
  const memcpy = C("__memcpy_avx_unaligned")
  const memset = C("__memset_avx2_unaligned")
  const malloc = C("malloc")
  const realloc = C("realloc")
  const threadStart = G("gomp_thread_start")
  const barrier = G("gomp_team_barrier_wait")
  const futex = K("futex_wait")
  const clearPage = K("clear_page_erms")
  const allocPages = K("__alloc_pages")
  const pageFault = K("asm_exc_page_fault")
  const vfsWrite = K("vfs_write")
  const ext4Write = K("ext4_file_write_iter")
  const copyUser = K("copy_user_generic")

  const S = (...frames: number[]) => stack(b, frames)

  const IPC: Record<number, number> = {
    [gather]: 0.78, [hashL]: 0.81, [cellI]: 1.88, [vecDot]: 2.73, [expf]: 2.1, [sqrtf]: 1.78,
    [integrate]: 1.77, [updPos]: 1.77, [rebin]: 1.1, [morton]: 2.26, [memcpy]: 1.42,
    [memset]: 2.15, [futex]: 0.4, [applyF]: 2.0, [serialize]: 1.3, [clearPage]: 1.5,
  }
  const ipcOf = (sid: number): number => {
    const fr = b.stacks[sid].frames
    return IPC[fr[fr.length - 1]] ?? 1.5
  }

  const initStacks: [number, number][] = [
    [S(main, parseCfg), 6],
    [S(main, loadScene), 24],
    [S(main, loadScene, memcpy), 14],
    [S(main, loadScene, pageFault, clearPage), 9],
    [S(main, allocPools, malloc), 10],
    [S(main, allocPools, malloc, pageFault, allocPages), 8],
    [S(main, allocPools, memset), 17],
    [S(main, allocPools, memset, pageFault, clearPage), 12],
  ]

  const computeStacks = (root: number[]): [number, number][] => [
    [S(...root, forces, gather), 30],
    [S(...root, forces, gather, hashL), 16],
    [S(...root, forces, cellI), 20],
    [S(...root, forces, cellI, vecDot), 12],
    [S(...root, forces, cellI, expf), 7],
    [S(...root, forces, cellI, sqrtf), 4],
    [S(...root, forces, applyF), 5],
    [S(...root, integrate, updPos), 8],
    [S(...root, buildGrid, rebin), 5],
    [S(...root, buildGrid, rebin, morton), 3],
    [S(...root, buildGrid, memcpy), 3],
    [S(...root, barrier, futex), 6],
  ]

  const mainLoopRoot = [main, runSim, simStep]
  const workerRoot = [threadStart, simStep]
  const mainCompute = computeStacks(mainLoopRoot)
  const workerCompute = computeStacks(workerRoot)

  const cpMainStacks: [number, number][] = [
    [S(main, runSim, checkpoint, serialize), 26],
    [S(main, runSim, checkpoint, serialize, memcpy), 22],
    [S(main, runSim, checkpoint, vfsWrite, ext4Write), 24],
    [S(main, runSim, checkpoint, vfsWrite, copyUser), 16],
  ]
  const cpWorkerStacks: [number, number][] = [
    [S(threadStart, barrier, futex), 90],
    [S(threadStart, simStep, forces, gather), 10],
  ]

  const finishStacks: [number, number][] = [
    [S(main, reduceS), 30],
    [S(main, writeOut, vfsWrite, ext4Write), 30],
    [S(main, writeOut, vfsWrite, copyUser), 18],
    [S(main, writeOut, memcpy), 14],
  ]

  const threads: ThreadInfo[] = [
    { tid: 41200, name: "physim", pid: 41200 },
    ...Array.from({ length: 7 }, (_, i) => ({ tid: 41202 + i, name: `omp worker ${i + 1}`, pid: 41200 })),
  ]

  const syncStackIds = new Set<number>()
  for (const [sid] of [...cpWorkerStacks, ...mainCompute, ...workerCompute]) {
    const fr = b.stacks[sid].frames
    if (fr.includes(futex)) syncStackIds.add(sid)
  }

  const HZ = scenario === "Snapshot" ? 99 : 500
  const dt = 1 / HZ
  const samples: Sample[] = []
  const stats: ThreadStats[] = []

  for (let ti = 0; ti < threads.length; ti++) {
    const t = threads[ti]
    const isMain = ti === 0
    let cpu = ti
    let migrations = 0
    let busy = 0
    let ticks = 0
    let sync = 0
    const jitter = rng() * dt
    let wasCp = false
    for (let time = jitter; time < DURATION; time += dt) {
      let util: number
      let dist: [number, number][]
      const cp = inCheckpoint(time)
      if (cp !== wasCp) {
        if (!cp && rng() < 0.4) {
          cpu = Math.floor(rng() * 8)
          migrations++
        }
        wasCp = cp
      }
      if (rng() < 0.0006) {
        cpu = Math.floor(rng() * 8)
        migrations++
      }
      if (time < 1.4) {
        if (!isMain) continue
        util = 0.92
        dist = initStacks
      } else if (time >= 11.2) {
        if (!isMain) continue
        util = 0.85
        dist = finishStacks
      } else if (cp) {
        util = isMain ? 0.95 : 0.3
        dist = isMain ? cpMainStacks : cpWorkerStacks
      } else {
        util = isMain ? 0.93 : ti === 7 ? 0.72 : 0.9
        dist = isMain ? mainCompute : workerCompute
        if (ti === 7) dist = [...dist, [workerCompute[11][0], 26]]
      }
      ticks++
      if (rng() > util) continue
      busy++
      const gatherBoost = 1 + 0.5 * Math.sin((time - 1.4) * 1.9)
      const adjusted: [number, number][] = dist.map(([sid, w]) => {
        const leafFrames = b.stacks[sid].frames
        return [sid, leafFrames.includes(gather) ? w * gatherBoost : w]
      })
      const stackId = weightedPick(rng, adjusted)
      if (syncStackIds.has(stackId)) sync++
      samples.push({
        time,
        tid: t.tid,
        cpu,
        stackId,
        weight: 1,
        instr: ipcOf(stackId) * (0.9 + rng() * 0.2),
      })
    }
    stats.push({ tid: t.tid, busyFrac: ticks ? busy / ticks : 0, syncFrac: busy ? sync / busy : 0, migrations })
  }
  samples.sort((a, c) => a.time - c.time)

  const allocSamples: Sample[] = []
  if (scenario === "Memory") {
    const allocStacks: [number, number, number][] = [
      [S(main, allocPools, malloc), 4, 64 * 1024 * 1024],
      [S(main, loadScene, malloc), 2, 24 * 1024 * 1024],
      [S(threadStart, simStep, buildGrid, rebin, realloc), 0.7, 2 * 1024 * 1024],
      [S(main, runSim, simStep, buildGrid, rebin, realloc), 0.5, 2 * 1024 * 1024],
      [S(main, runSim, checkpoint, serialize, malloc), 1.2, 16 * 1024 * 1024],
    ]
    for (let time = 0.02; time < DURATION; time += 0.01) {
      const ph = phaseOf(time)
      for (const [sid, rate, bytes] of allocStacks) {
        const frames = b.stacks[sid].frames
        const isInit = frames.includes(allocPools) || frames.includes(loadScene)
        const isCp = frames.includes(serialize)
        const active = isInit ? ph === "init" : isCp ? ph === "cp" : ph === "loop"
        if (!active) continue
        if (rng() < rate * 0.1) {
          allocSamples.push({
            time,
            tid: isInit || isCp ? threads[0].tid : threads[1 + Math.floor(rng() * 7)].tid,
            cpu: 0,
            stackId: sid,
            weight: bytes * (0.6 + rng() * 0.8),
            instr: 0,
          })
        }
      }
    }
  }

  const tmaByFunction = new Map<number, { retiring: number; badSpec: number; frontend: number; backend: number }>()
  const put = (fid: number, r: number, bs: number, fe: number, be: number) =>
    tmaByFunction.set(fid, { retiring: r, badSpec: bs, frontend: fe, backend: be })
  put(gather, 0.18, 0.05, 0.06, 0.71)
  put(hashL, 0.22, 0.19, 0.09, 0.5)
  put(cellI, 0.52, 0.07, 0.09, 0.32)
  put(vecDot, 0.61, 0.04, 0.08, 0.27)
  put(expf, 0.58, 0.03, 0.12, 0.27)
  put(integrate, 0.44, 0.05, 0.08, 0.43)
  put(updPos, 0.41, 0.04, 0.07, 0.48)
  put(rebin, 0.3, 0.12, 0.11, 0.47)
  put(morton, 0.55, 0.08, 0.13, 0.24)
  put(memcpy, 0.35, 0.02, 0.04, 0.59)
  put(serialize, 0.4, 0.08, 0.11, 0.41)

  const scenarioConf: Record<Scenario, Pick<SessionMeta, "command" | "events" | "views" | "pinnedCounters" | "flameMetrics" | "headline">> = {
    "Top-Down": {
      command: "mperf record -s tma -o results/physim-04 -- ./physim --particles 2000000 --steps 1200",
      events: [
        "pmu_cycles", "pmu_instructions", "pmu_llc_references", "pmu_llc_misses",
        "pmu_branch_instructions", "pmu_branch_misses", "TOPDOWN.SLOTS_P", "IDQ_UOPS_NOT_DELIVERED.CORE",
      ],
      views: ["summary", "hotspots", "flamegraph", "flamescope", "callers", "timeline", "cores", "tma", "source"],
      pinnedCounters: ["derived.ipc", "stalled_be", "uncore_dram_read"],
      flameMetrics: ["cycles", "instructions"],
      headline: { verdict: "Backend Bound", sub: "46% of slots · Memory Bound 33% · mostly DRAM" },
    },
    Snapshot: {
      command: "mperf record -s snapshot -o results/physim-snap-01 -- ./physim --particles 2000000 --steps 1200",
      events: ["os_cpu_clock", "os_page_faults", "os_context_switches", "uncore: imc_read, imc_write", "power: package_energy"],
      views: ["summary", "use", "cores", "hotspots", "flamegraph", "timeline"],
      pinnedCounters: ["cpu_util", "uncore_dram_read", "uncore_pkg_power"],
      flameMetrics: ["cycles"],
      headline: { verdict: "DRAM bandwidth saturates", sub: "peak 79% of 45.8 GB/s roof · CPU 86% busy in compute" },
    },
    Memory: {
      command: "mperf record -s mem -o results/physim-mem-02 -- ./physim --particles 2000000 --steps 1200",
      events: ["pmu_cycles", "pmu_instructions", "alloc tracking (LD_PRELOAD)", "uncore: imc_read, imc_write", "qemu address trace"],
      views: ["summary", "memory", "hotspots", "flamegraph", "timeline", "cores", "source"],
      pinnedCounters: ["uncore_dram_read", "rss", "allocs"],
      flameMetrics: ["cycles", "instructions", "alloc"],
      headline: { verdict: "Bandwidth-limited, capacity fine", sub: "24.1 avg / 36.2 peak of 45.8 GB/s · RSS 6.2 of 31.1 GiB" },
    },
    Roofline: {
      command: "mperf record -s roofline -o results/physim-roofline-01 -- ./physim --particles 2000000 --steps 1200",
      events: ["pmu_cycles", "pmu_instructions", "qemu op/byte accounting", "loop discovery", "calibration: fp64 + memory levels"],
      views: ["summary", "roofline", "memory", "hotspots", "flamegraph", "cores", "source"],
      pinnedCounters: ["gflops", "uncore_dram_read", "uncore_pkg_power"],
      flameMetrics: ["cycles", "instructions"],
      headline: { verdict: "Hot loops sit on the DRAM roof", sub: "AI 0.06–0.28 flops/byte · vectorization won't help until locality does" },
    },
  }
  const conf = scenarioConf[scenario]

  const meta: SessionMeta = {
    command: conf.command,
    cwd: "~/work/physim",
    startedAt: "2026-08-15 14:02:31",
    durationS: DURATION,
    sampleCount: samples.length,
    sampleFreqHz: HZ,
    hostname: "tgl-devbox",
    cpuModel: "11th Gen Intel Core i7-1185G7 @ 3.00GHz",
    cores: 8,
    events: conf.events,
    scenario,
    views: conf.views,
    pinnedCounters: conf.pinnedCounters,
    flameMetrics: conf.flameMetrics,
    headline: conf.headline,
  }

  return {
    meta,
    frames: b.frames,
    stacks: b.stacks,
    samples,
    allocSamples,
    threads,
    processes: [{ pid: 41200, name: "physim", cmdline: "./physim --particles 2000000 --steps 1200" }],
    counters: buildCounters(rng, scenario),
    threadStats: stats,
    tma: scenario === "Top-Down" ? buildTma() : undefined,
    tmaIntervals: scenario === "Top-Down" ? buildTmaIntervals(rng) : undefined,
    tmaByFunction,
    use: scenario === "Snapshot" ? buildUse(rng) : undefined,
    roofline: scenario === "Roofline" ? buildRoofline(gather, cellI, integrate, vecDot, rebin, reduceS) : undefined,
    memory: scenario === "Memory" || scenario === "Roofline" ? buildMemory() : undefined,
    sources: scenario === "Snapshot" ? [] : buildSources(gather, vecDot),
  }
}

function buildCounters(rng: Rng, scenario: Scenario): CounterSeries[] {
  const step = 0.1
  const n = Math.floor(DURATION / step)
  const mk = (
    id: string, label: string, unit: string,
    fn: (t: number) => number, noise: number, max?: number, uncore?: boolean
  ): CounterSeries => ({
    id, label, unit, max, uncore,
    points: Array.from({ length: n }, (_, i) => {
      const t = i * step
      return { time: t, value: Math.max(0, fn(t) * (1 + (rng() - 0.5) * noise)) }
    }),
  })
  const wave = (t: number) => 1 + 0.4 * Math.sin((t - 1.4) * 1.9)

  const dramRead = mk("uncore_dram_read", "IMC read", "GB/s", (t) => {
    switch (phaseOf(t)) {
      case "init": return 5.1
      case "finish": return 2.4
      case "cp": return 7.2
      default: return 18 * wave(t)
    }
  }, 0.09, 45.8, true)
  const dramWrite = mk("uncore_dram_write", "IMC write", "GB/s", (t) => {
    switch (phaseOf(t)) {
      case "init": return 3.8
      case "finish": return 1.9
      case "cp": return 4.8
      default: return 6.2 * wave(t) * 0.8
    }
  }, 0.11, 45.8, true)
  const pkgPower = mk("uncore_pkg_power", "Package power", "W", (t) => {
    switch (phaseOf(t)) {
      case "init": return 21
      case "finish": return 18
      case "cp": return 26
      default: return 39
    }
  }, 0.05, 45, true)
  const uncore = [dramRead, dramWrite, pkgPower]

  const rss = mk("rss", "RSS", "GiB", (t) => (t < 1.4 ? 0.4 + (t / 1.4) * 5.8 : inCheckpoint(t) ? 6.5 : 6.2), 0.01, 8)
  const pageFaults = mk("os_page_faults", "Page faults", "/s",
    (t) => (phaseOf(t) === "init" ? 48e3 : phaseOf(t) === "cp" ? 6e3 : 400), 0.25)

  if (scenario === "Snapshot") {
    return [
      mk("cpu_util", "CPU utilization", "%", (t) => {
        switch (phaseOf(t)) {
          case "init": return 13
          case "finish": return 12
          case "cp": return 42
          default: return 93
        }
      }, 0.05, 100),
      pageFaults,
      mk("ctx_switches", "Context switches", "/s",
        (t) => (phaseOf(t) === "cp" ? 21e3 : phaseOf(t) === "loop" ? 3.4e3 : 900), 0.2),
      rss,
      ...uncore,
    ]
  }
  if (scenario === "Memory") {
    return [
      rss,
      mk("live_alloc", "Live allocated", "GiB", (t) => (t < 1.4 ? 0.3 + (t / 1.4) * 5.4 : inCheckpoint(t) ? 6.1 : 5.8), 0.01, 8),
      mk("allocs", "Allocations", "/s", (t) => (phaseOf(t) === "init" ? 8200 : phaseOf(t) === "cp" ? 2400 : 310), 0.3),
      pageFaults,
      ...uncore,
    ]
  }
  if (scenario === "Roofline") {
    return [
      mk("gflops", "FP64 throughput", "GFLOP/s", (t) => (phaseOf(t) === "loop" ? 26 / wave(t) : 1.2), 0.1, 76.8),
      mk("pmu_instructions", "Instructions", "/s", (t) => (phaseOf(t) === "loop" ? 34e9 / wave(t) : 3e9), 0.07),
      ...uncore,
    ]
  }
  return [
    mk("derived.ipc", "IPC (derived)", "", (t) => {
      switch (phaseOf(t)) {
        case "init": return 0.9
        case "finish": return 0.75
        case "cp": return 0.55
        default: return 1.7 - 0.45 * (wave(t) - 1) * 2
      }
    }, 0.08, 4),
    mk("pmu_cycles", "Cycles", "/s",
      (t) => (phaseOf(t) === "init" || phaseOf(t) === "finish" ? 3.1e9 : phaseOf(t) === "cp" ? 8.2e9 : 22e9), 0.06),
    mk("pmu_instructions", "Instructions", "/s", (t) => {
      switch (phaseOf(t)) {
        case "init": return 2.8e9
        case "finish": return 2.3e9
        case "cp": return 4.5e9
        default: return 34e9 / wave(t)
      }
    }, 0.07),
    mk("pmu_llc_misses", "LLC misses", "/s", (t) => {
      switch (phaseOf(t)) {
        case "init": return 9e6
        case "finish": return 4e6
        case "cp": return 14e6
        default: return 52e6 * wave(t)
      }
    }, 0.12),
    mk("llc_miss_rate", "LLC miss rate", "%", (t) => (phaseOf(t) === "loop" ? 34 * wave(t) * 0.7 : 12), 0.1, 100),
    mk("branch_mpki", "Branch MPKI", "", (t) => (phaseOf(t) === "loop" ? 4.2 : 7.5), 0.15),
    mk("stalled_be", "Backend stall", "%", (t) => {
      switch (phaseOf(t)) {
        case "init": return 35
        case "finish": return 42
        case "cp": return 58
        default: return 44 + 18 * (wave(t) - 1) * 2
      }
    }, 0.07, 100),
    pageFaults,
    ...uncore,
  ]
}

function buildTma(): TmaNode {
  const node = (id: string, name: string, short: string, value: number, level: number, description: string, children?: TmaNode[]): TmaNode =>
    ({ id, name, short, value, level, description, children })
  return node("root", "Pipeline slots", "Slots", 1, 0, "", [
    node("retiring", "Retiring", "Ret", 0.328, 1, "Slots that retired useful uops. Higher is better — but high retiring with low vectorization still leaves throughput on the table."),
    node("bad_speculation", "Bad Speculation", "BadSpec", 0.094, 1, "Slots wasted on mispredicted branches and machine clears.", [
      node("branch_mispredicts", "Branch Mispredicts", "BrMisp", 0.081, 2, "Wasted work from mispredicted branches; dominated by hash_lookup probe loop."),
      node("machine_clears", "Machine Clears", "Clears", 0.013, 2, "Pipeline flushes from memory ordering or self-modifying code."),
    ]),
    node("fe_bound", "Frontend Bound", "FE", 0.118, 1, "Slots where the frontend undersupplied the backend.", [
      node("fetch_latency", "Fetch Latency", "FetchLat", 0.074, 2, "Instruction cache and iTLB misses, branch resteers."),
      node("fetch_bandwidth", "Fetch Bandwidth", "FetchBW", 0.044, 2, "Decoder and uop-cache delivery limits."),
    ]),
    node("be_bound", "Backend Bound", "BE", 0.46, 1, "Slots stalled on data or execution resources. Dominant category for this run.", [
      node("be_bound_memory_bound", "Memory Bound", "Mem", 0.331, 2, "Stalls waiting for loads through the cache hierarchy — gather_neighbors random access pattern misses LLC.", [
        node("dram_bound", "DRAM Bound", "DRAM", 0.212, 3, "Loads serviced from DRAM; bandwidth peaks at 79% of calibrated roof."),
        node("l3_bound", "L3 Bound", "L3", 0.077, 3, "Loads hitting L3 with contention from 8 hardware threads."),
        node("store_bound", "Store Bound", "Store", 0.042, 3, "Store buffer full during checkpoint serialization."),
      ]),
      node("be_bound_core_bound", "Core Bound", "Core", 0.129, 2, "Execution port pressure and long-latency ops (expf, sqrtf, divides)."),
    ]),
  ])
}

function buildTmaIntervals(rng: Rng): TmaInterval[] {
  const out: TmaInterval[] = []
  for (let t = 0.5; t < DURATION; t += 1) {
    let r: number, bs: number, fe: number, be: number
    if (t < 1.4) { r = 0.28; bs = 0.08; fe = 0.22; be = 0.42 }
    else if (t >= 11.2) { r = 0.24; bs = 0.07; fe = 0.15; be = 0.54 }
    else if (inCheckpoint(t)) { r = 0.22; bs = 0.06; fe = 0.14; be = 0.58 }
    else {
      const wave = 0.5 * Math.sin((t - 1.4) * 1.9)
      r = 0.36 - 0.07 * wave; bs = 0.1; fe = 0.11; be = 0.43 + 0.07 * wave
    }
    const jitter = () => 1 + (rng() - 0.5) * 0.08
    r *= jitter(); bs *= jitter(); fe *= jitter(); be *= jitter()
    const sum = r + bs + fe + be
    out.push({ time: t, retiring: r / sum, badSpec: bs / sum, frontend: fe / sum, backend: be / sum })
  }
  return out
}

function buildUse(rng: Rng): UseResource[] {
  const n = Math.ceil(DURATION)
  const series = (fn: (t: number) => number, noise = 0.1) =>
    Array.from({ length: n }, (_, i) => Math.min(1, Math.max(0, fn(i + 0.5) * (1 + (rng() - 0.5) * noise))))
  return [
    {
      id: "cpu",
      name: "CPU",
      scopeNote: "process tree · procfs + PSI",
      metrics: [
        { label: "Utilization", text: "6.9 of 8 logical CPUs (86%)", frac: 0.86, kind: "util" },
        { label: "Saturation", text: "run-queue PSI some avg10 4.1%", frac: 0.041, kind: "sat" },
        { label: "Errors", text: "0 throttle events", frac: 0, kind: "err" },
      ],
      utilSeries: series((t) => (t < 1.4 ? 0.13 : t >= 11.2 ? 0.12 : inCheckpoint(t) ? 0.42 : 0.93)),
      detail: "Compute phase keeps 7 of 8 CPUs busy; drops to 2.4 CPUs during checkpoints (workers blocked on barrier).",
    },
    {
      id: "membw",
      name: "Memory · bandwidth",
      scopeNote: "system during target · uncore IMC",
      metrics: [
        { label: "Utilization", text: "avg 21.4 of 45.8 GB/s roof (47%)", frac: 0.47, kind: "util" },
        { label: "Peak", text: "36.2 GB/s (79% of roof)", frac: 0.79, kind: "util" },
        { label: "Saturation", text: "memory PSI some avg10 7.8%", frac: 0.078, kind: "sat" },
      ],
      utilSeries: series((t) => (t < 1.4 ? 0.2 : 0.42 + 0.18 * Math.sin((t - 1.4) * 1.9))),
      detail: "Read-dominated: 18 GB/s read vs 5 GB/s write in compute phase. Roof from calibrated STREAM-like sweep.",
    },
    {
      id: "memcap",
      name: "Memory · capacity",
      scopeNote: "process tree · smaps_rollup",
      metrics: [
        { label: "Utilization", text: "RSS 6.2 of 31.1 GiB (20%)", frac: 0.2, kind: "util" },
        { label: "Saturation", text: "swap 0 B · major-fault burst at start", frac: 0.04, kind: "sat" },
        { label: "Errors", text: "0 OOM events", frac: 0, kind: "err" },
      ],
      utilSeries: series((t) => (t < 1.4 ? 0.02 + (t / 1.4) * 0.18 : 0.2)),
      detail: "Footprint stable after init; capacity is not the constraint — bandwidth is.",
    },
    {
      id: "disk",
      name: "Disk · nvme0n1",
      scopeNote: "system during target · diskstats",
      metrics: [
        { label: "Throughput", text: "peak 412 MB/s of ~3.5 GB/s seq (12%)", frac: 0.12, kind: "util" },
        { label: "Busy", text: "78% busy during checkpoints", frac: 0.78, kind: "util" },
        { label: "Saturation", text: "weighted IO time 340 ms spikes · block p99 1.8 ms", frac: 0.35, kind: "sat" },
      ],
      utilSeries: series((t) => (inCheckpoint(t) || t >= 11.6 ? 0.78 : 0.01), 0.2),
      detail: "Device is far from throughput limits; the cost is the synchronous wait, not the drive.",
    },
    {
      id: "power",
      name: "Power · package",
      scopeNote: "system · RAPL (uncore)",
      metrics: [
        { label: "Utilization", text: "39 W avg of 45 W PL1 (87%)", frac: 0.87, kind: "util" },
        { label: "Saturation", text: "0 s frequency-limited", frac: 0, kind: "sat" },
        { label: "Errors", text: "0 thermal events", frac: 0, kind: "err" },
      ],
      utilSeries: series((t) => (phaseOf(t) === "loop" ? 0.87 : phaseOf(t) === "cp" ? 0.58 : 0.45), 0.04),
      detail: "Near PL1 in compute phase — little headroom to clock up; efficiency gains must come from IPC.",
    },
    {
      id: "network",
      name: "Network · eno1",
      scopeNote: "system during target · netdev",
      metrics: [
        { label: "Utilization", text: "<0.1% of 1 Gb/s", frac: 0.001, kind: "util" },
        { label: "Saturation", text: "0 drops, 0 retransmits", frac: 0, kind: "sat" },
        { label: "Errors", text: "0", frac: 0, kind: "err" },
      ],
      utilSeries: series(() => 0.01, 0.8),
      detail: "Idle.",
    },
  ]
}

function buildRoofline(gather: number, cellI: number, integrate: number, vecDot: number, rebin: number, reduceS: number): RooflineData {
  return {
    precision: "FP64",
    ceilings: [
      { id: "l1", label: "L1 880 GB/s", kind: "memory", value: 880 },
      { id: "l2", label: "L2 465 GB/s", kind: "memory", value: 465 },
      { id: "l3", label: "L3 218 GB/s", kind: "memory", value: 218 },
      { id: "dram", label: "DRAM 45.8 GB/s", kind: "memory", value: 45.8 },
      { id: "fma", label: "FP64 vector FMA 76.8 GFLOP/s", kind: "compute", value: 76.8 },
      { id: "scalar", label: "FP64 scalar 9.6 GFLOP/s", kind: "compute", value: 9.6 },
    ],
    ridgeAi: 76.8 / 45.8,
    loops: [
      { id: "gather", frameId: gather, location: "physim.c:187", ai: 0.06, gflops: 2.6, timeShare: 0.34, tripCount: 2.1e9, vectorized: false, quality: "advisor-grade", bound: "DRAM", note: "Pointer chase through hash grid; almost no FP work to hide latency. Locality first, vectorization later." },
      { id: "cell", frameId: cellI, location: "physim.c:214", ai: 0.28, gflops: 11.9, timeShare: 0.31, tripCount: 2.4e9, vectorized: true, quality: "advisor-grade", bound: "DRAM", note: "8-wide FMA pair loop. Sits on the DRAM roof — feeding it is the problem, not the math." },
      { id: "vecdot", frameId: vecDot, location: "vec.h:44", ai: 0.5, gflops: 21.4, timeShare: 0.12, tripCount: 9.6e9, vectorized: true, quality: "low-confidence", bound: "L2", note: "Inlined kernel; attribution split across callers. Near L2 roof at its intensity." },
      { id: "integrate", frameId: integrate, location: "physim.c:301", ai: 0.11, gflops: 4.9, timeShare: 0.09, tripCount: 1.2e9, vectorized: true, quality: "advisor-grade", bound: "DRAM", note: "Streaming update; already saturates its share of bandwidth." },
      { id: "rebin", frameId: rebin, location: "grid.c:98", ai: 0.05, gflops: 1.6, timeShare: 0.07, tripCount: 8.4e8, vectorized: false, quality: "low-confidence", bound: "DRAM", note: "Scatter into cells; integer-heavy, counted ops are addresses not math." },
      { id: "reduce", frameId: reduceS, location: "stats.c:31", ai: 2.1, gflops: 7.2, timeShare: 0.02, tripCount: 2.0e6, vectorized: false, quality: "advisor-grade", bound: "scalar FP", note: "Compute-bound but scalar — the one loop where vectorizing directly pays." },
    ],
  }
}

function buildMemory(): MemoryAnalysis {
  return {
    achievedGBs: 24.1,
    peakGBs: 36.2,
    roofGBs: 45.8,
    bandwidthSource: "hardware_memory_controller",
    bandwidthScope: "system_during_target",
    peakRssBytes: 6.6 * GIB,
    liveAllocPeakBytes: 5.9 * GIB,
    footprintBytes: 5.1 * GIB,
    lineSize: 64,
    spatialUtilization: 0.41,
    coldFraction: 0.09,
    missRatio: [
      { cacheBytes: 32 * 1024, missRatio: 0.38 },
      { cacheBytes: 64 * 1024, missRatio: 0.33 },
      { cacheBytes: 128 * 1024, missRatio: 0.3 },
      { cacheBytes: 256 * 1024, missRatio: 0.26 },
      { cacheBytes: 512 * 1024, missRatio: 0.24 },
      { cacheBytes: 1 * 1024 * 1024, missRatio: 0.215 },
      { cacheBytes: 2 * 1024 * 1024, missRatio: 0.19 },
      { cacheBytes: 4 * 1024 * 1024, missRatio: 0.16 },
      { cacheBytes: 8 * 1024 * 1024, missRatio: 0.12 },
      { cacheBytes: 16 * 1024 * 1024, missRatio: 0.071 },
      { cacheBytes: 32 * 1024 * 1024, missRatio: 0.031 },
      { cacheBytes: 64 * 1024 * 1024, missRatio: 0.012 },
    ],
    strides: [
      { strideLog2: -3, share: 0.02 },
      { strideLog2: -1, share: 0.05 },
      { strideLog2: 0, share: 0.11 },
      { strideLog2: 1, share: 0.31 },
      { strideLog2: 2, share: 0.05 },
      { strideLog2: 3, share: 0.02 },
      { strideLog2: 6, share: 0.09 },
      { strideLog2: 9, share: 0.14 },
      { strideLog2: 12, share: 0.21 },
    ],
    reuse: [
      { distLog2: 0, share: 0.18 },
      { distLog2: 2, share: 0.09 },
      { distLog2: 4, share: 0.07 },
      { distLog2: 6, share: 0.06 },
      { distLog2: 8, share: 0.07 },
      { distLog2: 10, share: 0.09 },
      { distLog2: 12, share: 0.13 },
      { distLog2: 14, share: 0.17 },
      { distLog2: 16, share: 0.14 },
    ],
    workingSet: [
      { windowRefs: 1e6, meanBytes: 18 * 1024 * 1024, p95Bytes: 34 * 1024 * 1024, maxBytes: 61 * 1024 * 1024 },
      { windowRefs: 1e7, meanBytes: 210 * 1024 * 1024, p95Bytes: 480 * 1024 * 1024, maxBytes: 0.9 * GIB },
      { windowRefs: 1e8, meanBytes: 1.9 * GIB, p95Bytes: 3.4 * GIB, maxBytes: 4.8 * GIB },
    ],
  }
}

function buildSources(gather: number, vecDot: number): SourceListing[] {
  const gatherSrc: SourceListing = {
    frameId: gather,
    file: "src/physim.c",
    startLine: 182,
    lines: [
      "static void gather_neighbors(const grid_t *g, const particle_t *p,",
      "                             neighbor_list_t *out, int i) {",
      "  const cell_id cid = cell_of(g, p[i].pos);",
      "  int n = 0;",
      "  for (int dz = -1; dz <= 1; dz++)",
      "    for (int dy = -1; dy <= 1; dy++)",
      "      for (int dx = -1; dx <= 1; dx++) {",
      "        cell_id c = cid + cell_offset(g, dx, dy, dz);",
      "        uint32_t idx = g->head[c];",
      "        while (idx != EMPTY_CELL) {",
      "          const particle_t *q = &p[idx];",
      "          float d2 = dist2(p[i].pos, q->pos);",
      "          if (d2 < g->cutoff2)",
      "            out->idx[n++] = idx;",
      "          idx = g->next[idx];",
      "        }",
      "      }",
      "  out->count = n;",
      "}",
    ],
    lineSamples: [0, 0, 0.02, 0, 0.01, 0.01, 0.02, 0.03, 0.14, 0.04, 0.3, 0.12, 0.03, 0.02, 0.26, 0, 0, 0, 0],
    asm: [
      { addr: "1a40", text: "push   %r15", line: 183, samples: 0 },
      { addr: "1a42", text: "mov    0x8(%rsi,%rcx,1),%xmm0", line: 184, samples: 0.01 },
      { addr: "1a48", text: "call   cell_of", line: 184, samples: 0.01 },
      { addr: "1a4d", text: "xor    %r14d,%r14d", line: 185, samples: 0 },
      { addr: "1a50", text: "mov    $0xffffffff,%r13d", line: 186, samples: 0 },
      { addr: "1a58", text: "lea    (%rax,%rdx,1),%ecx", line: 189, samples: 0.02 },
      { addr: "1a5b", text: "mov    0x18(%rdi),%rax", line: 190, samples: 0.01 },
      { addr: "1a5f", text: "mov    (%rax,%rcx,4),%ebx", line: 190, samples: 0.11, llcMissShare: 0.29, note: "head[] load — one random line per cell" },
      { addr: "1a62", text: "cmp    $0xffffffff,%ebx", line: 191, samples: 0.02 },
      { addr: "1a65", text: "je     1ab0", line: 191, samples: 0.02 },
      { addr: "1a67", text: "mov    %ebx,%eax", line: 192, samples: 0.01 },
      { addr: "1a69", text: "shl    $0x5,%rax", line: 192, samples: 0.01 },
      { addr: "1a6d", text: "movups (%r9,%rax,1),%xmm1", line: 192, samples: 0.24, llcMissShare: 0.38, note: "particle load — 32 B stride-random, misses LLC" },
      { addr: "1a72", text: "subps  %xmm0,%xmm1", line: 193, samples: 0.04 },
      { addr: "1a75", text: "mulps  %xmm1,%xmm1", line: 193, samples: 0.03 },
      { addr: "1a78", text: "haddps %xmm1,%xmm1", line: 193, samples: 0.03 },
      { addr: "1a7c", text: "comiss %xmm2,%xmm1", line: 194, samples: 0.02 },
      { addr: "1a7f", text: "jae    1a8c", line: 194, samples: 0.02 },
      { addr: "1a81", text: "mov    %ebx,(%r8,%r14,4)", line: 195, samples: 0.02 },
      { addr: "1a85", text: "inc    %r14d", line: 195, samples: 0.01 },
      { addr: "1a8c", text: "mov    0x20(%rdi),%rax", line: 196, samples: 0.01 },
      { addr: "1a90", text: "mov    (%rax,%rbx,4),%ebx", line: 196, samples: 0.22, llcMissShare: 0.33, note: "next[] pointer chase — serialized, latency-bound" },
      { addr: "1a93", text: "cmp    $0xffffffff,%ebx", line: 191, samples: 0.02 },
      { addr: "1a96", text: "jne    1a67", line: 191, samples: 0.02 },
      { addr: "1ab0", text: "mov    %r14d,0x80(%r8)", line: 199, samples: 0.01 },
      { addr: "1ab7", text: "ret", line: 200, samples: 0 },
    ],
    totalSamples: 7352,
    summary:
      "Three loads carry 100% of this function's LLC misses: head[c], the particle body, and next[idx]. All three are dependent random accesses — the classic linked-cell traversal. Restructuring to sorted cell-contiguous particles (SoA) turns two of them into streaming loads.",
  }

  const vecSrc: SourceListing = {
    frameId: vecDot,
    file: "src/vec.h",
    startLine: 40,
    lines: [
      "static inline float vec3_dot_acc(const float *restrict a,",
      "                                 const float *restrict b, int n) {",
      "  float acc = 0.f;",
      "  #pragma omp simd reduction(+:acc)",
      "  for (int k = 0; k < n; k++)",
      "    acc += a[k] * b[k];",
      "  return acc;",
      "}",
    ],
    lineSamples: [0, 0, 0.01, 0, 0.08, 0.88, 0.03, 0],
    asm: [
      { addr: "0c20", text: "vxorps %ymm0,%ymm0,%ymm0", line: 42, samples: 0.01 },
      { addr: "0c24", text: "vmovups (%rdi,%rax,4),%ymm1", line: 45, samples: 0.16 },
      { addr: "0c29", text: "vfmadd231ps (%rsi,%rax,4),%ymm1,%ymm0", line: 45, samples: 0.62, note: "8-wide FMA — retiring-dominated, this is healthy" },
      { addr: "0c2f", text: "add    $0x8,%rax", line: 44, samples: 0.05 },
      { addr: "0c33", text: "cmp    %rax,%rdx", line: 44, samples: 0.03 },
      { addr: "0c36", text: "jg     0c24", line: 44, samples: 0.05 },
      { addr: "0c38", text: "vextractf128 $1,%ymm0,%xmm1", line: 46, samples: 0.03 },
      { addr: "0c3e", text: "vaddps %xmm1,%xmm0,%xmm0", line: 46, samples: 0.03 },
      { addr: "0c42", text: "ret", line: 46, samples: 0.02 },
    ],
    totalSamples: 3041,
    summary:
      "Already vectorized 8-wide and retiring-dominated. No action needed here — time spent is proportional to useful work. Contrast with gather_neighbors.",
  }
  return [gatherSrc, vecSrc]
}

export interface UseFinding {
  rank: number
  severity: "high" | "medium" | "info"
  resource: string
  finding: string
  evidence: string
  recommendation: string
}

export const USE_FINDINGS: UseFinding[] = [
  {
    rank: 1,
    severity: "high",
    resource: "memory bw",
    finding: "DRAM bandwidth periodically saturates",
    evidence: "Read bandwidth peaks at 36.2 GB/s (79% of 45.8 GB/s calibrated roof); memory PSI some avg10 reaches 7.8%",
    recommendation: "gather_neighbors dominates DRAM traffic — improve locality (sort particles by cell, SoA layout) or software prefetch",
  },
  {
    rank: 2,
    severity: "medium",
    resource: "cpu",
    finding: "Worker imbalance at barriers",
    evidence: "omp worker 7 spends 27% of loop time in futex_wait vs 8% median",
    recommendation: "Switch to dynamic scheduling for cell iteration or rebalance grid partitioning",
  },
  {
    rank: 3,
    severity: "medium",
    resource: "disk",
    finding: "Checkpoints stall all workers",
    evidence: "3 checkpoint windows of ~380ms; worker utilization drops to 30% while main thread serializes",
    recommendation: "Double-buffer checkpoint state and write asynchronously",
  },
  {
    rank: 4,
    severity: "info",
    resource: "memory cap",
    finding: "Initialization page-fault burst",
    evidence: "48k faults/s during first 1.4s (pool allocation touches 6.2 GiB)",
    recommendation: "Pre-fault with MAP_POPULATE or huge pages if startup latency matters",
  },
]

const cache = new Map<Scenario, ProfileData>()
export function getProfile(scenario: Scenario = "Top-Down"): ProfileData {
  let d = cache.get(scenario)
  if (!d) {
    d = generateProfile(scenario)
    cache.set(scenario, d)
  }
  return d
}
