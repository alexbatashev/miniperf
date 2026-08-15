export interface FrameInfo {
  name: string
  module: string
  kind: "user" | "kernel" | "runtime" | "lib"
}

export interface StackInfo {
  id: number
  frames: number[]
}

export interface Sample {
  time: number
  tid: number
  cpu: number
  stackId: number
  weight: number
  instr: number
}

export interface ThreadInfo {
  tid: number
  name: string
  pid: number
}

export interface ProcessInfo {
  pid: number
  name: string
  cmdline: string
}

export interface CounterPoint {
  time: number
  value: number
}

export interface CounterSeries {
  id: string
  label: string
  unit: string
  points: CounterPoint[]
  max?: number
  uncore?: boolean
}

export interface TmaNode {
  id: string
  name: string
  short: string
  value: number
  level: number
  description: string
  children?: TmaNode[]
}

export interface TmaInterval {
  time: number
  retiring: number
  badSpec: number
  frontend: number
  backend: number
}

export interface UseMetric {
  label: string
  text: string
  frac: number | null
  kind: "util" | "sat" | "err"
}

export interface UseResource {
  id: string
  name: string
  scopeNote: string
  metrics: UseMetric[]
  utilSeries: number[]
  detail: string
}

export type FlameMetric = "cycles" | "instructions" | "alloc"

export interface RooflineCeiling {
  id: string
  label: string
  kind: "memory" | "compute"
  value: number
}

export interface RooflineLoop {
  id: string
  frameId: number
  location: string
  ai: number
  gflops: number
  timeShare: number
  tripCount: number
  vectorized: boolean
  quality: "advisor-grade" | "low-confidence"
  bound: string
  note: string
}

export interface RooflineData {
  precision: string
  ceilings: RooflineCeiling[]
  ridgeAi: number
  loops: RooflineLoop[]
}

export interface MemoryAnalysis {
  achievedGBs: number
  peakGBs: number
  roofGBs: number
  bandwidthSource: string
  bandwidthScope: string
  peakRssBytes: number
  liveAllocPeakBytes: number
  footprintBytes: number
  lineSize: number
  spatialUtilization: number
  coldFraction: number
  missRatio: { cacheBytes: number; missRatio: number }[]
  strides: { strideLog2: number; share: number }[]
  reuse: { distLog2: number; share: number }[]
  workingSet: { windowRefs: number; meanBytes: number; p95Bytes: number; maxBytes: number }[]
}

export interface AsmRow {
  addr: string
  text: string
  line: number
  samples: number
  llcMissShare?: number
  note?: string
}

export interface SourceListing {
  frameId: number
  file: string
  startLine: number
  lines: string[]
  lineSamples: number[]
  asm: AsmRow[]
  totalSamples: number
  summary: string
}

export interface ThreadStats {
  tid: number
  busyFrac: number
  syncFrac: number
  migrations: number
}

export type Scenario = "Snapshot" | "Top-Down" | "Memory" | "Roofline"

export interface SessionMeta {
  command: string
  cwd: string
  startedAt: string
  durationS: number
  sampleCount: number
  sampleFreqHz: number
  hostname: string
  cpuModel: string
  cores: number
  events: string[]
  scenario: Scenario
  views: string[]
  pinnedCounters: string[]
  flameMetrics: FlameMetric[]
  headline: { verdict: string; sub: string }
}

export interface ProfileData {
  meta: SessionMeta
  frames: FrameInfo[]
  stacks: StackInfo[]
  samples: Sample[]
  allocSamples: Sample[]
  threads: ThreadInfo[]
  processes: ProcessInfo[]
  counters: CounterSeries[]
  threadStats: ThreadStats[]
  tma?: TmaNode
  tmaIntervals?: TmaInterval[]
  tmaByFunction: Map<number, { retiring: number; badSpec: number; frontend: number; backend: number }>
  use?: UseResource[]
  roofline?: RooflineData
  memory?: MemoryAnalysis
  sources: SourceListing[]
}
