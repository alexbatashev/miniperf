import { mulberry32 } from "./prng"
import type { ProfileData } from "./model"

export interface FnMetrics {
  ipc: number
  llcMpki: number
  llcMissRate: number
  beStall: number
  brMpki: number
  tma: { retiring: number; badSpec: number; frontend: number; backend: number }
}

const cache = new Map<number, FnMetrics>()

export function functionMetrics(data: ProfileData, frameId: number): FnMetrics {
  let m = cache.get(frameId)
  if (m) return m
  const rng = mulberry32(frameId * 2654435761)
  const tma = data.tmaByFunction.get(frameId) ?? {
    retiring: 0.3 + rng() * 0.25,
    badSpec: 0.03 + rng() * 0.08,
    frontend: 0.06 + rng() * 0.1,
    backend: 0.25 + rng() * 0.3,
  }
  const sum = tma.retiring + tma.badSpec + tma.frontend + tma.backend
  const t = {
    retiring: tma.retiring / sum,
    badSpec: tma.badSpec / sum,
    frontend: tma.frontend / sum,
    backend: tma.backend / sum,
  }
  m = {
    ipc: Math.max(0.1, 4 * t.retiring * (0.85 + rng() * 0.3)),
    llcMpki: Math.max(0.05, t.backend * 55 * (0.6 + rng() * 0.8)),
    llcMissRate: Math.min(0.95, t.backend * 0.9 * (0.5 + rng() * 0.6)),
    beStall: Math.min(0.95, t.backend * 1.15),
    brMpki: Math.max(0.1, t.badSpec * 90 * (0.5 + rng() * 0.7)),
    tma: t,
  }
  cache.set(frameId, m)
  return m
}
