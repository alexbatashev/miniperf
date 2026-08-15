import { useMemo, useState } from "react"
import type { FlameMetric, ProfileData, Sample } from "@/lib/model"
import { applyFilter, useFilter } from "@/lib/filter"
import { buildFlameTree } from "@/lib/derive"
import { fmtBytes, fmtCount } from "@/lib/format"
import { FlameGraph } from "@/components/viz/FlameGraph"
import { FlameScope } from "@/components/viz/FlameScope"
import { HotspotsTable } from "@/components/viz/HotspotsTable"
import { CallerCallee } from "@/components/viz/CallerCallee"
import { TmaView } from "@/components/viz/TmaView"
import { UseView } from "@/components/viz/UseView"
import { SummaryView } from "@/components/viz/SummaryView"
import { MetricsChart } from "@/components/viz/MetricsChart"
import { TimelineBrush } from "@/components/viz/TimelineBrush"
import { SourceView } from "@/components/viz/SourceView"
import { RooflineView } from "@/components/viz/RooflineView"
import { MemoryView } from "@/components/viz/MemoryView"
import { CoresView } from "@/components/viz/CoresView"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { Separator } from "@/components/ui/separator"
import {
  LayoutDashboard,
  Flame,
  Grid3x3,
  Table2,
  GitFork,
  Layers,
  Gauge,
  LineChart,
  FileCode2,
  Mountain,
  MemoryStick,
  Cpu,
} from "lucide-react"

export function useScopedSamples(data: ProfileData): Sample[] {
  const { filter } = useFilter()
  return useMemo(() => applyFilter(data, filter), [data, filter])
}

const METRIC_LABEL: Record<FlameMetric, string> = {
  cycles: "Cycles",
  instructions: "Instructions",
  alloc: "Alloc bytes",
}

export function FlamegraphView({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const [mode, setMode] = useState<"topdown" | "bottomup">("topdown")
  const [metric, setMetric] = useState<FlameMetric>("cycles")
  const { filter } = useFilter()
  const activeMetric = data.meta.flameMetrics.includes(metric) ? metric : "cycles"

  const allocScoped = useMemo(
    () => (activeMetric === "alloc" ? applyFilter(data, filter, data.allocSamples) : []),
    [data, filter, activeMetric]
  )
  const tree = useMemo(
    () =>
      buildFlameTree(
        data,
        activeMetric === "alloc" ? allocScoped : samples,
        mode === "bottomup",
        activeMetric === "alloc" ? "cycles" : activeMetric
      ),
    [data, samples, allocScoped, mode, activeMetric]
  )
  const formatValue =
    activeMetric === "alloc"
      ? (n: number) => fmtBytes(n) + " allocated"
      : activeMetric === "instructions"
        ? (n: number) => fmtCount(n * 6e6) + " instructions"
        : (n: number) => `${fmtCount(n)} samples`

  return (
    <div className="flex size-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b px-2">
        <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">Stacks</span>
        <ToggleGroup
          type="single"
          variant="outline"
          value={mode}
          onValueChange={(v) => v && setMode(v as typeof mode)}
          className="h-6"
        >
          <ToggleGroupItem value="topdown" className="h-6 px-2 text-[11px]">
            Top-down
          </ToggleGroupItem>
          <ToggleGroupItem value="bottomup" className="h-6 px-2 text-[11px]">
            Bottom-up
          </ToggleGroupItem>
        </ToggleGroup>
        {data.meta.flameMetrics.length > 1 && (
          <>
            <Separator orientation="vertical" className="mx-1 h-4!" />
            <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">Weight</span>
            <ToggleGroup
              type="single"
              variant="outline"
              value={activeMetric}
              onValueChange={(v) => v && setMetric(v as FlameMetric)}
              className="h-6"
            >
              {data.meta.flameMetrics.map((m) => (
                <ToggleGroupItem key={m} value={m} className="h-6 px-2 text-[11px]">
                  {METRIC_LABEL[m]}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </>
        )}
        <span className="ml-auto text-[10.5px] text-muted-foreground">
          {activeMetric === "alloc"
            ? "width = bytes allocated at this call path"
            : "color = module kind · matches of symbol filter stay lit"}
        </span>
      </div>
      <div className="min-h-0 flex-1">
        <FlameGraph data={data} root={tree} inverted={mode === "bottomup"} formatValue={formatValue} />
      </div>
    </div>
  )
}

export function TimelineView({ data, samples }: { data: ProfileData; samples: Sample[] }) {
  const processCounters = data.counters.filter((c) => !c.uncore)
  const uncoreCounters = data.counters.filter((c) => c.uncore)
  return (
    <div className="flex size-full min-h-0 flex-col overflow-auto">
      <div className="px-2 pt-1.5 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
        Thread activity · drag to filter
      </div>
      <TimelineBrush data={data} samples={samples} />
      <div className="border-t px-2 pt-1.5 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
        Counters · process scope
      </div>
      <MetricsChart series={processCounters} durationS={data.meta.durationS} height={44} gutter={92} />
      {uncoreCounters.length > 0 && (
        <>
          <div className="flex items-baseline justify-between border-t bg-muted/30 px-2 py-1">
            <span className="text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
              System · uncore
            </span>
            <span className="text-[10px] text-muted-foreground">
              socket-wide — follows the time filter, ignores thread/symbol filters
            </span>
          </div>
          <MetricsChart series={uncoreCounters} durationS={data.meta.durationS} height={44} gutter={92} />
        </>
      )}
    </div>
  )
}

export function HotspotsSplitView({
  data,
  samples,
  onOpenSource,
  onNavigate,
}: {
  data: ProfileData
  samples: Sample[]
  onOpenSource?: (frameId: number) => void
  onNavigate?: (id: string) => void
}) {
  const { setSelectedFrame } = useFilter()
  onOpenSource ??= (frameId) => {
    if (data.sources.some((s) => s.frameId === frameId) && data.meta.views.includes("source")) {
      setSelectedFrame(frameId)
      onNavigate?.("source")
    }
  }
  return (
    <div className="grid size-full min-h-0 grid-cols-[minmax(0,3fr)_minmax(280px,2fr)]">
      <div className="min-h-0 border-r">
        <HotspotsTable data={data} samples={samples} onOpenSource={onOpenSource} />
      </div>
      <div className="min-h-0">
        <CallerCallee data={data} samples={samples} />
      </div>
    </div>
  )
}

export interface ViewRenderProps {
  data: ProfileData
  samples: Sample[]
  onNavigate?: (id: string) => void
  onOpenSource?: (frameId: number) => void
}

export interface ViewDef {
  id: string
  title: string
  icon: React.ComponentType<{ className?: string }>
  render: (props: ViewRenderProps) => React.ReactNode
}

export const VIEWS: ViewDef[] = [
  { id: "summary", title: "Summary", icon: LayoutDashboard, render: (p) => <SummaryView {...p} /> },
  { id: "hotspots", title: "Hotspots", icon: Table2, render: (p) => <HotspotsSplitView {...p} /> },
  { id: "flamegraph", title: "Flame Graph", icon: Flame, render: (p) => <FlamegraphView {...p} /> },
  { id: "flamescope", title: "Flame Scope", icon: Grid3x3, render: (p) => <FlameScope {...p} /> },
  { id: "callers", title: "Callers / Callees", icon: GitFork, render: (p) => <CallerCallee {...p} /> },
  { id: "timeline", title: "Timeline", icon: LineChart, render: (p) => <TimelineView {...p} /> },
  { id: "cores", title: "Cores", icon: Cpu, render: (p) => <CoresView {...p} /> },
  { id: "tma", title: "Top-Down", icon: Layers, render: (p) => <TmaView {...p} /> },
  { id: "use", title: "Resources", icon: Gauge, render: (p) => <UseView data={p.data} /> },
  { id: "memory", title: "Memory", icon: MemoryStick, render: (p) => <MemoryView data={p.data} /> },
  { id: "roofline", title: "Roofline", icon: Mountain, render: (p) => <RooflineView data={p.data} onNavigate={p.onNavigate} onOpenSource={p.onOpenSource} /> },
  { id: "source", title: "Source / Asm", icon: FileCode2, render: (p) => <SourceView data={p.data} onNavigate={p.onNavigate} /> },
]

export function viewById(id: string): ViewDef {
  return VIEWS.find((v) => v.id === id) ?? VIEWS[0]
}

export function availableViews(data: ProfileData): ViewDef[] {
  return data.meta.views.map(viewById)
}
