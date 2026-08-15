import type { FlameMetric, ProfileData, Sample } from "./model"

export interface FlameNode {
  frameId: number
  total: number
  self: number
  children: Map<number, FlameNode>
}

export function metricWeight(metric: FlameMetric): (s: Sample) => number {
  return metric === "instructions" ? (s) => s.instr : (s) => s.weight
}

export function buildFlameTree(
  data: ProfileData,
  samples: Sample[],
  inverted = false,
  metric: FlameMetric = "cycles"
): FlameNode {
  const weight = metricWeight(metric)
  const root: FlameNode = { frameId: -1, total: 0, self: 0, children: new Map() }
  for (const s of samples) {
    const frames = data.stacks[s.stackId].frames
    const w = weight(s)
    root.total += w
    let node = root
    const n = frames.length
    for (let i = 0; i < n; i++) {
      const fid = inverted ? frames[n - 1 - i] : frames[i]
      let child = node.children.get(fid)
      if (!child) {
        child = { frameId: fid, total: 0, self: 0, children: new Map() }
        node.children.set(fid, child)
      }
      child.total += w
      node = child
    }
    node.self += w
  }
  return root
}

export interface HotspotRow {
  frameId: number
  self: number
  total: number
  selfPct: number
  totalPct: number
}

export function computeHotspots(data: ProfileData, samples: Sample[]): HotspotRow[] {
  const self = new Map<number, number>()
  const total = new Map<number, number>()
  let grand = 0
  const seen = new Set<number>()
  for (const s of samples) {
    const frames = data.stacks[s.stackId].frames
    grand += s.weight
    const leaf = frames[frames.length - 1]
    self.set(leaf, (self.get(leaf) ?? 0) + s.weight)
    seen.clear()
    for (const fid of frames) {
      if (seen.has(fid)) continue
      seen.add(fid)
      total.set(fid, (total.get(fid) ?? 0) + s.weight)
    }
  }
  const rows: HotspotRow[] = []
  for (const [frameId, t] of total) {
    const sw = self.get(frameId) ?? 0
    rows.push({ frameId, self: sw, total: t, selfPct: grand ? sw / grand : 0, totalPct: grand ? t / grand : 0 })
  }
  rows.sort((a, b) => b.self - a.self)
  return rows
}

export interface CallerCalleeEntry {
  frameId: number
  weight: number
  pct: number
}

export function computeCallersCallees(
  data: ProfileData,
  samples: Sample[],
  target: number
): { callers: CallerCalleeEntry[]; callees: CallerCalleeEntry[]; selfWeight: number; totalWeight: number } {
  const callers = new Map<number, number>()
  const callees = new Map<number, number>()
  let totalWeight = 0
  let selfWeight = 0
  for (const s of samples) {
    const frames = data.stacks[s.stackId].frames
    const idx = frames.indexOf(target)
    if (idx < 0) continue
    totalWeight += s.weight
    if (idx > 0) callers.set(frames[idx - 1], (callers.get(frames[idx - 1]) ?? 0) + s.weight)
    if (idx < frames.length - 1) {
      callees.set(frames[idx + 1], (callees.get(frames[idx + 1]) ?? 0) + s.weight)
    } else {
      selfWeight += s.weight
    }
  }
  const toSorted = (m: Map<number, number>): CallerCalleeEntry[] =>
    [...m.entries()]
      .map(([frameId, weight]) => ({ frameId, weight, pct: totalWeight ? weight / totalWeight : 0 }))
      .sort((a, b) => b.weight - a.weight)
  return { callers: toSorted(callers), callees: toSorted(callees), selfWeight, totalWeight }
}

export function flamescopeMatrix(
  samples: Sample[],
  durationS: number,
  rowsPerSecond = 50
): { matrix: number[][]; max: number; cols: number; rows: number } {
  const cols = Math.ceil(durationS)
  const rows = rowsPerSecond
  const matrix: number[][] = Array.from({ length: rows }, () => new Array(cols).fill(0))
  let max = 0
  for (const s of samples) {
    const col = Math.min(cols - 1, Math.floor(s.time))
    const row = Math.min(rows - 1, Math.floor((s.time - Math.floor(s.time)) * rows))
    const v = (matrix[row][col] += s.weight)
    if (v > max) max = v
  }
  return { matrix, max, cols, rows }
}

export function threadTimeline(
  data: ProfileData,
  samples: Sample[],
  buckets: number
): Map<number, number[]> {
  const dur = data.meta.durationS
  const out = new Map<number, number[]>()
  for (const t of data.threads) out.set(t.tid, new Array(buckets).fill(0))
  for (const s of samples) {
    const arr = out.get(s.tid)
    if (!arr) continue
    const b = Math.min(buckets - 1, Math.floor((s.time / dur) * buckets))
    arr[b] += s.weight
  }
  return out
}

export function totalTimeline(samples: Sample[], durationS: number, buckets: number): number[] {
  const out = new Array(buckets).fill(0)
  for (const s of samples) {
    const b = Math.min(buckets - 1, Math.floor((s.time / durationS) * buckets))
    out[b] += s.weight
  }
  return out
}

export function perCpuUtilization(
  data: ProfileData,
  samples: Sample[],
  buckets: number
): { lanes: number[][]; bucketS: number } {
  const cores = data.meta.cores
  const dur = data.meta.durationS
  const bucketS = dur / buckets
  const capacity = bucketS * data.meta.sampleFreqHz
  const lanes: number[][] = Array.from({ length: cores }, () => new Array(buckets).fill(0))
  for (const s of samples) {
    if (s.cpu < 0 || s.cpu >= cores) continue
    const bi = Math.min(buckets - 1, Math.floor((s.time / dur) * buckets))
    lanes[s.cpu][bi] += 1
  }
  for (const lane of lanes) for (let i = 0; i < lane.length; i++) lane[i] = Math.min(1, lane[i] / capacity)
  return { lanes, bucketS }
}

export function concurrencyHistogram(
  data: ProfileData,
  samples: Sample[],
  slotS = 0.02
): { histogram: number[]; avg: number; totalS: number } {
  const cores = data.meta.cores
  const dur = data.meta.durationS
  const slots = Math.ceil(dur / slotS)
  const counts = new Array(slots).fill(0)
  for (const s of samples) {
    const si = Math.min(slots - 1, Math.floor(s.time / slotS))
    counts[si]++
  }
  const capacityPerCore = slotS * data.meta.sampleFreqHz
  const histogram = new Array(cores + 1).fill(0)
  let weighted = 0
  for (const c of counts) {
    const busy = Math.max(0, Math.min(cores, Math.round(c / capacityPerCore)))
    histogram[busy] += slotS
    weighted += busy * slotS
  }
  return { histogram, avg: weighted / dur, totalS: dur }
}
