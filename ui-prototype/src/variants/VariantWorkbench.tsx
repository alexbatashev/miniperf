import { useEffect, useMemo, useState } from "react"
import type { ProfileData } from "@/lib/model"
import { FilterBar } from "@/components/FilterBar"
import { computeHotspots } from "@/lib/derive"
import { useFilter } from "@/lib/filter"
import { availableViews, useScopedSamples, viewById } from "@/views"
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable"
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { cn } from "@/lib/utils"
import { ChevronDown, Command as CommandIcon } from "lucide-react"

function PaneHeader({
  viewId,
  setViewId,
  views,
  hint,
}: {
  viewId: string
  setViewId: (id: string) => void
  views: ReturnType<typeof availableViews>
  hint?: string
}) {
  const v = viewById(viewId)
  return (
    <div className="flex h-7 shrink-0 items-center gap-1 border-b bg-muted/30 px-1.5">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button className="flex items-center gap-1.5 rounded px-1.5 py-0.5 text-[11.5px] font-medium hover:bg-accent">
            <v.icon className="size-3.5" />
            {v.title}
            <ChevronDown className="size-3 text-muted-foreground" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {views.map((view) => (
            <DropdownMenuItem key={view.id} onClick={() => setViewId(view.id)} className="gap-2 text-xs">
              <view.icon className="size-3.5" />
              {view.title}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      {hint && <span className="ml-auto truncate text-[10px] text-muted-foreground">{hint}</span>}
    </div>
  )
}

export function VariantWorkbench({ data }: { data: ProfileData }) {
  const samples = useScopedSamples(data)
  const views = availableViews(data)
  const has = (id: string) => views.some((v) => v.id === id)
  const [topView, setTopView] = useState(() => {
    const v = new URLSearchParams(window.location.search).get("view")
    if (v && has(v)) return v
    return has("flamegraph") ? "flamegraph" : views[0].id
  })
  const [bottomView, setBottomView] = useState(() => {
    const v = new URLSearchParams(window.location.search).get("view2")
    if (v && has(v)) return v
    return has("hotspots") ? "hotspots" : views[Math.min(1, views.length - 1)].id
  })
  const [paletteOpen, setPaletteOpen] = useState(false)
  const { patch, clear, setSelectedFrame } = useFilter()

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault()
        setPaletteOpen((o) => !o)
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  const topFns = useMemo(() => computeHotspots(data, data.samples).slice(0, 25), [data])

  const top = viewById(topView)
  const bottom = viewById(bottomView)

  return (
    <div className="flex size-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1">
        <div className="flex w-11 shrink-0 flex-col items-center gap-0.5 border-r bg-muted/30 py-1.5">
          {views.map((v) => (
            <button
              key={v.id}
              title={`${v.title} — click: top pane · shift-click: bottom pane`}
              onClick={(e) => (e.shiftKey ? setBottomView(v.id) : setTopView(v.id))}
              className={cn(
                "rounded-md p-2",
                topView === v.id
                  ? "bg-[var(--series-1)]/15 text-[var(--series-1)]"
                  : bottomView === v.id
                    ? "bg-accent text-foreground"
                    : "text-muted-foreground hover:bg-accent hover:text-foreground"
              )}
            >
              <v.icon className="size-4" />
            </button>
          ))}
          <button
            title="Command palette (Ctrl+K)"
            onClick={() => setPaletteOpen(true)}
            className="mt-auto rounded-md p-2 text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <CommandIcon className="size-4" />
          </button>
        </div>

        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <FilterBar data={data} />
          <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
            <ResizablePanel defaultSize="62%" minSize="20%">
              <div className="flex size-full min-h-0 flex-col">
                <PaneHeader viewId={topView} setViewId={setTopView} views={views} hint="pane 1" />
                <div className="min-h-0 flex-1">{top.render({ data, samples, onNavigate: setTopView })}</div>
              </div>
            </ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize="38%" minSize="15%">
              <div className="flex size-full min-h-0 flex-col">
                <PaneHeader viewId={bottomView} setViewId={setBottomView} views={views} hint="pane 2" />
                <div className="min-h-0 flex-1">{bottom.render({ data, samples, onNavigate: setBottomView })}</div>
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>
      </div>

      <CommandDialog open={paletteOpen} onOpenChange={setPaletteOpen}>
        <CommandInput placeholder="Jump to view, function, thread…" />
        <CommandList>
          <CommandEmpty>Nothing found.</CommandEmpty>
          <CommandGroup heading="Views">
            {views.map((v) => (
              <CommandItem
                key={v.id}
                onSelect={() => {
                  setTopView(v.id)
                  setPaletteOpen(false)
                }}
                className="gap-2 text-xs"
              >
                <v.icon className="size-3.5" />
                {v.title}
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Functions">
            {topFns.map((f) => (
              <CommandItem
                key={f.frameId}
                value={data.frames[f.frameId].name + f.frameId}
                onSelect={() => {
                  setSelectedFrame(f.frameId)
                  setPaletteOpen(false)
                }}
                className="gap-2 font-mono text-xs"
              >
                {data.frames[f.frameId].name}
                <span className="ml-auto font-sans text-[10px] text-muted-foreground">
                  {(f.selfPct * 100).toFixed(1)}%
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Threads">
            {data.threads.map((t) => (
              <CommandItem
                key={t.tid}
                value={"thread " + t.name}
                onSelect={() => {
                  patch({ threads: [t.tid] })
                  setPaletteOpen(false)
                }}
                className="text-xs"
              >
                Filter to {t.name}
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Filters">
            <CommandItem
              onSelect={() => {
                clear()
                setSelectedFrame(null)
                setPaletteOpen(false)
              }}
              className="text-xs"
            >
              Clear all filters
            </CommandItem>
          </CommandGroup>
        </CommandList>
      </CommandDialog>
    </div>
  )
}
