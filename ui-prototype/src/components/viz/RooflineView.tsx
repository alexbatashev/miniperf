import { useMemo, useState } from "react"
import type { ProfileData, RooflineLoop } from "@/lib/model"
import { useFilter } from "@/lib/filter"
import { fmtCount, fmtPct } from "@/lib/format"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

function QualityBadge({ quality }: { quality: RooflineLoop["quality"] }) {
  return (
    <Badge
      variant="secondary"
      className={cn(
        "rounded-sm px-1.5 py-0 text-[9.5px] font-medium",
        quality === "advisor-grade"
          ? "bg-[var(--status-good)]/12 text-[var(--status-good)]"
          : "bg-[var(--status-warn)]/15 text-foreground/70"
      )}
    >
      {quality}
    </Badge>
  )
}

const X_MIN = 0.02
const X_MAX = 8
const Y_MIN = 0.5
const Y_MAX = 1200

function sx(ai: number): number {
  return ((Math.log10(ai) - Math.log10(X_MIN)) / (Math.log10(X_MAX) - Math.log10(X_MIN))) * 100
}
function sy(gf: number): number {
  return 100 - ((Math.log10(gf) - Math.log10(Y_MIN)) / (Math.log10(Y_MAX) - Math.log10(Y_MIN))) * 100
}

export function RooflineView({
  data,
  onNavigate,
  onOpenSource,
}: {
  data: ProfileData
  onNavigate?: (id: string) => void
  onOpenSource?: (frameId: number) => void
}) {
  const rl = data.roofline!
  const { setSelectedFrame } = useFilter()
  const [sel, setSel] = useState<string | null>(rl.loops[0]?.id ?? null)
  const selected = rl.loops.find((l) => l.id === sel) ?? null

  const memCeils = rl.ceilings.filter((c) => c.kind === "memory")
  const compCeils = rl.ceilings.filter((c) => c.kind === "compute")
  const topCompute = Math.max(...compCeils.map((c) => c.value))

  const limitAt = (ai: number): number => Math.min(topCompute, ai * Math.max(...memCeils.map((c) => c.value)))

  const gridX = [0.05, 0.1, 0.25, 0.5, 1, 2, 4]
  const gridY = [1, 10, 100, 1000]

  const dotR = (l: RooflineLoop) => 0.65 + Math.sqrt(l.timeShare) * 1.6

  const rows = useMemo(() => [...rl.loops].sort((a, b) => b.timeShare - a.timeShare), [rl.loops])

  return (
    <div className="flex size-full min-h-0 flex-col overflow-auto">
      <div className="grid grid-cols-[minmax(380px,3fr)_minmax(260px,2fr)] border-b">
        <div className="relative border-r p-2">
          <div className="mb-1 flex items-baseline justify-between">
            <span className="text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
              {rl.precision} roofline · calibrated on this machine
            </span>
            <span className="text-[10px] text-muted-foreground">dot size = share of run time · hollow = scalar</span>
          </div>
          <svg viewBox="0 0 100 58" className="mx-auto w-full max-w-[880px] bg-[var(--viz-surface)]" style={{ aspectRatio: "100/58" }}>
            {(() => {
              const Y = (v: number) => 2 + sy(v) * 0.53
              return (
                <>
                  {gridX.map((x) => (
                    <line key={x} x1={sx(x)} x2={sx(x)} y1={2} y2={55} stroke="var(--viz-grid)" strokeWidth={0.12} />
                  ))}
                  {gridY.map((y) => (
                    <line key={y} x1={0} x2={100} y1={Y(y)} y2={Y(y)} stroke="var(--viz-grid)" strokeWidth={0.12} />
                  ))}
                  {memCeils.map((c) => {
                    const aiTop = Math.min(X_MAX, topCompute / c.value)
                    return (
                      <line
                        key={c.id}
                        x1={sx(X_MIN)}
                        y1={Y(Math.max(Y_MIN, c.value * X_MIN))}
                        x2={sx(aiTop)}
                        y2={Y(Math.min(topCompute, c.value * aiTop))}
                        stroke="var(--viz-axis)"
                        strokeWidth={c.id === "dram" ? 0.4 : 0.22}
                      />
                    )
                  })}
                  {compCeils.map((c) => (
                    <line
                      key={c.id}
                      x1={sx(c.value / Math.max(...memCeils.map((m) => m.value)))}
                      x2={100}
                      y1={Y(c.value)}
                      y2={Y(c.value)}
                      stroke="var(--viz-axis)"
                      strokeWidth={c.id === "fma" ? 0.4 : 0.22}
                    />
                  ))}
                  {memCeils.map((c) => {
                    const aiTop = Math.min(X_MAX, topCompute / c.value)
                    const midAi = Math.exp(Math.log(X_MIN) * 0.35 + Math.log(aiTop) * 0.65)
                    const lx = sx(midAi)
                    const ly = Y(c.value * midAi) - 1.2
                    return (
                      <text key={c.id + "-lb"} x={lx} y={ly} fontSize={1.9} fill="var(--viz-muted)" transform={`rotate(-23 ${lx} ${ly})`}>
                        {c.label}
                      </text>
                    )
                  })}
                  {compCeils.map((c) => (
                    <text key={c.id + "-lb"} x={99} y={Y(c.value) - 1} fontSize={1.9} textAnchor="end" fill="var(--viz-muted)">
                      {c.label}
                    </text>
                  ))}
                  {rl.loops.map((l) => {
                    const isSel = sel === l.id
                    return (
                      <g key={l.id} className="cursor-pointer" onClick={() => setSel(l.id)}>
                        <circle cx={sx(l.ai)} cy={Y(l.gflops)} r={dotR(l) + 2} fill="transparent" />
                        <circle
                          cx={sx(l.ai)}
                          cy={Y(l.gflops)}
                          r={dotR(l)}
                          fill={l.vectorized ? "var(--series-1)" : "var(--viz-surface)"}
                          stroke={isSel ? "var(--viz-ink)" : "var(--series-1)"}
                          strokeWidth={isSel ? 0.6 : 0.4}
                          opacity={l.quality === "low-confidence" ? 0.55 : 1}
                        />
                      </g>
                    )
                  })}
                  {rl.loops
                    .filter((l) => l.timeShare >= 0.09)
                    .map((l) => (
                      <text
                        key={l.id + "-lb"}
                        x={sx(l.ai) + dotR(l) + 1}
                        y={Y(l.gflops) + 0.7}
                        fontSize={1.9}
                        fill="var(--viz-ink-2)"
                        className="pointer-events-none"
                      >
                        {data.frames[l.frameId].name}
                      </text>
                    ))}
                  {gridX.map((x) => (
                    <text key={x + "-t"} x={sx(x)} y={57.5} fontSize={1.7} textAnchor="middle" fill="var(--viz-muted)">
                      {x}
                    </text>
                  ))}
                  {gridY.map((y) => (
                    <text key={y + "-t"} x={0.5} y={Math.max(3.4, Y(y) - 0.6)} fontSize={1.7} fill="var(--viz-muted)">
                      {y}
                    </text>
                  ))}
                </>
              )
            })()}
          </svg>
          <div className="mt-0.5 flex justify-between text-[10px] text-muted-foreground">
            <span>arithmetic intensity, FLOP/byte →</span>
            <span>↑ GFLOP/s</span>
          </div>
        </div>

        <div className="flex min-w-0 flex-col p-2 text-[11px]">
          {selected ? (
            <>
              <div className="flex items-baseline gap-2">
                <span className="truncate font-mono text-xs font-semibold">{data.frames[selected.frameId].name}</span>
                <span className="text-[10px] text-muted-foreground">{selected.location}</span>
                <span className="ml-auto">
                  <QualityBadge quality={selected.quality} />
                </span>
              </div>
              <dl className="mt-2 grid grid-cols-[110px_1fr] gap-y-1">
                <dt className="text-muted-foreground">Time share</dt>
                <dd className="tabular-nums">{fmtPct(selected.timeShare)}</dd>
                <dt className="text-muted-foreground">Intensity</dt>
                <dd className="tabular-nums">{selected.ai} FLOP/byte</dd>
                <dt className="text-muted-foreground">Throughput</dt>
                <dd className="tabular-nums">
                  {selected.gflops} GFLOP/s · {fmtPct(selected.gflops / limitAt(selected.ai), 0)} of the{" "}
                  {selected.bound} roof at this intensity
                </dd>
                <dt className="text-muted-foreground">Trips</dt>
                <dd className="tabular-nums">{fmtCount(selected.tripCount)}</dd>
                <dt className="text-muted-foreground">Vectorized</dt>
                <dd>{selected.vectorized ? "yes" : "no"}</dd>
              </dl>
              <div className="mt-2 rounded border bg-muted/30 px-2.5 py-2 leading-snug text-muted-foreground">
                {selected.note}
              </div>
              {data.sources.some((s) => s.frameId === selected.frameId) && (
                <Button
                  variant="link"
                  size="sm"
                  className="mt-1 h-6 self-start px-0 text-[10.5px] text-[var(--series-1)]"
                  onClick={() => {
                    setSelectedFrame(selected.frameId)
                    if (onOpenSource) onOpenSource(selected.frameId)
                    else onNavigate?.("source")
                  }}
                >
                  open source & disassembly →
                </Button>
              )}
            </>
          ) : (
            <span className="text-muted-foreground">Click a loop dot.</span>
          )}
        </div>
      </div>

      <div className="px-2 py-1.5 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
        Loops · by run-time share
      </div>
      <Table className="text-xs tabular-nums">
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className="h-7 px-2 text-[10.5px]">Loop</TableHead>
            <TableHead className="h-7 px-2 text-[10.5px]">Location</TableHead>
            <TableHead className="h-7 px-2 text-right text-[10.5px]">Time</TableHead>
            <TableHead className="h-7 px-2 text-right text-[10.5px]">AI</TableHead>
            <TableHead className="h-7 px-2 text-right text-[10.5px]">GFLOP/s</TableHead>
            <TableHead className="h-7 px-2 text-right text-[10.5px]">of roof</TableHead>
            <TableHead className="h-7 px-2 text-[10.5px]">Bound</TableHead>
            <TableHead className="h-7 px-2 text-[10.5px]">Vector</TableHead>
            <TableHead className="h-7 px-2 text-[10.5px]">Quality</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((l) => (
            <TableRow
              key={l.id}
              className={cn("cursor-pointer border-border/40", sel === l.id && "bg-accent")}
              onClick={() => setSel(l.id)}
            >
              <TableCell className="px-2 py-1 font-mono text-[11px]">{data.frames[l.frameId].name}</TableCell>
              <TableCell className="px-2 py-1 text-muted-foreground">{l.location}</TableCell>
              <TableCell className="relative px-2 py-1 text-right">
                <div
                  className="absolute inset-y-1 left-0 rounded-r-[2px] bg-[var(--series-1)]/16"
                  style={{ width: `${l.timeShare * 100 * 2}%` }}
                />
                <span className="relative">{fmtPct(l.timeShare)}</span>
              </TableCell>
              <TableCell className="px-2 py-1 text-right">{l.ai}</TableCell>
              <TableCell className="px-2 py-1 text-right">{l.gflops}</TableCell>
              <TableCell className="px-2 py-1 text-right">{fmtPct(l.gflops / limitAt(l.ai), 0)}</TableCell>
              <TableCell className="px-2 py-1">{l.bound}</TableCell>
              <TableCell className="px-2 py-1">{l.vectorized ? "yes" : "—"}</TableCell>
              <TableCell className="px-2 py-1">
                <QualityBadge quality={l.quality} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
