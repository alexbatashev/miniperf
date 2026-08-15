import { useState } from "react"
import type { ProfileData } from "@/lib/model"
import { FilterBar } from "@/components/FilterBar"
import { HotspotsTable } from "@/components/viz/HotspotsTable"
import { CallerCallee } from "@/components/viz/CallerCallee"
import { useScopedSamples, viewById } from "@/views"
import { cn } from "@/lib/utils"
import { ChevronDown, ChevronUp } from "lucide-react"

export function VariantStudio({ data }: { data: ProfileData }) {
  const samples = useScopedSamples(data)
  const topTabs = data.meta.views.filter((v) => v !== "callers")
  const [active, setActive] = useState(() => {
    const v = new URLSearchParams(window.location.search).get("view")
    return v && topTabs.includes(v) ? v : "summary"
  })
  const [dockOpen, setDockOpen] = useState(true)
  const [dockTab, setDockTab] = useState<"hotspots" | "callers">("hotspots")

  const view = viewById(active)
  const showDock = dockOpen && active !== "hotspots" && active !== "summary"

  return (
    <div className="flex size-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-end gap-0 border-b bg-muted/40 px-1.5">
        {topTabs.map((id) => {
          const v = viewById(id)
          return (
            <button
              key={id}
              onClick={() => setActive(id)}
              className={cn(
                "relative flex h-7 items-center gap-1.5 rounded-t-md border border-b-0 px-3 text-[11.5px]",
                active === id
                  ? "z-10 -mb-px border-border bg-background font-medium"
                  : "border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground"
              )}
            >
              <v.icon className="size-3.5" />
              {v.title}
            </button>
          )
        })}
      </div>

      <FilterBar data={data} />

      <div className="min-h-0 flex-1">{view.render({ data, samples, onNavigate: setActive })}</div>

      <div className="shrink-0 border-t">
        <div className="flex h-7 items-center gap-1 bg-muted/40 px-1.5">
          {(
            [
              ["hotspots", "Hotspots"],
              ["callers", "Callers & Callees"],
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              onClick={() => {
                setDockTab(id)
                setDockOpen(true)
              }}
              className={cn(
                "rounded px-2 py-0.5 text-[11px]",
                showDock && dockTab === id ? "bg-background font-medium shadow-sm" : "text-muted-foreground hover:text-foreground"
              )}
            >
              {label}
            </button>
          ))}
          <button
            className="ml-auto flex items-center gap-1 rounded px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground"
            onClick={() => setDockOpen(!dockOpen)}
          >
            {showDock ? <ChevronDown className="size-3.5" /> : <ChevronUp className="size-3.5" />}
          </button>
        </div>
        {showDock && (
          <div className="h-[200px] border-t">
            {dockTab === "hotspots" ? (
              <HotspotsTable data={data} samples={samples} dense limit={40} />
            ) : (
              <CallerCallee data={data} samples={samples} />
            )}
          </div>
        )}
      </div>
    </div>
  )
}
