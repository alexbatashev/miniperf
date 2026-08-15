import { useEffect, useMemo, useRef, useState } from "react"
import type { ProfileData, Sample } from "@/lib/model"
import { flamescopeMatrix } from "@/lib/derive"
import { useFilter } from "@/lib/filter"
import { cssVar, setupCanvas, useCanvasSize } from "./use-canvas"

const RAMP = [
  "#cde2fb", "#b7d3f6", "#9ec5f4", "#86b6ef", "#6da7ec", "#5598e7",
  "#3987e5", "#2a78d6", "#256abf", "#1c5cab", "#184f95", "#104281", "#0d366b",
]

export function FlameScope({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const { wrapRef, w, h } = useCanvasSize()
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { filter, patch } = useFilter()
  const [drag, setDrag] = useState<{ t0: number; t1: number } | null>(null)
  const [hover, setHover] = useState<{ col: number; row: number; v: number; x: number; y: number } | null>(null)

  const rowsPerSec = 50
  const { matrix, max, cols, rows } = useMemo(
    () => flamescopeMatrix(samples, data.meta.durationS, rowsPerSec),
    [samples, data.meta.durationS]
  )

  const PAD_L = 46
  const PAD_B = 18
  const plotW = Math.max(0, w - PAD_L - 4)
  const plotH = Math.max(0, h - PAD_B - 6)
  const cw = plotW / cols
  const ch = plotH / rows

  const timeAt = (clientX: number): number => {
    const bounds = canvasRef.current!.getBoundingClientRect()
    const x = clientX - bounds.left - PAD_L
    return Math.max(0, Math.min(data.meta.durationS, (x / plotW) * data.meta.durationS))
  }

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || w === 0 || h === 0) return
    const ctx = setupCanvas(canvas, w, h)
    ctx.fillStyle = cssVar("--viz-surface")
    ctx.fillRect(0, 0, w, h)
    const muted = cssVar("--viz-muted")

    for (let c = 0; c < cols; c++) {
      for (let r = 0; r < rows; r++) {
        const v = matrix[r][c]
        if (v === 0) continue
        const idx = Math.min(RAMP.length - 1, Math.floor(Math.sqrt(v / max) * RAMP.length))
        ctx.fillStyle = RAMP[idx]
        const y = plotH - (r + 1) * ch + 6
        ctx.fillRect(PAD_L + c * cw + 0.5, y, Math.max(0.5, cw - 1), Math.max(0.5, ch - 0.5))
      }
    }

    ctx.font = "10px 'Geist Variable', sans-serif"
    ctx.fillStyle = muted
    ctx.textAlign = "center"
    const tickEvery = cols > 30 ? 5 : cols > 15 ? 2 : 1
    for (let c = 0; c <= cols; c += tickEvery) {
      ctx.fillText(`${c}s`, PAD_L + c * cw, h - 5)
    }
    ctx.textAlign = "right"
    for (const frac of [0, 0.5, 1]) {
      const y = plotH - frac * plotH + 6
      ctx.fillText(`${Math.round(frac * 1000)}ms`, PAD_L - 5, Math.min(plotH, Math.max(10, y)))
    }

    const range = drag ?? (filter.timeRange ? { t0: filter.timeRange[0], t1: filter.timeRange[1] } : null)
    if (range) {
      const x0 = PAD_L + (Math.min(range.t0, range.t1) / data.meta.durationS) * plotW
      const x1 = PAD_L + (Math.max(range.t0, range.t1) / data.meta.durationS) * plotW
      ctx.fillStyle = "rgba(42,120,214,0.14)"
      ctx.fillRect(x0, 6, x1 - x0, plotH)
      ctx.strokeStyle = cssVar("--series-1")
      ctx.lineWidth = 1
      ctx.strokeRect(x0 + 0.5, 6.5, x1 - x0 - 1, plotH - 1)
    }
  }, [matrix, max, cols, rows, w, h, cw, ch, plotW, plotH, drag, filter.timeRange, data.meta.durationS])

  return (
    <div ref={wrapRef} className="relative size-full min-h-0 overflow-hidden bg-[var(--viz-surface)]">
      <canvas
        ref={canvasRef}
        className="block cursor-crosshair"
        onMouseDown={(e) => {
          const t = timeAt(e.clientX)
          setDrag({ t0: t, t1: t })
        }}
        onMouseMove={(e) => {
          if (drag) {
            setDrag({ t0: drag.t0, t1: timeAt(e.clientX) })
            return
          }
          const bounds = canvasRef.current!.getBoundingClientRect()
          const x = e.clientX - bounds.left
          const y = e.clientY - bounds.top
          const col = Math.floor((x - PAD_L) / cw)
          const row = Math.floor((plotH - (y - 6)) / ch)
          if (col >= 0 && col < cols && row >= 0 && row < rows) {
            setHover({ col, row, v: matrix[row][col], x, y })
          } else setHover(null)
        }}
        onMouseUp={() => {
          if (!drag) return
          const t0 = Math.min(drag.t0, drag.t1)
          const t1 = Math.max(drag.t0, drag.t1)
          patch({ timeRange: t1 - t0 > 0.02 ? [t0, t1] : null })
          setDrag(null)
        }}
        onMouseLeave={() => {
          setHover(null)
          setDrag(null)
        }}
      />
      {hover && !drag && (
        <div
          className="pointer-events-none absolute z-20 rounded border bg-popover px-2 py-1 text-xs text-popover-foreground shadow-md"
          style={{ left: Math.min(hover.x + 10, w - 180), top: Math.max(0, hover.y - 34) }}
        >
          {hover.col}s +{Math.round((hover.row / rowsPerSec) * 1000)}ms · {hover.v} samples
        </div>
      )}
    </div>
  )
}
