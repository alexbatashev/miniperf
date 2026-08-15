import { useMemo } from "react"
import type { ProfileData } from "@/lib/model"
import { isFilterEmpty, useFilter } from "@/lib/filter"
import { fmtCount, fmtPct } from "@/lib/format"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { X, ChevronDown, Search, Clock } from "lucide-react"

function Chip({
  children,
  onClear,
  active = true,
}: {
  children: React.ReactNode
  onClear?: () => void
  active?: boolean
}) {
  return (
    <span
      className={
        "inline-flex h-6 shrink-0 items-center gap-1 rounded-md border px-2 text-[11px] " +
        (active
          ? "border-[var(--series-1)]/40 bg-[var(--series-1)]/10 text-foreground"
          : "border-border bg-background text-muted-foreground")
      }
    >
      {children}
      {onClear && (
        <button onClick={onClear} className="ml-0.5 rounded-sm text-muted-foreground hover:text-foreground">
          <X className="size-3" />
        </button>
      )}
    </span>
  )
}

export function FilterBar({ data, className = "" }: { data: ProfileData; className?: string }) {
  const { filter, patch, clear, selectedFrame, setSelectedFrame } = useFilter()

  const modules = useMemo(() => [...new Set(data.frames.map((f) => f.module))], [data.frames])

  const coverage = useMemo(() => {
    if (isFilterEmpty(filter)) return 1
    let inScope = 0
    const threadSet = filter.threads ? new Set(filter.threads) : null
    for (const s of data.samples) {
      if (filter.timeRange && (s.time < filter.timeRange[0] || s.time > filter.timeRange[1])) continue
      if (threadSet && !threadSet.has(s.tid)) continue
      inScope++
    }
    return inScope / data.samples.length
  }, [data.samples, filter])

  const anyFilter = !isFilterEmpty(filter) || selectedFrame !== null

  return (
    <div className={"flex h-9 items-center gap-1.5 overflow-x-auto border-b bg-muted/30 px-2 " + className}>
      <span className="mr-0.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Filter</span>

      {filter.timeRange ? (
        <Chip onClear={() => patch({ timeRange: null })}>
          <Clock className="size-3" />
          {filter.timeRange[0].toFixed(2)}s – {filter.timeRange[1].toFixed(2)}s
        </Chip>
      ) : (
        <Chip active={false}>
          <Clock className="size-3" /> full run
        </Chip>
      )}

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" className="h-6 gap-1 px-2 text-[11px] font-normal">
            {filter.threads ? `${filter.threads.length} thread${filter.threads.length > 1 ? "s" : ""}` : "all threads"}
            <ChevronDown className="size-3" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-52">
          {data.threads.map((t) => {
            const checked = filter.threads?.includes(t.tid) ?? true
            return (
              <DropdownMenuCheckboxItem
                key={t.tid}
                checked={checked}
                onCheckedChange={(on) => {
                  const cur = filter.threads ?? data.threads.map((x) => x.tid)
                  const next = on ? [...cur, t.tid] : cur.filter((id) => id !== t.tid)
                  patch({ threads: next.length === data.threads.length ? null : next })
                }}
                className="text-xs"
              >
                {t.name} <span className="ml-auto text-[10px] text-muted-foreground">{t.tid}</span>
              </DropdownMenuCheckboxItem>
            )
          })}
          <DropdownMenuSeparator />
          <DropdownMenuCheckboxItem checked={filter.threads === null} onCheckedChange={() => patch({ threads: null })} className="text-xs">
            All threads
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" className="h-6 gap-1 px-2 text-[11px] font-normal">
            {filter.modules ? filter.modules.join(", ") : "all modules"}
            <ChevronDown className="size-3" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-48">
          {modules.map((m) => {
            const checked = filter.modules?.includes(m) ?? true
            return (
              <DropdownMenuCheckboxItem
                key={m}
                checked={checked}
                onCheckedChange={(on) => {
                  const cur = filter.modules ?? modules
                  const next = on ? [...cur, m] : cur.filter((x) => x !== m)
                  patch({ modules: next.length === modules.length ? null : next })
                }}
                className="font-mono text-xs"
              >
                {m}
              </DropdownMenuCheckboxItem>
            )
          })}
          <DropdownMenuSeparator />
          <DropdownMenuCheckboxItem checked={filter.modules === null} onCheckedChange={() => patch({ modules: null })} className="text-xs">
            All modules
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <div className="relative shrink-0">
        <Search className="pointer-events-none absolute left-1.5 top-1/2 size-3 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={filter.symbolQuery}
          onChange={(e) => patch({ symbolQuery: e.target.value })}
          placeholder="match symbol…"
          className="h-6 w-40 rounded-md pl-6 text-[11px] md:text-[11px]"
        />
      </div>

      {selectedFrame !== null && (
        <Chip onClear={() => setSelectedFrame(null)}>
          <span className="font-mono">{data.frames[selectedFrame].name}</span>
        </Chip>
      )}

      <div className="ml-auto flex shrink-0 items-center gap-2">
        <span className="text-[10.5px] tabular-nums text-muted-foreground">
          {anyFilter && !isFilterEmpty(filter)
            ? `${fmtPct(coverage, 0)} of ${fmtCount(data.samples.length)} samples in scope`
            : `${fmtCount(data.samples.length)} samples`}
        </span>
        {anyFilter && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[11px]"
            onClick={() => {
              clear()
              setSelectedFrame(null)
            }}
          >
            Clear all
          </Button>
        )}
      </div>
    </div>
  )
}
