import { useMemo } from "react"
import type { ProfileData, Sample } from "@/lib/model"
import { computeHotspots } from "@/lib/derive"
import { functionMetrics } from "@/lib/fn-metrics"
import { USE_FINDINGS } from "@/lib/mock"
import { useFilter } from "@/lib/filter"
import { fmtBytes, fmtCount, fmtPct, fmtTimeS } from "@/lib/format"
import { TmaLegend, TmaMiniBar, TMA_COLORS } from "./TmaMiniBar"
import { VizCard } from "@/components/VizCard"
import { Button } from "@/components/ui/button"

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <VizCard title={label} contentClassName="flex min-w-0 flex-col gap-0.5">
      <span className="truncate text-lg font-semibold leading-tight">{value}</span>
      {sub && <span className="truncate text-[10.5px] text-muted-foreground">{sub}</span>}
    </VizCard>
  )
}

function Block({
  title,
  linkLabel,
  onLink,
  children,
}: {
  title: string
  linkLabel?: string
  onLink?: () => void
  children: React.ReactNode
}) {
  return (
    <VizCard
      title={title}
      action={
        linkLabel && (
          <Button variant="link" size="sm" className="h-4 px-0 text-[10.5px] text-[var(--series-1)]" onClick={onLink}>
            {linkLabel} →
          </Button>
        )
      }
      contentClassName="px-1.5 pb-0"
    >
      {children}
    </VizCard>
  )
}

const SEVERITY_DOT: Record<string, string> = {
  high: "var(--status-critical)",
  medium: "var(--status-serious)",
  info: "var(--series-1)",
}

export function SummaryView({
  data,
  samples,
  onNavigate,
}: {
  data: ProfileData
  samples: Sample[]
  onNavigate?: (view: string) => void
}) {
  const { setSelectedFrame } = useFilter()
  const top = useMemo(
    () => computeHotspots(data, samples).slice(0, 7).map((r) => ({ ...r, m: functionMetrics(data, r.frameId) })),
    [data, samples]
  )
  const l1 = data.tma?.children ?? []
  const scenario = data.meta.scenario

  return (
    <div className="size-full min-h-0 overflow-auto p-2">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(150px,1fr))] gap-2">
        <Stat label="Elapsed" value={fmtTimeS(data.meta.durationS)} sub={`${fmtCount(samples.length)} samples in scope`} />
        <Stat
          label="CPU time"
          value={fmtTimeS(samples.length / data.meta.sampleFreqHz)}
          sub={`${(samples.length / data.meta.sampleFreqHz / data.meta.durationS).toFixed(1)} of ${data.meta.cores} logical CPUs avg`}
        />
        <Stat label="Verdict" value={data.meta.headline.verdict} sub={data.meta.headline.sub} />
        {scenario === "Top-Down" && <Stat label="IPC" value="1.54" sub="cycles 22.1G/s · instr 34.0G/s" />}
        {scenario === "Snapshot" && <Stat label="Peak DRAM" value="36.2 GB/s" sub="79% of calibrated roof (uncore IMC)" />}
        {data.memory && (
          <Stat
            label="DRAM bandwidth"
            value={`${data.memory.achievedGBs} GB/s`}
            sub={`peak ${data.memory.peakGBs} · roof ${data.memory.roofGBs} GB/s`}
          />
        )}
        {data.roofline && (
          <Stat
            label="Hottest loop"
            value="AI 0.06"
            sub="gather_neighbors · 2.6 GFLOP/s on the DRAM roof"
          />
        )}
      </div>

      <div className="mt-2 grid grid-cols-1 gap-2 xl:grid-cols-2">
        {data.tma && (
          <Block title="Top-down level 1" linkLabel="open Top-Down" onLink={() => onNavigate?.("tma")}>
            <div className="px-2.5 pb-2">
              <div className="flex h-6 w-full overflow-hidden rounded-[3px]">
                {l1.map((n) => {
                  const key =
                    n.id === "retiring" ? "retiring" : n.id === "bad_speculation" ? "badSpec" : n.id === "fe_bound" ? "frontend" : "backend"
                  return (
                    <div
                      key={n.id}
                      className="mr-px flex items-center justify-center overflow-hidden text-[10px] font-medium text-white last:mr-0"
                      style={{ width: `${n.value * 100}%`, background: TMA_COLORS[key as keyof typeof TMA_COLORS] }}
                      title={`${n.name} ${fmtPct(n.value)}`}
                    >
                      {n.value > 0.12 ? `${n.short} ${Math.round(n.value * 100)}%` : ""}
                    </div>
                  )
                })}
              </div>
              <div className="mt-1.5">
                <TmaLegend />
              </div>
            </div>
          </Block>
        )}

        {data.use && (
          <Block title="Findings" linkLabel="open Resources" onLink={() => onNavigate?.("use")}>
            <div className="px-1 pb-1.5">
              {USE_FINDINGS.map((f) => (
                <div key={f.rank} className="flex items-center gap-2 rounded px-1.5 py-1 text-[11px] hover:bg-accent/50">
                  <span className="size-2 shrink-0 rounded-full" style={{ background: SEVERITY_DOT[f.severity] }} />
                  <span className="truncate">{f.finding}</span>
                  <span className="ml-auto shrink-0 text-[10px] uppercase text-muted-foreground">{f.resource}</span>
                </div>
              ))}
            </div>
          </Block>
        )}

        {data.roofline && (
          <Block title="Loops vs roofs" linkLabel="open Roofline" onLink={() => onNavigate?.("roofline")}>
            <div className="px-1 pb-1.5">
              {data.roofline.loops
                .slice()
                .sort((a, b) => b.timeShare - a.timeShare)
                .slice(0, 4)
                .map((l) => (
                  <div key={l.id} className="flex items-center gap-2 rounded px-1.5 py-1 text-[11px] hover:bg-accent/50">
                    <span className="truncate font-mono">{data.frames[l.frameId].name}</span>
                    <span className="shrink-0 text-[10px] text-muted-foreground">
                      {fmtPct(l.timeShare, 0)} time · AI {l.ai} · {l.bound}-bound
                    </span>
                    <span className="ml-auto shrink-0 text-[10px] text-muted-foreground">{l.vectorized ? "vector" : "scalar"}</span>
                  </div>
                ))}
            </div>
          </Block>
        )}

        {data.memory && (
          <Block title="Memory diagnosis" linkLabel="open Memory" onLink={() => onNavigate?.("memory")}>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1 px-2.5 pb-2 text-[11px]">
              <span className="text-muted-foreground">Peak RSS</span>
              <span className="tabular-nums">{fmtBytes(data.memory.peakRssBytes)}</span>
              <span className="text-muted-foreground">Touched footprint</span>
              <span className="tabular-nums">{fmtBytes(data.memory.footprintBytes)}</span>
              <span className="text-muted-foreground">Cache-line use</span>
              <span className="tabular-nums">{fmtPct(data.memory.spatialUtilization, 0)}</span>
              <span className="text-muted-foreground">Peak bandwidth</span>
              <span className="tabular-nums">
                {data.memory.peakGBs} of {data.memory.roofGBs} GB/s
              </span>
            </div>
          </Block>
        )}
      </div>

      <div className="mt-2">
        <Block title="Top hotspots by self time" linkLabel="open Hotspots" onLink={() => onNavigate?.("hotspots")}>
          <div className="pb-1">
          {top.map((r) => (
            <button
              key={r.frameId}
              className="grid w-full grid-cols-[minmax(0,1fr)_56px_110px_minmax(80px,140px)] items-center gap-2 px-2.5 py-[3.5px] text-left text-[11px] hover:bg-accent/60"
              onClick={() => {
                setSelectedFrame(r.frameId)
                onNavigate?.("hotspots")
              }}
            >
              <span className="truncate font-mono">{data.frames[r.frameId].name}</span>
              <span className="text-right tabular-nums">{fmtPct(r.selfPct)}</span>
              <div className="h-[7px] rounded-[2px] bg-[var(--viz-grid)]/50">
                <div
                  className="h-full rounded-[2px] bg-[var(--series-1)]"
                  style={{ width: `${Math.min(100, r.selfPct * 100 * 2.4)}%` }}
                />
              </div>
              {data.tma ? <TmaMiniBar tma={r.m.tma} /> : <span />}
            </button>
          ))}
          </div>
        </Block>
      </div>

      <div className="mt-2 grid grid-cols-1 gap-2 lg:grid-cols-2">
        <VizCard title="Recording" contentClassName="text-[11px]">
          <dl className="grid grid-cols-[90px_1fr] gap-y-0.5">
            <dt className="text-muted-foreground">Scenario</dt>
            <dd>{data.meta.scenario}</dd>
            <dt className="text-muted-foreground">Command</dt>
            <dd className="truncate font-mono text-[10.5px]">{data.meta.command}</dd>
            <dt className="text-muted-foreground">CPU</dt>
            <dd>{data.meta.cpuModel}</dd>
            <dt className="text-muted-foreground">Sampling</dt>
            <dd>
              {data.meta.sampleFreqHz} Hz · {fmtCount(data.meta.sampleCount)} samples
            </dd>
            <dt className="text-muted-foreground">Started</dt>
            <dd>
              {data.meta.startedAt} · {data.meta.hostname}
            </dd>
          </dl>
        </VizCard>
        <VizCard title="Recorded events & collectors" contentClassName="text-[11px]">
          <div className="flex flex-wrap gap-1">
            {data.meta.events.map((e) => (
              <span key={e} className="rounded-sm bg-muted px-1.5 py-0.5 font-mono text-[10px]">
                {e}
              </span>
            ))}
          </div>
        </VizCard>
      </div>
    </div>
  )
}
