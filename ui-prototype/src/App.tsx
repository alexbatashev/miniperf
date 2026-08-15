import { useEffect, useState } from "react"
import type { Scenario } from "@/lib/model"
import { getProfile } from "@/lib/mock"
import { FilterProvider, isFilterEmpty, useFilter } from "@/lib/filter"
import { VariantStudio } from "@/variants/VariantStudio"
import { VariantTracks } from "@/variants/VariantTracks"
import { VariantWorkbench } from "@/variants/VariantWorkbench"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { TooltipProvider } from "@/components/ui/tooltip"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Runner } from "@/components/runner/Runner"
import { useRunSim } from "@/components/runner/sim"
import { ChevronDown, Moon, Sun } from "lucide-react"
import { fmtCount, fmtTimeS } from "@/lib/format"

type VariantId = "studio" | "tracks" | "workbench"

const VARIANTS: { id: VariantId; label: string; blurb: string }[] = [
  { id: "studio", label: "A · Studio", blurb: "analysis tabs + bottom dock (VTune-like)" },
  { id: "tracks", label: "B · Tracks", blurb: "persistent master timeline + details panel (nSight-like)" },
  { id: "workbench", label: "C · Workbench", blurb: "split panes + command palette" },
]

interface RecordingDef {
  id: string
  label: string
  scenario: Scenario
}

const RECORDINGS: RecordingDef[] = [
  { id: "tma", label: "results/physim-04", scenario: "Top-Down" },
  { id: "snapshot", label: "results/physim-snap-01", scenario: "Snapshot" },
  { id: "mem", label: "results/physim-mem-02", scenario: "Memory" },
  { id: "roofline", label: "results/physim-roofline-01", scenario: "Roofline" },
]

export function urlParam(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name)
}

function StatusBar({ variant, rec }: { variant: VariantId; rec: RecordingDef }) {
  const data = getProfile(rec.scenario)
  const { filter, selectedFrame } = useFilter()
  const filtered = !isFilterEmpty(filter) || selectedFrame !== null
  return (
    <div className="flex h-6 shrink-0 items-center gap-3 border-t bg-muted/40 px-2 text-[10.5px] text-muted-foreground">
      <span className="font-medium text-foreground/80">{rec.label.replace("results/", "")}</span>
      <span>{data.meta.scenario}</span>
      <span>{data.meta.cpuModel.replace("11th Gen Intel Core ", "")}</span>
      <span>
        {fmtTimeS(data.meta.durationS)} · {fmtCount(data.meta.sampleCount)} samples · {data.meta.sampleFreqHz} Hz
      </span>
      <span className={filtered ? "text-[var(--series-1)]" : ""}>{filtered ? "● filtered view" : "○ unfiltered"}</span>
      <span className="ml-auto hidden md:block">{VARIANTS.find((v) => v.id === variant)?.blurb}</span>
    </div>
  )
}

export default function App() {
  const [recId, setRecId] = useState<string>(() => {
    const r = urlParam("recording")
    return RECORDINGS.some((x) => x.id === r) ? r! : "tma"
  })
  const rec = RECORDINGS.find((r) => r.id === recId)!
  const data = getProfile(rec.scenario)

  const [variant, setVariant] = useState<VariantId>(() => {
    const v = urlParam("variant")
    return v === "tracks" || v === "workbench" ? v : "studio"
  })
  const [dark, setDark] = useState(() =>
    urlParam("dark") !== null ? urlParam("dark") !== "0" : window.matchMedia("(prefers-color-scheme: dark)").matches
  )
  const sim = useRunSim()
  const openRecording = (scenario: Scenario) => {
    const r = RECORDINGS.find((x) => x.scenario === scenario)
    if (r) setRecId(r.id)
  }

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark)
  }, [dark])

  return (
    <TooltipProvider delayDuration={200}>
    <FilterProvider key={recId}>
      <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
        <div className="flex h-9 shrink-0 items-center gap-3 border-b px-2.5">
          <span className="text-[13px] font-semibold tracking-tight">
            miniperf <span className="font-normal text-muted-foreground">· ui prototype</span>
          </span>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button className="flex items-center gap-1.5 rounded-md bg-muted px-2 py-0.5 font-mono text-[10.5px] text-foreground/90 hover:bg-accent">
                {rec.label}
                <span className="rounded-sm bg-[var(--series-1)]/12 px-1 font-sans text-[9px] font-medium text-[var(--series-1)]">
                  {rec.scenario}
                </span>
                <ChevronDown className="size-3 text-muted-foreground" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-72">
              {RECORDINGS.map((r) => (
                <DropdownMenuItem key={r.id} className="gap-2 font-mono text-xs" onClick={() => setRecId(r.id)}>
                  {r.label}
                  <span className="ml-auto font-sans text-[10px] text-muted-foreground">{r.scenario}</span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
          <Runner sim={sim} onOpenRecording={openRecording} />
          <div className="ml-auto flex items-center gap-2">
            <span className="text-[10.5px] text-muted-foreground">layout variant</span>
            <ToggleGroup type="single" value={variant} onValueChange={(v) => v && setVariant(v as VariantId)} className="h-7">
              {VARIANTS.map((v) => (
                <ToggleGroupItem key={v.id} value={v.id} className="h-7 px-2.5 text-[11px]">
                  {v.label}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
            <button
              className="rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-foreground"
              onClick={() => setDark(!dark)}
              title="Toggle theme"
            >
              {dark ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />}
            </button>
          </div>
        </div>

        <div className="min-h-0 flex-1">
          {variant === "studio" && <VariantStudio key={recId} data={data} />}
          {variant === "tracks" && <VariantTracks key={recId} data={data} />}
          {variant === "workbench" && <VariantWorkbench key={recId} data={data} />}
        </div>

        <StatusBar variant={variant} rec={rec} />
      </div>
    </FilterProvider>
    </TooltipProvider>
  )
}
