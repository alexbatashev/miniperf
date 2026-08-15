import type { ProfileData, UseMetric, UseResource } from "@/lib/model"
import { USE_FINDINGS } from "@/lib/mock"
import { VizCard } from "@/components/VizCard"
import { Meter } from "@/components/Meter"
import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"

function meterColor(m: UseMetric): string {
  const v = m.frac ?? 0
  if (m.kind === "err") return v > 0 ? "var(--status-critical)" : "var(--status-good)"
  if (m.kind === "sat") {
    if (v >= 0.3) return "var(--status-critical)"
    if (v >= 0.06) return "var(--status-serious)"
    return "var(--status-good)"
  }
  if (v >= 0.85) return "var(--status-serious)"
  if (v >= 0.7) return "var(--status-warn)"
  return "var(--series-1)"
}

function MetricRow({ m }: { m: UseMetric }) {
  return (
    <div className="grid grid-cols-[64px_1fr] items-center gap-x-2">
      <span className="text-[9.5px] uppercase tracking-wide text-muted-foreground">{m.label}</span>
      <span className="truncate text-[10.5px] tabular-nums" title={m.text}>
        {m.text}
      </span>
      <div />
      <Meter value={m.frac ?? 0} color={meterColor(m)} className="mt-0.5 h-[5px]" />
    </div>
  )
}

function Spark({ series }: { series: number[] }) {
  const W = 100
  const H = 100
  const d = series
    .map((v, i) => `${i === 0 ? "M" : "L"}${((i / (series.length - 1)) * W).toFixed(1)},${(H - v * H).toFixed(1)}`)
    .join("")
  return (
    <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="h-8 w-full">
      <path d={`${d}L${W},${H}L0,${H}Z`} fill="var(--series-1)" opacity={0.1} />
      <path d={d} fill="none" stroke="var(--series-1)" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
    </svg>
  )
}

function ResourceCard({ r }: { r: UseResource }) {
  return (
    <VizCard
      title={<span className="normal-case text-xs font-semibold tracking-normal text-foreground">{r.name}</span>}
      action={r.scopeNote}
      contentClassName="flex flex-col gap-1.5"
    >
      {r.metrics.map((m) => (
        <MetricRow key={m.label} m={m} />
      ))}
      <Spark series={r.utilSeries} />
      <div className="text-[10.5px] leading-snug text-muted-foreground">{r.detail}</div>
    </VizCard>
  )
}

const SEVERITY_STYLE: Record<string, string> = {
  high: "bg-[var(--status-critical)]/12 text-[var(--status-critical)]",
  medium: "bg-[var(--status-serious)]/14 text-[var(--status-serious)]",
  info: "bg-[var(--series-1)]/12 text-[var(--series-1)]",
}

export function UseView({ data }: { data: ProfileData }) {
  const use = data.use ?? []
  return (
    <div className="flex size-full min-h-0 flex-col overflow-auto">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(230px,1fr))] gap-2 p-2">
        {use.map((r) => (
          <ResourceCard key={r.id} r={r} />
        ))}
      </div>
      <div className="px-2 pb-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
        Findings · ranked
      </div>
      <div className="flex flex-col gap-1.5 px-2 pb-3">
        {USE_FINDINGS.map((f) => (
          <VizCard key={f.rank} contentClassName="text-[11px] leading-snug">
            <div className="flex items-center gap-2">
              <Badge className={cn("rounded-sm px-1.5 py-0 text-[9.5px] font-semibold uppercase", SEVERITY_STYLE[f.severity])}>
                {f.severity}
              </Badge>
              <span className="font-medium">{f.finding}</span>
              <span className="ml-auto text-[10px] uppercase text-muted-foreground">{f.resource}</span>
            </div>
            <div className="mt-1 text-muted-foreground">
              <span className="font-medium text-foreground/70">Evidence: </span>
              {f.evidence}
            </div>
            <div className="mt-0.5 text-muted-foreground">
              <span className="font-medium text-foreground/70">Try: </span>
              {f.recommendation}
            </div>
          </VizCard>
        ))}
      </div>
    </div>
  )
}
