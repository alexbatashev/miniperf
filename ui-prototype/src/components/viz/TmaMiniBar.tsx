export const TMA_COLORS = {
  retiring: "var(--series-3)",
  badSpec: "var(--series-5)",
  frontend: "var(--series-4)",
  backend: "var(--series-2)",
} as const

export const TMA_LABELS = {
  retiring: "Retiring",
  badSpec: "Bad Speculation",
  frontend: "Frontend Bound",
  backend: "Backend Bound",
} as const

const TMA_SHORT = {
  retiring: "Ret",
  badSpec: "BadSpec",
  frontend: "FE",
  backend: "BE",
} as const

export function TmaMiniBar({
  tma,
  className = "",
}: {
  tma: { retiring: number; badSpec: number; frontend: number; backend: number }
  className?: string
}) {
  const parts = (["retiring", "badSpec", "frontend", "backend"] as const).map((k) => ({
    k,
    v: tma[k],
    color: TMA_COLORS[k],
  }))
  return (
    <div
      className={"flex h-2 w-full overflow-hidden rounded-[2px] " + className}
      title={parts.map((p) => `${TMA_LABELS[p.k]} ${(p.v * 100).toFixed(0)}%`).join(" · ")}
    >
      {parts.map((p) => (
        <div key={p.k} style={{ width: `${p.v * 100}%`, background: p.color }} className="mr-px h-full last:mr-0" />
      ))}
    </div>
  )
}

export function TmaLegend({ compact = false }: { compact?: boolean }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10.5px] text-muted-foreground">
      {(["retiring", "badSpec", "frontend", "backend"] as const).map((k) => (
        <span key={k} className="inline-flex items-center gap-1">
          <span className="inline-block size-2 rounded-[2px]" style={{ background: TMA_COLORS[k] }} />
          {compact ? TMA_SHORT[k] : TMA_LABELS[k]}
        </span>
      ))}
    </div>
  )
}
