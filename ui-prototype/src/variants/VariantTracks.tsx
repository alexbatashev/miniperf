import { useMemo, useState } from "react"
import type { ProfileData } from "@/lib/model"
import { FilterBar } from "@/components/FilterBar"
import { TimelineBrush } from "@/components/viz/TimelineBrush"
import { MetricsChart } from "@/components/viz/MetricsChart"
import { CallerCallee } from "@/components/viz/CallerCallee"
import { HotspotsTable } from "@/components/viz/HotspotsTable"
import { SourceView } from "@/components/viz/SourceView"
import { functionMetrics } from "@/lib/fn-metrics"
import { TmaMiniBar } from "@/components/viz/TmaMiniBar"
import { useFilter } from "@/lib/filter"
import { useScopedSamples, viewById } from "@/views"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Button } from "@/components/ui/button"
import { ChevronDown, ChevronRight, FileCode2, PanelRightClose, PanelRightOpen, X } from "lucide-react"

export function VariantTracks({ data }: { data: ProfileData }) {
  const samples = useScopedSamples(data)
  const detailTabs = data.meta.views.filter((v) => v !== "timeline" && v !== "callers" && v !== "source")
  const [active, setActive] = useState(() => {
    const p = new URLSearchParams(window.location.search)
    const name = p.get("src")
    if (name) {
      const src = data.sources.find((s) => data.frames[s.frameId].name === name)
      if (src) return `source:${src.frameId}`
    }
    const v = p.get("view")
    if (v && detailTabs.includes(v)) return v
    return detailTabs.includes("flamegraph") ? "flamegraph" : detailTabs[0]
  })
  const [sourceTabs, setSourceTabs] = useState<number[]>(() => {
    const name = new URLSearchParams(window.location.search).get("src")
    if (!name) return []
    const src = data.sources.find((s) => data.frames[s.frameId].name === name)
    return src ? [src.frameId] : []
  })
  const [tracksOpen, setTracksOpen] = useState(true)
  const [sideOpen, setSideOpen] = useState(true)
  const { selectedFrame } = useFilter()

  const pinned = useMemo(
    () => data.counters.filter((c) => data.meta.pinnedCounters.includes(c.id)),
    [data.counters, data.meta.pinnedCounters]
  )

  const hasSource = (frameId: number) => data.sources.some((s) => s.frameId === frameId)
  const openSource = (frameId: number) => {
    if (!hasSource(frameId)) return
    setSourceTabs((tabs) => (tabs.includes(frameId) ? tabs : [...tabs, frameId]))
    setActive(`source:${frameId}`)
  }
  const closeSource = (frameId: number) => {
    setSourceTabs((tabs) => tabs.filter((f) => f !== frameId))
    if (active === `source:${frameId}`) setActive(detailTabs.includes("hotspots") ? "hotspots" : detailTabs[0])
  }

  const activeSourceFrame = active.startsWith("source:") ? Number(active.slice(7)) : null
  const view = viewById(active)
  const selMetrics = selectedFrame !== null ? functionMetrics(data, selectedFrame) : null

  return (
    <div className="flex size-full min-h-0 flex-col">
      <FilterBar data={data} />

      <div className="shrink-0 border-b">
        <Button
          variant="ghost"
          size="sm"
          className="h-6 w-full justify-start gap-1 rounded-none bg-muted/40 px-2 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground"
          onClick={() => setTracksOpen(!tracksOpen)}
        >
          {tracksOpen ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
          Master timeline — drag anywhere to scope every view below
        </Button>
        {tracksOpen && (
          <>
            <TimelineBrush data={data} samples={samples} compact />
            <div className="border-t">
              <MetricsChart series={pinned} durationS={data.meta.durationS} height={38} gutter={92} />
            </div>
          </>
        )}
      </div>

      <div className="flex min-h-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex h-8 shrink-0 items-center gap-1 border-b bg-muted/30 px-1.5">
            <Tabs value={active} onValueChange={setActive} className="min-w-0">
              <TabsList className="h-7 gap-0.5 overflow-x-auto bg-transparent p-0">
                {detailTabs.map((id) => {
                  const v = viewById(id)
                  return (
                    <TabsTrigger key={id} value={id} className="h-6 flex-none px-2 text-[11px] data-active:shadow-sm">
                      <v.icon className="size-3" />
                      {v.title}
                    </TabsTrigger>
                  )
                })}
                {sourceTabs.map((fid) => (
                  <TabsTrigger key={fid} value={`source:${fid}`} className="h-6 flex-none gap-1 pl-2 pr-1 font-mono text-[11px] data-active:shadow-sm">
                    <FileCode2 className="size-3" />
                    {data.frames[fid].name}
                    <span
                      role="button"
                      tabIndex={-1}
                      className="rounded-sm p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
                      onMouseDown={(e) => {
                        e.stopPropagation()
                        e.preventDefault()
                      }}
                      onClick={(e) => {
                        e.stopPropagation()
                        closeSource(fid)
                      }}
                    >
                      <X className="size-3" />
                    </span>
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
            <Button
              variant="ghost"
              size="icon"
              className="ml-auto size-6 text-muted-foreground"
              onClick={() => setSideOpen(!sideOpen)}
              title={sideOpen ? "Hide details panel" : "Show details panel"}
            >
              {sideOpen ? <PanelRightClose className="size-3.5" /> : <PanelRightOpen className="size-3.5" />}
            </Button>
          </div>
          <div className="min-h-0 flex-1">
            {activeSourceFrame !== null ? (
              <SourceView data={data} frameId={activeSourceFrame} />
            ) : active === "hotspots" && sideOpen ? (
              <HotspotsTable data={data} samples={samples} onOpenSource={openSource} />
            ) : (
              view.render({ data, samples, onNavigate: setActive, onOpenSource: openSource })
            )}
          </div>
        </div>

        {sideOpen && (
          <div className="flex w-[300px] shrink-0 flex-col border-l">
            <div className="shrink-0 border-b bg-muted/30 px-2 py-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
              Selection
            </div>
            {selMetrics && selectedFrame !== null ? (
              <div className="shrink-0 border-b px-2 py-1.5 text-[11px]">
                <div className="flex items-center gap-1.5">
                  <span className="truncate font-mono font-medium">{data.frames[selectedFrame].name}</span>
                  {hasSource(selectedFrame) && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="ml-auto h-5 gap-1 px-1.5 text-[10px] text-[var(--series-1)]"
                      onClick={() => openSource(selectedFrame)}
                    >
                      <FileCode2 className="size-3" /> source
                    </Button>
                  )}
                </div>
                <div className="mt-1 grid grid-cols-3 gap-1 text-center">
                  <div className="rounded bg-muted/50 py-1">
                    <div className="text-[9px] uppercase text-muted-foreground">IPC</div>
                    <div className="font-semibold tabular-nums">{selMetrics.ipc.toFixed(2)}</div>
                  </div>
                  <div className="rounded bg-muted/50 py-1">
                    <div className="text-[9px] uppercase text-muted-foreground">LLC MPKI</div>
                    <div className="font-semibold tabular-nums">{selMetrics.llcMpki.toFixed(1)}</div>
                  </div>
                  <div className="rounded bg-muted/50 py-1">
                    <div className="text-[9px] uppercase text-muted-foreground">BE stall</div>
                    <div className="font-semibold tabular-nums">{Math.round(selMetrics.beStall * 100)}%</div>
                  </div>
                </div>
                {data.tma && <TmaMiniBar tma={selMetrics.tma} className="mt-1.5" />}
              </div>
            ) : (
              <div className="shrink-0 border-b px-2 py-2 text-[11px] text-muted-foreground">
                Click a function anywhere — flame graph, hotspots, top-down — to inspect it here. Double-click a
                hotspot to open its source.
              </div>
            )}
            <div className="min-h-0 flex-1">
              <CallerCallee data={data} samples={samples} />
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
