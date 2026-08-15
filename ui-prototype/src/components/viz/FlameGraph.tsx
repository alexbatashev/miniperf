import { useEffect, useMemo, useRef, useState } from "react"
import type { FlameNode } from "@/lib/derive"
import type { ProfileData } from "@/lib/model"
import { useFilter } from "@/lib/filter"
import { fmtCount, fmtPct } from "@/lib/format"
import { cssVar, setupCanvas, useCanvasSize } from "./use-canvas"
import { Button } from "@/components/ui/button"

const ROW_H = 17

interface Rect {
  x: number
  w: number
  depth: number
  frameId: number
  node: FlameNode
}

const KIND_VAR: Record<string, string> = {
  user: "--series-1",
  lib: "--series-3",
  runtime: "--series-7",
  kernel: "--series-2",
}

function layout(root: FlameNode, zoom: FlameNode | null): { rects: Rect[]; maxDepth: number } {
  const rects: Rect[] = []
  let maxDepth = 0
  const base = zoom ?? root
  const walk = (node: FlameNode, x: number, w: number, depth: number) => {
    if (w < 0.0005) return
    if (node.frameId >= 0) {
      rects.push({ x, w, depth, frameId: node.frameId, node })
      if (depth > maxDepth) maxDepth = depth
      depth += 1
    }
    let cx = x
    const kids = [...node.children.values()].sort((a, b) => b.total - a.total)
    for (const child of kids) {
      const cw = (child.total / base.total) * w
      walk(child, cx, cw, depth)
      cx += cw
    }
  }
  walk(base, 0, 1, 0)
  return { rects, maxDepth }
}

export function FlameGraph({
  data,
  root,
  inverted = false,
  formatValue = (n) => `${fmtCount(n)} samples`,
}: {
  data: ProfileData
  root: FlameNode
  inverted?: boolean
  formatValue?: (n: number) => string
}) {
  const { wrapRef, w, h } = useCanvasSize()
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { filter, selectedFrame, setSelectedFrame } = useFilter()
  const [zoomPath, setZoomPath] = useState<number[]>([])
  const [hover, setHover] = useState<{ rect: Rect; mx: number; my: number } | null>(null)

  const zoomNode = useMemo(() => {
    let node: FlameNode = root
    for (const fid of zoomPath) {
      const next = node.children.get(fid)
      if (!next) return node
      node = next
    }
    return node
  }, [root, zoomPath])

  useEffect(() => setZoomPath([]), [root])

  const { rects } = useMemo(() => layout(root, zoomNode), [root, zoomNode])
  const q = filter.symbolQuery.trim().toLowerCase()

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || w === 0 || h === 0) return
    const ctx = setupCanvas(canvas, w, h)
    const surface = cssVar("--viz-surface")
    ctx.fillStyle = surface
    ctx.fillRect(0, 0, w, h)
    ctx.font = "11px 'Geist Variable', sans-serif"
    ctx.textBaseline = "middle"
    const ink = cssVar("--viz-ink")
    const colors: Record<string, string> = {}
    for (const [kind, v] of Object.entries(KIND_VAR)) colors[kind] = cssVar(v)
    const dim = cssVar("--viz-grid")

    for (const r of rects) {
      const px = r.x * w
      const pw = Math.max(r.w * w - 1, 0.5)
      const py = r.depth * ROW_H
      if (py > h) continue
      const frame = data.frames[r.frameId]
      const matches = !q || frame.name.toLowerCase().includes(q)
      ctx.fillStyle = matches ? colors[frame.kind] : dim
      const selected = selectedFrame === r.frameId
      ctx.globalAlpha = selected || hover?.rect === r ? 1 : matches ? 0.82 : 0.6
      ctx.fillRect(px, py, pw, ROW_H - 1)
      ctx.globalAlpha = 1
      if (selected) {
        ctx.strokeStyle = ink
        ctx.lineWidth = 1
        ctx.strokeRect(px + 0.5, py + 0.5, pw - 1, ROW_H - 2)
      }
      if (pw > 28) {
        ctx.fillStyle = matches ? "#ffffff" : cssVar("--viz-muted")
        const label = frame.name
        const maxChars = Math.floor((pw - 8) / 5.6)
        ctx.fillText(label.length > maxChars ? label.slice(0, Math.max(0, maxChars - 1)) + "…" : label, px + 4, py + ROW_H / 2, pw - 8)
      }
    }
  }, [rects, w, h, data, q, selectedFrame, hover])

  const hitTest = (e: React.MouseEvent): Rect | null => {
    const bounds = canvasRef.current!.getBoundingClientRect()
    const mx = (e.clientX - bounds.left) / w
    const my = e.clientY - bounds.top
    const depth = Math.floor(my / ROW_H)
    for (const r of rects) {
      if (r.depth === depth && mx >= r.x && mx <= r.x + r.w) return r
    }
    return null
  }

  const pathTo = (target: Rect): number[] => {
    const path: number[] = []
    const walk = (node: FlameNode, acc: number[]): boolean => {
      if (node === target.node) {
        path.push(...acc)
        return true
      }
      for (const [fid, child] of node.children) {
        if (walk(child, [...acc, fid])) return true
      }
      return false
    }
    walk(zoomNode, [])
    return [...zoomPath, ...path]
  }

  return (
    <div ref={wrapRef} className="relative size-full min-h-0 overflow-hidden bg-[var(--viz-surface)]">
      <canvas
        ref={canvasRef}
        className="block cursor-pointer"
        onMouseMove={(e) => {
          const r = hitTest(e)
          const bounds = canvasRef.current!.getBoundingClientRect()
          setHover(r ? { rect: r, mx: e.clientX - bounds.left, my: e.clientY - bounds.top } : null)
        }}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const r = hitTest(e)
          setSelectedFrame(r ? r.frameId : null)
        }}
        onDoubleClick={(e) => {
          const r = hitTest(e)
          if (r) setZoomPath(pathTo(r))
        }}
      />
      {zoomPath.length > 0 && (
        <Button
          size="sm"
          variant="secondary"
          className="absolute right-2 top-2 h-6 px-2 text-xs shadow"
          onClick={() => setZoomPath([])}
        >
          Reset zoom
        </Button>
      )}
      {hover && (
        <div
          className="pointer-events-none absolute z-20 max-w-96 rounded border bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-md"
          style={{
            left: Math.min(hover.mx + 12, Math.max(0, w - 320)),
            top: Math.min(hover.my + 14, Math.max(0, h - 80)),
          }}
        >
          <div className="font-medium">{data.frames[hover.rect.frameId].name}</div>
          <div className="text-muted-foreground">
            {data.frames[hover.rect.frameId].module} · {formatValue(hover.rect.node.total)} ·{" "}
            {fmtPct(hover.rect.node.total / (root.total || 1))} of {inverted ? "inverted " : ""}total
          </div>
          <div className="text-muted-foreground">double-click to zoom · click to select</div>
        </div>
      )}
    </div>
  )
}
