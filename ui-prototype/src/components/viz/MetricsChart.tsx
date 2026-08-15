import { useMemo, useState } from "react"
import type { CounterSeries } from "@/lib/model"
import { useFilter } from "@/lib/filter"
import { fmtCount } from "@/lib/format"

function fmtVal(v: number, unit: string): string {
  if (unit === "%") return v.toFixed(1) + "%"
  if (unit === "GB/s") return v.toFixed(1) + " GB/s"
  if (unit === "/s") return fmtCount(v) + "/s"
  return v.toFixed(2)
}

export function MetricsChart({
  series,
  durationS,
  height = 64,
  gutter = 0,
}: {
  series: CounterSeries[]
  durationS: number
  height?: number
  gutter?: number
}) {
  const { filter, patch } = useFilter()
  const [hoverT, setHoverT] = useState<number | null>(null)
  const [drag, setDrag] = useState<{ t0: number; t1: number } | null>(null)

  const range: [number, number] = [0, durationS]
  const selection = filter.timeRange

  return (
    <div className="flex flex-col">
      {series.map((s) => (
        <SingleTrack
          key={s.id}
          s={s}
          range={range}
          height={height}
          gutter={gutter}
          selection={selection}
          hoverT={hoverT}
          setHoverT={setHoverT}
          drag={drag}
          setDrag={setDrag}
          onBrush={(t0, t1) => patch({ timeRange: t1 - t0 > 0.02 ? [t0, t1] : null })}
        />
      ))}
    </div>
  )
}

function SingleTrack({
  s,
  range,
  height,
  gutter,
  selection,
  hoverT,
  setHoverT,
  drag,
  setDrag,
  onBrush,
}: {
  s: CounterSeries
  range: [number, number]
  height: number
  gutter: number
  selection: [number, number] | null
  hoverT: number | null
  setHoverT: (t: number | null) => void
  drag: { t0: number; t1: number } | null
  setDrag: (d: { t0: number; t1: number } | null) => void
  onBrush: (t0: number, t1: number) => void
}) {
  const [t0, t1] = range
  const pts = useMemo(() => s.points.filter((p) => p.time >= t0 && p.time <= t1), [s.points, t0, t1])
  const vmax = useMemo(() => s.max ?? Math.max(1e-9, ...pts.map((p) => p.value)) * 1.08, [s.max, pts])

  const W = 100
  const H = 100
  const path = useMemo(() => {
    if (pts.length === 0) return ""
    return pts
      .map((p, i) => {
        const x = ((p.time - t0) / (t1 - t0)) * W
        const y = H - (p.value / vmax) * H
        return `${i === 0 ? "M" : "L"}${x.toFixed(2)},${y.toFixed(2)}`
      })
      .join("")
  }, [pts, t0, t1, vmax])

  const areaPath = path ? `${path}L${W},${H}L0,${H}Z` : ""

  const hoverPt = useMemo(() => {
    if (hoverT === null || pts.length === 0) return null
    let best = pts[0]
    for (const p of pts) if (Math.abs(p.time - hoverT) < Math.abs(best.time - hoverT)) best = p
    return best
  }, [hoverT, pts])

  const timeAt = (e: React.MouseEvent<SVGSVGElement>): number => {
    const b = e.currentTarget.getBoundingClientRect()
    return t0 + ((e.clientX - b.left) / b.width) * (t1 - t0)
  }

  const labelBlock = (
    <>
      <span className="font-medium text-[var(--viz-ink-2)]">{s.label}</span>
      <span className="tabular-nums text-[var(--viz-muted)]">
        {hoverPt ? fmtVal(hoverPt.value, s.unit) : pts.length ? fmtVal(pts[pts.length - 1].value, s.unit) : "—"}
      </span>
    </>
  )

  return (
    <div className="group relative flex border-b border-border/60 last:border-b-0">
      {gutter > 0 ? (
        <div
          className="flex shrink-0 flex-col justify-center gap-0.5 overflow-hidden bg-[var(--viz-surface)] px-1.5 text-[10px] leading-tight"
          style={{ width: gutter }}
        >
          {labelBlock}
        </div>
      ) : (
        <div className="pointer-events-none absolute left-1.5 top-0.5 z-10 flex items-baseline gap-2 text-[10.5px] leading-none">
          {labelBlock}
        </div>
      )}
      <svg
        height={height}
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        className="block min-w-0 flex-1 cursor-crosshair bg-[var(--viz-surface)]"
        onMouseMove={(e) => {
          const t = timeAt(e)
          setHoverT(t)
          if (drag) setDrag({ t0: drag.t0, t1: t })
        }}
        onMouseLeave={() => setHoverT(null)}
        onMouseDown={(e) => {
          const t = timeAt(e)
          setDrag({ t0: t, t1: t })
        }}
        onMouseUp={() => {
          if (drag) {
            onBrush(Math.min(drag.t0, drag.t1), Math.max(drag.t0, drag.t1))
            setDrag(null)
          }
        }}
      >
        {areaPath && <path d={areaPath} fill="var(--series-1)" opacity={0.1} />}
        {path && (
          <path d={path} fill="none" stroke="var(--series-1)" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
        )}
        {selection && !drag && (
          <rect
            x={((selection[0] - t0) / (t1 - t0)) * W}
            y={0}
            width={((selection[1] - selection[0]) / (t1 - t0)) * W}
            height={H}
            fill="var(--series-1)"
            opacity={0.1}
          />
        )}
        {drag && (
          <rect
            x={((Math.min(drag.t0, drag.t1) - t0) / (t1 - t0)) * W}
            y={0}
            width={((Math.abs(drag.t1 - drag.t0)) / (t1 - t0)) * W}
            height={H}
            fill="var(--series-1)"
            opacity={0.12}
          />
        )}
        {hoverT !== null && hoverT >= t0 && hoverT <= t1 && (
          <line
            x1={((hoverT - t0) / (t1 - t0)) * W}
            x2={((hoverT - t0) / (t1 - t0)) * W}
            y1={0}
            y2={H}
            stroke="var(--viz-axis)"
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
          />
        )}
      </svg>
    </div>
  )
}
