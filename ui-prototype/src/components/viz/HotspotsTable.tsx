import { useMemo, useState } from "react"
import type { ProfileData, Sample } from "@/lib/model"
import { computeHotspots } from "@/lib/derive"
import { functionMetrics } from "@/lib/fn-metrics"
import { useFilter } from "@/lib/filter"
import { fmtCount, fmtPct, fmtTimeS } from "@/lib/format"
import { TmaMiniBar } from "./TmaMiniBar"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"

type SortKey = "self" | "total" | "ipc" | "llcMpki" | "beStall" | "brMpki"

const MODULE_BADGE: Record<string, string> = {
  user: "bg-[var(--series-1)]/15 text-[var(--series-1)]",
  lib: "bg-[var(--series-3)]/15 text-[var(--series-3)]",
  runtime: "bg-[var(--series-7)]/15 text-[var(--series-7)]",
  kernel: "bg-[var(--series-2)]/15 text-[var(--series-2)]",
}

export function HotspotsTable({
  data,
  samples,
  limit = 60,
  dense = false,
  onOpenSource,
}: {
  data: ProfileData
  samples: Sample[]
  limit?: number
  dense?: boolean
  onOpenSource?: (frameId: number) => void
}) {
  const { selectedFrame, setSelectedFrame } = useFilter()
  const [sortKey, setSortKey] = useState<SortKey>("self")
  const [desc, setDesc] = useState(true)

  const rows = useMemo(() => {
    const hs = computeHotspots(data, samples).slice(0, 200)
    const withMetrics = hs.map((r) => ({ ...r, m: functionMetrics(data, r.frameId) }))
    const get = (r: (typeof withMetrics)[0]): number => {
      switch (sortKey) {
        case "self": return r.self
        case "total": return r.total
        case "ipc": return r.m.ipc
        case "llcMpki": return r.m.llcMpki
        case "beStall": return r.m.beStall
        case "brMpki": return r.m.brMpki
      }
    }
    withMetrics.sort((a, b) => (desc ? get(b) - get(a) : get(a) - get(b)))
    return withMetrics.slice(0, limit)
  }, [data, samples, sortKey, desc, limit])

  const showMetrics = !!data.tma
  const hasSource = (frameId: number) => data.sources.some((s) => s.frameId === frameId)

  const th = (key: SortKey | null, label: string, cls = "") => (
    <TableHead
      className={cn(
        "sticky top-0 z-10 h-7 whitespace-nowrap border-b bg-background px-2 text-[10.5px] font-medium text-muted-foreground",
        key && "cursor-pointer select-none hover:text-foreground",
        cls
      )}
      onClick={() => {
        if (!key) return
        if (sortKey === key) setDesc(!desc)
        else {
          setSortKey(key)
          setDesc(true)
        }
      }}
    >
      {label}
      {key === sortKey && <span className="ml-0.5 text-[9px]">{desc ? "▼" : "▲"}</span>}
    </TableHead>
  )

  const cellPad = dense ? "px-2 py-0.5" : "px-2 py-1"

  return (
    <Table className="text-xs tabular-nums" containerClassName="size-full overflow-auto">
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          {th(null, "Function", "w-full")}
          {th("self", "Self %", "text-right")}
          {th("total", "Total %", "text-right")}
          {th(null, "CPU time", "text-right")}
          {showMetrics && th("ipc", "IPC", "text-right")}
          {showMetrics && th("llcMpki", "LLC MPKI", "text-right")}
          {showMetrics && th("beStall", "BE stall", "text-right")}
          {showMetrics && th("brMpki", "Br MPKI", "text-right")}
          {showMetrics && th(null, "TMA", "min-w-[90px]")}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => {
          const fr = data.frames[r.frameId]
          const selected = selectedFrame === r.frameId
          return (
            <TableRow
              key={r.frameId}
              className={cn("cursor-pointer border-border/40", selected && "bg-accent")}
              onClick={() => setSelectedFrame(selected ? null : r.frameId)}
              onDoubleClick={() => onOpenSource?.(r.frameId)}
              title={onOpenSource && hasSource(r.frameId) ? "Double-click to open source & disassembly" : undefined}
            >
              <TableCell className={cn("max-w-0 truncate font-mono text-[11px]", cellPad)}>
                <Badge
                  variant="secondary"
                  className={cn("mr-1.5 rounded-sm px-1 py-0 align-middle font-sans text-[9px] leading-4", MODULE_BADGE[fr.kind])}
                >
                  {fr.module.replace(".so.6", "").replace(".so.1", "")}
                </Badge>
                {fr.name}
              </TableCell>
              <TableCell className={cn("relative whitespace-nowrap text-right", cellPad)}>
                <div
                  className="absolute inset-y-1 left-0 rounded-r-[2px] bg-[var(--series-1)]/18"
                  style={{ width: `${Math.min(100, r.selfPct * 100 * 2.4)}%` }}
                />
                <span className="relative">{fmtPct(r.selfPct)}</span>
              </TableCell>
              <TableCell className={cn("whitespace-nowrap text-right text-muted-foreground", cellPad)}>
                {fmtPct(r.totalPct)}
              </TableCell>
              <TableCell className={cn("whitespace-nowrap text-right text-muted-foreground", cellPad)}>
                {fmtTimeS(r.self / data.meta.sampleFreqHz)}
              </TableCell>
              {showMetrics && <TableCell className={cn("whitespace-nowrap text-right", cellPad)}>{r.m.ipc.toFixed(2)}</TableCell>}
              {showMetrics && <TableCell className={cn("whitespace-nowrap text-right", cellPad)}>{r.m.llcMpki.toFixed(1)}</TableCell>}
              {showMetrics && <TableCell className={cn("whitespace-nowrap text-right", cellPad)}>{fmtPct(r.m.beStall, 0)}</TableCell>}
              {showMetrics && <TableCell className={cn("whitespace-nowrap text-right", cellPad)}>{r.m.brMpki.toFixed(1)}</TableCell>}
              {showMetrics && (
                <TableCell className={cellPad}>
                  <TmaMiniBar tma={r.m.tma} />
                </TableCell>
              )}
            </TableRow>
          )
        })}
        {rows.length === 0 && (
          <TableRow>
            <TableCell colSpan={showMetrics ? 9 : 4} className="px-2 py-6 text-center text-muted-foreground">
              No samples match the current filter — {fmtCount(data.samples.length)} total available.
            </TableCell>
          </TableRow>
        )}
      </TableBody>
    </Table>
  )
}
