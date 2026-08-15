import { useMemo, useState } from "react"
import type { ProfileData, SourceListing } from "@/lib/model"
import { useFilter } from "@/lib/filter"
import { fmtCount, fmtPct } from "@/lib/format"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { cn } from "@/lib/utils"
import { ChevronDown } from "lucide-react"

function heatColor(frac: number): string {
  if (frac <= 0) return "transparent"
  const RAMP = ["#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b"]
  return RAMP[Math.min(RAMP.length - 1, Math.floor(Math.sqrt(frac) * RAMP.length))]
}

function HeatCell({ frac, max }: { frac: number; max: number }) {
  return (
    <div className="relative h-[13px] w-12 shrink-0 self-center rounded-[2px] bg-[var(--viz-grid)]/40">
      {frac > 0 && (
        <div
          className="h-full rounded-[2px]"
          style={{ width: `${Math.max(4, (frac / max) * 100)}%`, background: heatColor(frac / max) }}
        />
      )}
    </div>
  )
}

export function SourceView({
  data,
  frameId,
  onNavigate,
}: {
  data: ProfileData
  frameId?: number
  onNavigate?: (id: string) => void
}) {
  void onNavigate
  const { selectedFrame, setSelectedFrame } = useFilter()
  const sources = data.sources

  const current: SourceListing | undefined = useMemo(() => {
    const target = frameId ?? selectedFrame
    const byFrame = sources.find((s) => s.frameId === target)
    return byFrame ?? sources[0]
  }, [sources, frameId, selectedFrame])

  const [focusLine, setFocusLine] = useState<number | null>(null)
  const [pinned, setPinned] = useState<number | null>(null)
  const activeLine = pinned ?? focusLine

  if (!current) {
    return (
      <div className="flex size-full items-center justify-center text-xs text-muted-foreground">
        No disassembly recorded for this scenario.
      </div>
    )
  }

  const fr = data.frames[current.frameId]
  const maxLine = Math.max(...current.lineSamples, 0.001)
  const maxAsm = Math.max(...current.asm.map((a) => a.samples), 0.001)
  const asmForLine = (ln: number) => current.asm.filter((a) => a.line === ln)

  return (
    <div className="flex size-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b px-2 text-xs">
        {frameId !== undefined ? (
          <span className="px-1.5 font-mono text-[11.5px] font-medium">{fr.name}</span>
        ) : (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button className="flex items-center gap-1.5 rounded px-1.5 py-0.5 font-mono text-[11.5px] font-medium hover:bg-accent">
                {fr.name}
                <ChevronDown className="size-3 text-muted-foreground" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {sources.map((s) => (
                <DropdownMenuItem
                  key={s.frameId}
                  className="gap-2 font-mono text-xs"
                  onClick={() => {
                    setSelectedFrame(s.frameId)
                    setPinned(null)
                  }}
                >
                  {data.frames[s.frameId].name}
                  <span className="ml-auto font-sans text-[10px] text-muted-foreground">{s.file}</span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        <span className="text-muted-foreground">{current.file}</span>
        <span className="ml-auto text-[10.5px] text-muted-foreground">
          {fmtCount(current.totalSamples)} samples in function · heat = share of function time · click a line to pin
        </span>
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-2">
        <div className="min-h-0 overflow-auto border-r py-1">
          {current.lines.map((text, i) => {
            const ln = current.startLine + i
            const frac = current.lineSamples[i]
            const active = activeLine === ln
            return (
              <div
                key={ln}
                className={cn(
                  "flex cursor-pointer gap-2 px-2 py-px font-mono text-[11.5px] leading-[17px]",
                  active ? "bg-[var(--series-1)]/12" : "hover:bg-accent/50"
                )}
                onMouseEnter={() => setFocusLine(ln)}
                onMouseLeave={() => setFocusLine(null)}
                onClick={() => setPinned(pinned === ln ? null : ln)}
              >
                <span className="w-8 shrink-0 select-none text-right text-[10px] leading-[17px] text-muted-foreground">
                  {ln}
                </span>
                <HeatCell frac={frac} max={maxLine} />
                <span className="whitespace-pre">{text}</span>
                {frac >= 0.1 && (
                  <span className="ml-auto shrink-0 self-center rounded-sm bg-[var(--series-1)]/12 px-1 text-[9.5px] font-sans text-[var(--series-1)]">
                    {fmtPct(frac, 0)}
                  </span>
                )}
              </div>
            )
          })}
          <div className="mx-2 mt-2 rounded border bg-muted/30 px-2.5 py-2 font-sans text-[11px] leading-snug text-muted-foreground">
            <span className="font-medium text-foreground">Reading this: </span>
            {current.summary}
          </div>
        </div>

        <div className="min-h-0 overflow-auto py-1">
          {current.asm.map((row) => {
            const active = activeLine === row.line
            return (
              <div
                key={row.addr}
                className={cn(
                  "flex gap-2 px-2 py-px font-mono text-[11.5px] leading-[17px]",
                  active ? "bg-[var(--series-1)]/12" : "hover:bg-accent/50"
                )}
                onMouseEnter={() => setFocusLine(row.line)}
                onMouseLeave={() => setFocusLine(null)}
                onClick={() => setPinned(pinned === row.line ? null : row.line)}
              >
                <span className="w-9 shrink-0 select-none text-right text-[10px] leading-[17px] text-muted-foreground">
                  {row.addr}
                </span>
                <HeatCell frac={row.samples} max={maxAsm} />
                <span className="whitespace-pre">{row.text}</span>
                {row.llcMissShare !== undefined && (
                  <span
                    className="ml-auto shrink-0 self-center whitespace-nowrap rounded-sm bg-[var(--series-2)]/14 px-1 font-sans text-[9.5px] text-[var(--series-2)]"
                    title={row.note}
                  >
                    {fmtPct(row.llcMissShare, 0)} LLC miss
                  </span>
                )}
              </div>
            )
          })}
          {activeLine !== null &&
            asmForLine(activeLine)
              .filter((r) => r.note)
              .map((r) => (
                <div
                  key={r.addr + "-note"}
                  className="mx-2 mt-1 rounded border border-[var(--series-2)]/30 bg-[var(--series-2)]/8 px-2.5 py-1.5 font-sans text-[11px] leading-snug"
                >
                  <span className="font-mono text-[10px] text-muted-foreground">{r.addr} </span>
                  {r.note}
                </div>
              ))}
        </div>
      </div>
    </div>
  )
}
