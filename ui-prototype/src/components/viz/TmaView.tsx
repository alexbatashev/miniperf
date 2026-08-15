import { useMemo } from "react"
import type { ProfileData, Sample, TmaNode } from "@/lib/model"
import { computeHotspots } from "@/lib/derive"
import { functionMetrics } from "@/lib/fn-metrics"
import { useFilter } from "@/lib/filter"
import { fmtPct } from "@/lib/format"
import { TMA_COLORS, TMA_LABELS, TmaLegend, TmaMiniBar } from "./TmaMiniBar"
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip"
import { Badge } from "@/components/ui/badge"
import { Info } from "lucide-react"
import { cn } from "@/lib/utils"

const BRANCH_COLOR: Record<string, string> = {
  retiring: TMA_COLORS.retiring,
  bad_speculation: TMA_COLORS.badSpec,
  fe_bound: TMA_COLORS.frontend,
  be_bound: TMA_COLORS.backend,
}

function TmaRows({ root, dominantPath }: { root: TmaNode; dominantPath: Set<string> }) {
  const rows: React.ReactNode[] = []
  const walk = (n: TmaNode, branch: string | null) => {
    if (n.level > 0) {
      const color = BRANCH_COLOR[branch ?? n.id] ?? "var(--series-1)"
      const dominant = dominantPath.has(n.id)
      rows.push(
        <div
          key={n.id}
          className="flex h-[26px] items-center gap-2 border-b border-border/40 pr-2 text-xs hover:bg-accent/40"
          style={{ paddingLeft: `${(n.level - 1) * 16 + 8}px` }}
        >
          <span className={cn("flex w-44 shrink-0 items-center gap-1 truncate", dominant ? "font-semibold" : "text-foreground/90")}>
            <span className="truncate">{n.name}</span>
            {n.description && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Info className="size-3 shrink-0 cursor-help text-muted-foreground/60 hover:text-muted-foreground" />
                </TooltipTrigger>
                <TooltipContent side="right" className="max-w-72 leading-snug">
                  {n.description}
                </TooltipContent>
              </Tooltip>
            )}
            {dominant && n.level === 1 && (
              <Badge className="rounded-sm bg-[var(--status-serious)]/15 px-1 py-0 text-[9px] font-medium text-[var(--status-serious)]">
                dominant
              </Badge>
            )}
          </span>
          <div className="relative h-[10px] min-w-0 flex-1 rounded-[2px] bg-[var(--viz-grid)]/50">
            <div
              className="h-full rounded-[2px]"
              style={{ width: `${n.value * 100}%`, background: color, opacity: dominant ? 1 : 0.55 }}
            />
          </div>
          <span className={cn("w-12 shrink-0 text-right tabular-nums", dominant && "font-semibold")}>
            {fmtPct(n.value)}
          </span>
        </div>
      )
    }
    for (const c of n.children ?? []) walk(c, n.level === 0 ? c.id : (branch ?? n.id))
  }
  walk(root, null)
  return <>{rows}</>
}

function StackedIntervals({ data }: { data: ProfileData }) {
  const { filter } = useFilter()
  const [t0, t1] = filter.timeRange ?? [0, data.meta.durationS]
  const pts = (data.tmaIntervals ?? []).filter((p) => p.time >= t0 - 0.5 && p.time <= t1 + 0.5)
  const W = 100
  const H = 100
  const keys = ["retiring", "badSpec", "frontend", "backend"] as const

  const paths = useMemo(() => {
    const acc = new Array(pts.length).fill(0)
    return keys.map((k) => {
      const top: string[] = []
      const bottom: string[] = []
      pts.forEach((p, i) => {
        const x = ((p.time - t0) / Math.max(0.001, t1 - t0)) * W
        bottom.push(`${x.toFixed(2)},${(H - acc[i] * H).toFixed(2)}`)
        acc[i] += p[k]
        top.push(`${x.toFixed(2)},${(H - acc[i] * H).toFixed(2)}`)
      })
      return { k, d: `M${bottom.join("L")}L${top.reverse().join("L")}Z` }
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pts, t0, t1])

  return (
    <div className="flex min-h-0 flex-col">
      <div className="flex items-center justify-between px-2 py-1.5">
        <span className="text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
          Pipeline slots over time
        </span>
        <TmaLegend compact />
      </div>
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="h-24 w-full bg-[var(--viz-surface)]">
        {paths.map((p) => (
          <path key={p.k} d={p.d} fill={TMA_COLORS[p.k]} opacity={0.85} stroke="var(--viz-surface)" strokeWidth={0.4} />
        ))}
      </svg>
    </div>
  )
}

export function TmaView({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const { setSelectedFrame, selectedFrame } = useFilter()
  const tma = data.tma!

  const dominantPath = useMemo(() => {
    const set = new Set<string>()
    let node: TmaNode | undefined = tma
    while (node) {
      const kids: TmaNode[] = node.children ?? []
      if (kids.length === 0) break
      const top: TmaNode = kids.reduce((a, b) => (b.value > a.value ? b : a))
      set.add(top.id)
      node = top
    }
    return set
  }, [tma])

  const topFns = useMemo(() => {
    return computeHotspots(data, samples)
      .slice(0, 14)
      .map((r) => ({ ...r, m: functionMetrics(data, r.frameId) }))
  }, [data, samples])

  return (
    <div className="grid size-full min-h-0 grid-cols-[minmax(340px,5fr)_minmax(300px,4fr)] overflow-hidden">
      <div className="flex min-h-0 flex-col overflow-auto border-r">
        <div className="px-2 py-1.5 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
          Top-down hierarchy · % of pipeline slots
        </div>
        <TmaRows root={tma} dominantPath={dominantPath} />
        <StackedIntervals data={data} />
      </div>
      <div className="flex min-h-0 flex-col overflow-auto">
        <div className="flex items-center justify-between px-2 py-1.5">
          <span className="text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
            Per-function breakdown · top hotspots
          </span>
        </div>
        {topFns.map((r) => (
          <button
            key={r.frameId}
            className={cn(
              "grid grid-cols-[minmax(0,1fr)_52px_minmax(90px,120px)] items-center gap-2 border-b border-border/40 px-2 py-[5px] text-left text-[11px] hover:bg-accent/60",
              selectedFrame === r.frameId && "bg-accent"
            )}
            onClick={() => setSelectedFrame(selectedFrame === r.frameId ? null : r.frameId)}
          >
            <span className="truncate font-mono">{data.frames[r.frameId].name}</span>
            <span className="text-right tabular-nums text-muted-foreground">{fmtPct(r.selfPct)}</span>
            <TmaMiniBar tma={r.m.tma} />
          </button>
        ))}
        <div className="px-2 py-2">
          <TmaLegend />
        </div>
        <div className="mx-2 mb-2 rounded border bg-muted/30 px-2.5 py-2 text-[11px] leading-snug text-muted-foreground">
          <span className="font-medium text-foreground">Reading this: </span>
          {TMA_LABELS.backend} dominates ({fmtPct(0.46)}), driven by Memory Bound {fmtPct(0.331)} — mostly DRAM.
          The hottest offender is <span className="font-mono">gather_neighbors</span>: random access misses LLC.
          Sort particles by cell or prefetch to convert DRAM-bound slots into retiring slots.
        </div>
      </div>
    </div>
  )
}
