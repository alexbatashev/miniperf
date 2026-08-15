import { createContext, useContext, useMemo, useState, type ReactNode } from "react"
import type { ProfileData, Sample } from "./model"

export interface GlobalFilter {
  timeRange: [number, number] | null
  threads: number[] | null
  symbolQuery: string
  modules: string[] | null
}

export const EMPTY_FILTER: GlobalFilter = {
  timeRange: null,
  threads: null,
  symbolQuery: "",
  modules: null,
}

export function isFilterEmpty(f: GlobalFilter): boolean {
  return !f.timeRange && !f.threads && !f.symbolQuery && !f.modules
}

interface FilterCtx {
  filter: GlobalFilter
  setFilter: (f: GlobalFilter | ((prev: GlobalFilter) => GlobalFilter)) => void
  patch: (p: Partial<GlobalFilter>) => void
  clear: () => void
  selectedFrame: number | null
  setSelectedFrame: (f: number | null) => void
}

const Ctx = createContext<FilterCtx | null>(null)

function initialFilter(): GlobalFilter {
  const p = new URLSearchParams(window.location.search)
  const t0 = p.get("t0")
  const t1 = p.get("t1")
  return {
    ...EMPTY_FILTER,
    timeRange: t0 !== null && t1 !== null ? [parseFloat(t0), parseFloat(t1)] : null,
    symbolQuery: p.get("q") ?? "",
  }
}

export function FilterProvider({ children }: { children: ReactNode }) {
  const [filter, setFilter] = useState<GlobalFilter>(initialFilter)
  const [selectedFrame, setSelectedFrame] = useState<number | null>(null)
  const value = useMemo<FilterCtx>(
    () => ({
      filter,
      setFilter,
      patch: (p) => setFilter((prev) => ({ ...prev, ...p })),
      clear: () => setFilter(EMPTY_FILTER),
      selectedFrame,
      setSelectedFrame,
    }),
    [filter, selectedFrame]
  )
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>
}

export function useFilter(): FilterCtx {
  const ctx = useContext(Ctx)
  if (!ctx) throw new Error("useFilter outside FilterProvider")
  return ctx
}

export function applyFilter(data: ProfileData, filter: GlobalFilter, source?: Sample[]): Sample[] {
  const { timeRange, threads, symbolQuery, modules } = filter
  const threadSet = threads ? new Set(threads) : null
  const moduleSet = modules ? new Set(modules) : null
  const q = symbolQuery.trim().toLowerCase()

  let stackAllowed: Uint8Array | null = null
  if (q || moduleSet) {
    stackAllowed = new Uint8Array(data.stacks.length)
    for (const stack of data.stacks) {
      let ok = false
      for (const fid of stack.frames) {
        const fr = data.frames[fid]
        if (moduleSet && !moduleSet.has(fr.module)) continue
        if (q && !fr.name.toLowerCase().includes(q)) continue
        ok = true
        break
      }
      stackAllowed[stack.id] = ok ? 1 : 0
    }
  }

  return (source ?? data.samples).filter((s) => {
    if (timeRange && (s.time < timeRange[0] || s.time > timeRange[1])) return false
    if (threadSet && !threadSet.has(s.tid)) return false
    if (stackAllowed && !stackAllowed[s.stackId]) return false
    return true
  })
}
