import { useMemo } from "react"
import type { ProfileData, Sample } from "@/lib/model"
import { computeCallersCallees, computeHotspots } from "@/lib/derive"
import { useFilter } from "@/lib/filter"
import { fmtCount, fmtPct } from "@/lib/format"
import { cn } from "@/lib/utils"

function Row({
  name,
  module,
  pct,
  weight,
  onClick,
  dir,
}: {
  name: string
  module: string
  pct: number
  weight: number
  onClick: () => void
  dir: "up" | "down"
}) {
  return (
    <button
      onClick={onClick}
      className="group relative flex w-full items-center gap-2 border-b border-border/40 px-2 py-[3px] text-left text-[11px] hover:bg-accent/60"
    >
      <div
        className="absolute inset-y-0.5 left-0 rounded-r-[2px] bg-[var(--series-1)]/14"
        style={{ width: `${pct * 100}%` }}
      />
      <span className="relative w-12 shrink-0 text-right tabular-nums text-muted-foreground">{fmtPct(pct)}</span>
      <span className="relative shrink-0 text-[9px] text-muted-foreground">{dir === "up" ? "↑" : "↓"}</span>
      <span className="relative truncate font-mono">{name}</span>
      <span className="relative ml-auto shrink-0 text-[10px] text-muted-foreground">{module}</span>
      <span className="relative shrink-0 tabular-nums text-[10px] text-muted-foreground">{fmtCount(weight)}</span>
    </button>
  )
}

export function CallerCallee({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const { selectedFrame, setSelectedFrame } = useFilter()

  const fallback = useMemo(() => {
    if (selectedFrame !== null) return null
    return computeHotspots(data, samples).slice(0, 12)
  }, [selectedFrame, data, samples])

  const cc = useMemo(
    () => (selectedFrame !== null ? computeCallersCallees(data, samples, selectedFrame) : null),
    [data, samples, selectedFrame]
  )

  if (selectedFrame === null || !cc) {
    return (
      <div className="flex size-full flex-col overflow-auto">
        <div className="px-3 py-2 text-xs text-muted-foreground">
          Select a function to see its callers and callees. Hottest by self time:
        </div>
        {fallback?.map((r) => (
          <Row
            key={r.frameId}
            name={data.frames[r.frameId].name}
            module={data.frames[r.frameId].module}
            pct={r.selfPct}
            weight={r.self}
            dir="down"
            onClick={() => setSelectedFrame(r.frameId)}
          />
        ))}
      </div>
    )
  }

  const fr = data.frames[selectedFrame]
  return (
    <div className="flex size-full min-h-0 flex-col overflow-auto text-xs">
      <div className="px-2 pt-1.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
        Callers
      </div>
      <div>
        {cc.callers.length === 0 && <div className="px-3 py-1 text-muted-foreground">— thread root —</div>}
        {cc.callers.map((e) => (
          <Row
            key={e.frameId}
            name={data.frames[e.frameId].name}
            module={data.frames[e.frameId].module}
            pct={e.pct}
            weight={e.weight}
            dir="up"
            onClick={() => setSelectedFrame(e.frameId)}
          />
        ))}
      </div>
      <div
        className={cn(
          "sticky top-0 z-10 my-1 flex items-center gap-2 border-y bg-accent px-2 py-1.5 font-mono text-[11.5px] font-semibold"
        )}
      >
        <span className="truncate">{fr.name}</span>
        <span className="font-sans text-[10px] font-normal text-muted-foreground">{fr.module}</span>
        <span className="ml-auto whitespace-nowrap font-sans text-[10px] font-normal text-muted-foreground">
          {fmtCount(cc.totalWeight)} samples · self {fmtCount(cc.selfWeight)}
        </span>
        <button
          className="rounded px-1 font-sans text-[10px] text-muted-foreground hover:bg-background"
          onClick={() => setSelectedFrame(null)}
        >
          ✕
        </button>
      </div>
      <div className="px-2 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">Callees</div>
      <div>
        {cc.callees.length === 0 && <div className="px-3 py-1 text-muted-foreground">— leaf —</div>}
        {cc.callees.map((e) => (
          <Row
            key={e.frameId}
            name={data.frames[e.frameId].name}
            module={data.frames[e.frameId].module}
            pct={e.pct}
            weight={e.weight}
            dir="down"
            onClick={() => setSelectedFrame(e.frameId)}
          />
        ))}
      </div>
    </div>
  )
}
