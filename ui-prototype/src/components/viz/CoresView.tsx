import { useEffect, useMemo, useRef } from "react"
import type { ProfileData, Sample } from "@/lib/model"
import { concurrencyHistogram, perCpuUtilization } from "@/lib/derive"
import { useFilter } from "@/lib/filter"
import { fmtPct, fmtTimeS } from "@/lib/format"
import { cssVar, setupCanvas, useCanvasSize } from "./use-canvas"
import { VizCard } from "@/components/VizCard"
import { Meter } from "@/components/Meter"
import { Badge } from "@/components/ui/badge"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { cn } from "@/lib/utils"

const RAMP = ["#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b"]
const LANE_H = 20
const LABEL_W = 60
const AXIS_H = 16

function CpuHeatLanes({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const { wrapRef, w, h } = useCanvasSize()
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { patch } = useFilter()
  const dragRef = useRef<{ t0: number; t1: number } | null>(null)

  const buckets = Math.max(80, Math.floor((w - LABEL_W) / 4))
  const { lanes } = useMemo(() => perCpuUtilization(data, samples, buckets), [data, samples, buckets])
  const dur = data.meta.durationS
  const plotW = Math.max(0, w - LABEL_W - 4)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || w === 0 || h === 0) return
    const ctx = setupCanvas(canvas, w, h)
    ctx.fillStyle = cssVar("--viz-surface")
    ctx.fillRect(0, 0, w, h)
    ctx.font = "10px 'Geist Variable', sans-serif"
    const muted = cssVar("--viz-muted")
    const bw = plotW / buckets
    lanes.forEach((lane, cpu) => {
      const y = cpu * LANE_H + 2
      ctx.fillStyle = cssVar("--viz-ink-2")
      ctx.textAlign = "left"
      ctx.textBaseline = "middle"
      ctx.fillText(`cpu ${cpu}`, 6, y + LANE_H / 2)
      const avg = lane.reduce((a, b) => a + b, 0) / lane.length
      ctx.fillStyle = muted
      ctx.fillText(fmtPct(avg, 0), 36, y + LANE_H / 2)
      for (let i = 0; i < buckets; i++) {
        const v = lane[i]
        if (v <= 0.01) continue
        ctx.fillStyle = RAMP[Math.min(RAMP.length - 1, Math.floor(v * RAMP.length))]
        ctx.fillRect(LABEL_W + i * bw, y + 1, Math.max(1, bw - 0.5), LANE_H - 3)
      }
    })
    ctx.fillStyle = muted
    ctx.textAlign = "center"
    ctx.textBaseline = "top"
    for (let s = 0; s <= dur; s += 2) {
      ctx.fillText(`${s}s`, LABEL_W + (s / dur) * plotW, h - AXIS_H + 4)
    }
  }, [lanes, w, h, buckets, plotW, dur])

  const timeAt = (clientX: number) => {
    const b = canvasRef.current!.getBoundingClientRect()
    return Math.max(0, Math.min(dur, ((clientX - b.left - LABEL_W) / plotW) * dur))
  }

  return (
    <div
      ref={wrapRef}
      className="relative w-full shrink-0 overflow-hidden bg-[var(--viz-surface)]"
      style={{ height: data.meta.cores * LANE_H + AXIS_H + 4 }}
    >
      <canvas
        ref={canvasRef}
        className="block cursor-crosshair"
        title="Drag to filter time range"
        onMouseDown={(e) => (dragRef.current = { t0: timeAt(e.clientX), t1: timeAt(e.clientX) })}
        onMouseMove={(e) => {
          if (dragRef.current) dragRef.current.t1 = timeAt(e.clientX)
        }}
        onMouseUp={() => {
          const d = dragRef.current
          dragRef.current = null
          if (d) {
            const t0 = Math.min(d.t0, d.t1)
            const t1 = Math.max(d.t0, d.t1)
            patch({ timeRange: t1 - t0 > 0.02 ? [t0, t1] : null })
          }
        }}
      />
    </div>
  )
}

export function CoresView({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const { filter } = useFilter()
  const conc = useMemo(() => concurrencyHistogram(data, samples), [data, samples])
  const scopeS = filter.timeRange ? filter.timeRange[1] - filter.timeRange[0] : data.meta.durationS
  const maxSlot = Math.max(...conc.histogram, 0.001)

  const busySorted = [...data.threadStats].sort((a, b) => b.syncFrac - a.syncFrac)
  const maxSync = Math.max(...busySorted.map((t) => t.syncFrac), 0.001)

  return (
    <div className="flex size-full min-h-0 flex-col gap-2 overflow-auto p-2">
      <VizCard title="Per-CPU occupancy · color = busy fraction" action="drag to filter time" contentClassName="px-0 pb-0">
        <CpuHeatLanes data={data} samples={samples} />
      </VizCard>

      <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
        <VizCard
          title="Concurrency histogram"
          action={
            <span>
              avg <span className="font-semibold text-foreground">{conc.avg.toFixed(1)}</span> of {data.meta.cores} CPUs
              busy over {fmtTimeS(scopeS)}
            </span>
          }
        >
          <div className="mt-1 flex h-[120px] items-end gap-1.5">
            {conc.histogram.map((sec, n) => (
              <div key={n} className="flex min-w-0 flex-1 flex-col items-center gap-0.5">
                <span className="text-[9px] tabular-nums text-muted-foreground">
                  {sec >= 0.05 ? fmtTimeS(sec) : ""}
                </span>
                <div
                  className={cn("w-full max-w-9 rounded-t-[3px]")}
                  style={{
                    height: `${Math.max(sec > 0 ? 2 : 0, (sec / maxSlot) * 96)}px`,
                    background: n === 0 ? "var(--viz-grid)" : n <= 2 ? "var(--series-2)" : "var(--series-1)",
                  }}
                />
                <span className="text-[9.5px] text-muted-foreground">{n}</span>
              </div>
            ))}
          </div>
          <div className="mt-1 text-[10px] text-muted-foreground">simultaneously busy CPUs → elapsed time at that level</div>
          <div className="mt-1.5 text-[10.5px] leading-snug text-muted-foreground">
            <span className="inline-block size-2 rounded-[2px] align-middle" style={{ background: "var(--series-2)" }} />{" "}
            The 1–2-core band is serial time: init, finish, and the three checkpoints where one thread writes while
            seven wait. Amdahl cost of that band: ~1.9s of 8-core capacity.
          </div>
        </VizCard>

        <VizCard title="Thread balance · loop phase">
          <Table className="text-[11px] tabular-nums">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="h-6 px-0 text-[10px]">Thread</TableHead>
                <TableHead className="h-6 px-0 text-right text-[10px]">Busy</TableHead>
                <TableHead className="h-6 px-0 text-right text-[10px]">In sync</TableHead>
                <TableHead className="h-6 w-[30%] px-0 text-[10px]"></TableHead>
                <TableHead className="h-6 px-0 text-right text-[10px]">Migrations</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {busySorted.map((t) => {
                const th = data.threads.find((x) => x.tid === t.tid)!
                const worst = t.syncFrac === maxSync && t.syncFrac > 0.08
                return (
                  <TableRow key={t.tid} className="border-border/40 hover:bg-transparent">
                    <TableCell className="px-0 py-[3px]">
                      {th.name}
                      {worst && (
                        <Badge className="ml-1.5 rounded-sm bg-[var(--status-serious)]/14 px-1 py-0 text-[9px] font-medium text-[var(--status-serious)]">
                          waits most
                        </Badge>
                      )}
                    </TableCell>
                    <TableCell className="px-0 py-[3px] text-right">{fmtPct(t.busyFrac, 0)}</TableCell>
                    <TableCell className="px-0 py-[3px] text-right">{fmtPct(t.syncFrac)}</TableCell>
                    <TableCell className="py-[3px] pl-2 pr-0">
                      <Meter
                        value={t.syncFrac / maxSync}
                        color={worst ? "var(--status-serious)" : "var(--series-1)"}
                        className="h-[7px]"
                      />
                    </TableCell>
                    <TableCell className="px-0 py-[3px] text-right">{t.migrations}</TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
          <div className="mt-1.5 text-[10.5px] leading-snug text-muted-foreground">
            High sync share means the thread finishes its slice early and waits at the barrier. Worker 7 draws a sparse
            grid region and idles at every step boundary while the heavier slices set the pace — static
            partitioning leaves that capacity idle at every barrier.
          </div>
        </VizCard>
      </div>
    </div>
  )
}
