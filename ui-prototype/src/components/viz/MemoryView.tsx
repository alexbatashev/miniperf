import type { ProfileData } from "@/lib/model"
import { MetricsChart } from "./MetricsChart"
import { fmtBytes, fmtPct } from "@/lib/format"
import { VizCard } from "@/components/VizCard"
import { Meter } from "@/components/Meter"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

function Panel({ title, sub, children }: { title: string; sub?: string; children: React.ReactNode }) {
  return (
    <VizCard title={title} action={sub} contentClassName="min-w-0">
      {children}
    </VizCard>
  )
}

function Stat({ label, value, sub, meter }: { label: string; value: string; sub?: string; meter?: number }) {
  return (
    <VizCard title={label} contentClassName="flex flex-col gap-1">
      <span className="truncate text-lg font-semibold leading-tight">{value}</span>
      {meter !== undefined && (
        <Meter value={meter} color={meter > 0.75 ? "var(--status-serious)" : "var(--series-1)"} />
      )}
      {sub && <span className="truncate text-[10.5px] text-muted-foreground">{sub}</span>}
    </VizCard>
  )
}

const CAP_MARKS = [
  { bytes: 48 * 1024, label: "L1d" },
  { bytes: 1.25 * 1024 * 1024, label: "L2" },
  { bytes: 12 * 1024 * 1024, label: "L3" },
]

export function MemoryView({ data }: { data: ProfileData }) {
  const m = data.memory!
  const memCounters = data.counters.filter((c) =>
    ["uncore_dram_read", "uncore_dram_write", "rss", "live_alloc", "allocs"].includes(c.id)
  )

  const xmin = Math.log2(m.missRatio[0].cacheBytes)
  const xmax = Math.log2(m.missRatio[m.missRatio.length - 1].cacheBytes)
  const missPath = m.missRatio
    .map((p, i) => {
      const x = ((Math.log2(p.cacheBytes) - xmin) / (xmax - xmin)) * 100
      const y = 100 - (p.missRatio / 0.45) * 100
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join("")

  const strideLabel = (s: number) =>
    s === 0 ? "same" : Math.abs(s) <= 5 ? `${s > 0 ? "+" : "−"}${2 ** Math.abs(s)}L` : `${s > 0 ? "+" : "−"}2^${Math.abs(s)}L`

  return (
    <div className="flex size-full min-h-0 flex-col gap-2 overflow-auto p-2">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(160px,1fr))] gap-2">
        <Stat
          label="DRAM bandwidth"
          value={`${m.achievedGBs} GB/s avg`}
          sub={`peak ${m.peakGBs} of ${m.roofGBs} GB/s roof (${Math.round((m.peakGBs / m.roofGBs) * 100)}%)`}
          meter={m.peakGBs / m.roofGBs}
        />
        <Stat label="Peak RSS" value={fmtBytes(m.peakRssBytes)} sub={`live allocations peak ${fmtBytes(m.liveAllocPeakBytes)}`} meter={m.peakRssBytes / (31.1 * 1024 ** 3)} />
        <Stat label="Touched footprint" value={fmtBytes(m.footprintBytes)} sub={`${fmtPct(m.coldFraction, 0)} of references are cold (first touch)`} />
        <Stat
          label="Cache-line use"
          value={fmtPct(m.spatialUtilization, 0)}
          sub={`of each ${m.lineSize} B line actually read — AoS layout wastes the rest`}
          meter={m.spatialUtilization}
        />
      </div>

      <Panel
        title="Bandwidth & residency over time"
        sub={`source: ${m.bandwidthSource.replaceAll("_", " ")} · scope: ${m.bandwidthScope.replaceAll("_", " ")}`}
      >
        <MetricsChart series={memCounters} durationS={data.meta.durationS} height={44} gutter={92} />
      </Panel>

      <div className="grid grid-cols-1 gap-2 lg:grid-cols-3">
        <Panel title="Miss ratio vs cache size" sub="from address trace">
          <svg viewBox="0 0 100 42" className="w-full">
            <g transform="scale(1,0.42)">
              {CAP_MARKS.map((c) => {
                const x = ((Math.log2(c.bytes) - xmin) / (xmax - xmin)) * 100
                return <line key={c.label} x1={x} x2={x} y1={0} y2={100} stroke="var(--viz-grid)" strokeWidth={0.4} />
              })}
              <path d={missPath} fill="none" stroke="var(--series-1)" strokeWidth={1.2} vectorEffect="non-scaling-stroke" />
            </g>
            {CAP_MARKS.map((c) => {
              const x = ((Math.log2(c.bytes) - xmin) / (xmax - xmin)) * 100
              return (
                <text key={c.label} x={x + 1} y={4} fontSize={3} fill="var(--viz-muted)">
                  {c.label}
                </text>
              )
            })}
            <text x={1} y={41} fontSize={2.6} fill="var(--viz-muted)">
              32 KiB
            </text>
            <text x={99} y={41} fontSize={2.6} textAnchor="end" fill="var(--viz-muted)">
              64 MiB
            </text>
          </svg>
          <div className="mt-1 text-[10.5px] leading-snug text-muted-foreground">
            Long plateau past L3: the working set doesn't fit until ~32 MiB — locality, not capacity tuning, is the lever.
          </div>
        </Panel>

        <Panel title="Access strides" sub="lines per consecutive access">
          <div className="flex h-[104px] items-end gap-1">
            {m.strides.map((s) => (
              <div key={s.strideLog2} className="flex min-w-0 flex-1 flex-col items-center gap-0.5">
                <span className="text-[9px] tabular-nums text-muted-foreground">{fmtPct(s.share, 0)}</span>
                <div
                  className="w-full max-w-6 rounded-t-[3px]"
                  style={{
                    height: `${Math.max(2, s.share * 220)}px`,
                    background: Math.abs(s.strideLog2) >= 6 ? "var(--series-2)" : "var(--series-1)",
                  }}
                />
                <span className="text-[8.5px] text-muted-foreground">{strideLabel(s.strideLog2)}</span>
              </div>
            ))}
          </div>
          <div className="mt-1 text-[10.5px] leading-snug text-muted-foreground">
            <span className="inline-block size-2 rounded-[2px] align-middle" style={{ background: "var(--series-2)" }} />{" "}
            44% of accesses jump ≥64 lines — the gather path. The +1-line share is the healthy streaming part.
          </div>
        </Panel>

        <Panel title="Reuse distance" sub="lines touched between reuses (log2)">
          <div className="flex h-[104px] items-end gap-1">
            {m.reuse.map((r) => (
              <div key={r.distLog2} className="flex min-w-0 flex-1 flex-col items-center gap-0.5">
                <span className="text-[9px] tabular-nums text-muted-foreground">{fmtPct(r.share, 0)}</span>
                <div
                  className="w-full max-w-6 rounded-t-[3px]"
                  style={{ height: `${Math.max(2, r.share * 300)}px`, background: "var(--series-1)" }}
                />
                <span className="text-[8.5px] text-muted-foreground">2^{r.distLog2}</span>
              </div>
            ))}
          </div>
          <div className="mt-1 text-[10.5px] leading-snug text-muted-foreground">
            Bimodal: tight register-level reuse plus a far mode at 2^12–2^16 lines that no realistic cache holds.
          </div>
        </Panel>
      </div>

      <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
        <Panel title="Working set by window" sub="bytes needed to cover a window of references">
          <Table className="text-[11px] tabular-nums">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="h-6 px-0 text-[10px]">Window (refs)</TableHead>
                <TableHead className="h-6 px-0 text-right text-[10px]">mean</TableHead>
                <TableHead className="h-6 px-0 text-right text-[10px]">p95</TableHead>
                <TableHead className="h-6 px-0 text-right text-[10px]">max</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {m.workingSet.map((w) => (
                <TableRow key={w.windowRefs} className="border-border/40 hover:bg-transparent">
                  <TableCell className="px-0 py-1">{w.windowRefs.toExponential(0).replace("e+", "e")}</TableCell>
                  <TableCell className="px-0 py-1 text-right">{fmtBytes(w.meanBytes)}</TableCell>
                  <TableCell className="px-0 py-1 text-right">{fmtBytes(w.p95Bytes)}</TableCell>
                  <TableCell className="px-0 py-1 text-right">{fmtBytes(w.maxBytes)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </Panel>
        <Panel title="Verdict">
          <div className="text-[11px] leading-snug text-muted-foreground">
            <span className="font-medium text-foreground">Bandwidth-limited, capacity fine. </span>
            RSS is stable at 20% of system memory — this is not a capacity problem. The cost is traffic: 41% cache-line
            use and a 44% random-stride share mean roughly 2.4× more DRAM bytes moved than the algorithm needs. Sorting
            particles by cell (SoA) attacks all three panels at once: strides collapse toward +1 line, line use rises,
            and the far reuse mode shrinks into L3.
          </div>
        </Panel>
      </div>
    </div>
  )
}
