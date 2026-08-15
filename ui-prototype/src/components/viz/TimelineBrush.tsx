import { useEffect, useMemo, useRef, useState } from "react"
import type { ProfileData, Sample } from "@/lib/model"
import { threadTimeline } from "@/lib/derive"
import { useFilter } from "@/lib/filter"
import { cssVar, setupCanvas, useCanvasSize } from "./use-canvas"

const LANE_H = 15
const AXIS_H = 16
const LABEL_W = 92

export function TimelineBrush({
  data,
  samples,
  compact = false,
}: {
  data: ProfileData
  samples: Sample[]
  compact?: boolean
}) {
  const { wrapRef, w, h } = useCanvasSize()
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { filter, patch } = useFilter()
  const [drag, setDrag] = useState<{ t0: number; t1: number } | null>(null)

  const buckets = Math.max(60, Math.floor((w - LABEL_W) / 3))
  const lanes = useMemo(() => threadTimeline(data, samples, buckets), [data, samples, buckets])
  const laneMax = useMemo(() => {
    let m = 1
    for (const arr of lanes.values()) for (const v of arr) if (v > m) m = v
    return m
  }, [lanes])

  const dur = data.meta.durationS
  const plotW = Math.max(0, w - LABEL_W - 4)

  const timeAt = (clientX: number): number => {
    const bounds = canvasRef.current!.getBoundingClientRect()
    const x = clientX - bounds.left - LABEL_W
    return Math.max(0, Math.min(dur, (x / plotW) * dur))
  }

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || w === 0 || h === 0) return
    const ctx = setupCanvas(canvas, w, h)
    ctx.fillStyle = cssVar("--viz-surface")
    ctx.fillRect(0, 0, w, h)
    const blue = cssVar("--series-1")
    const muted = cssVar("--viz-muted")
    const grid = cssVar("--viz-grid")
    const ink2 = cssVar("--viz-ink-2")
    ctx.font = "10px 'Geist Variable', sans-serif"

    const threadDimmed = (tid: number) => filter.threads !== null && !filter.threads.includes(tid)

    data.threads.forEach((t, i) => {
      const y = i * LANE_H + 2
      if (y + LANE_H > h - AXIS_H) return
      ctx.fillStyle = threadDimmed(t.tid) ? muted : ink2
      ctx.textAlign = "left"
      ctx.textBaseline = "middle"
      ctx.fillText(compact ? t.name.replace("omp worker", "w") : t.name, 6, y + LANE_H / 2, LABEL_W - 10)
      const arr = lanes.get(t.tid)!
      const bw = plotW / buckets
      for (let bi = 0; bi < buckets; bi++) {
        const v = arr[bi]
        if (v === 0) continue
        ctx.fillStyle = blue
        ctx.globalAlpha = threadDimmed(t.tid) ? 0.12 : 0.15 + 0.85 * (v / laneMax)
        ctx.fillRect(LABEL_W + bi * bw, y + 1, Math.max(1, bw - 0.5), LANE_H - 3)
      }
      ctx.globalAlpha = 1
    })

    ctx.strokeStyle = grid
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(LABEL_W, h - AXIS_H + 0.5)
    ctx.lineTo(w, h - AXIS_H + 0.5)
    ctx.stroke()
    ctx.fillStyle = muted
    ctx.textAlign = "center"
    ctx.textBaseline = "top"
    const step = dur > 20 ? 5 : dur > 8 ? 2 : 1
    for (let s = 0; s <= dur; s += step) {
      ctx.fillText(`${s}s`, LABEL_W + (s / dur) * plotW, h - AXIS_H + 4)
    }

    const range = drag ?? (filter.timeRange ? { t0: filter.timeRange[0], t1: filter.timeRange[1] } : null)
    if (range) {
      const x0 = LABEL_W + (Math.min(range.t0, range.t1) / dur) * plotW
      const x1 = LABEL_W + (Math.max(range.t0, range.t1) / dur) * plotW
      ctx.fillStyle = "rgba(42,120,214,0.12)"
      ctx.fillRect(x0, 0, x1 - x0, h - AXIS_H)
      ctx.strokeStyle = blue
      ctx.strokeRect(x0 + 0.5, 0.5, x1 - x0 - 1, h - AXIS_H - 1)
    }
  }, [lanes, laneMax, w, h, buckets, plotW, dur, drag, filter.timeRange, filter.threads, data.threads, compact])

  const height = data.threads.length * LANE_H + AXIS_H + 4

  return (
    <div ref={wrapRef} className="relative w-full shrink-0 overflow-hidden bg-[var(--viz-surface)]" style={{ height }}>
      <canvas
        ref={canvasRef}
        className="block cursor-crosshair"
        onMouseDown={(e) => {
          const t = timeAt(e.clientX)
          setDrag({ t0: t, t1: t })
        }}
        onMouseMove={(e) => drag && setDrag({ t0: drag.t0, t1: timeAt(e.clientX) })}
        onMouseUp={() => {
          if (!drag) return
          const t0 = Math.min(drag.t0, drag.t1)
          const t1 = Math.max(drag.t0, drag.t1)
          patch({ timeRange: t1 - t0 > 0.02 ? [t0, t1] : null })
          setDrag(null)
        }}
        onMouseLeave={() => setDrag(null)}
        onDoubleClick={() => patch({ timeRange: null })}
        title="Drag to filter time range · double-click to clear"
      />
    </div>
  )
}
