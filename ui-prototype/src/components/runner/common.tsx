import { useEffect, useRef } from "react"
import type { Run, RunSim } from "@/components/runner/sim"
import { STAGE_LABEL, stageList } from "@/components/runner/sim"
import { Check, X } from "lucide-react"

export const inputCls = "h-7 font-mono text-[11px]"

export function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[92px_1fr] items-center gap-2">
      <span className="text-right text-[10.5px] text-muted-foreground">{label}</span>
      {children}
    </div>
  )
}

export function LogView({ run, className }: { run: Run; className?: string }) {
  const ref = useRef<HTMLDivElement>(null)
  const running = run.stage !== "done" && run.stage !== "failed"
  useEffect(() => {
    ref.current?.scrollTo({ top: ref.current.scrollHeight })
  }, [run.log.length])
  return (
    <div ref={ref} className={`overflow-y-auto font-mono text-[10.5px] leading-relaxed ${className ?? ""}`}>
      {run.log.map((l, i) => (
        <div
          key={i}
          className={
            l.startsWith("✓")
              ? "text-[var(--status-good)]"
              : l.includes("denied") || l.startsWith("set up")
                ? "text-[var(--status-critical)]"
                : l.startsWith("$")
                  ? "text-muted-foreground"
                  : ""
          }
        >
          {l}
        </div>
      ))}
      {running && <div className="animate-pulse text-muted-foreground">▌</div>}
    </div>
  )
}

export function RunChip({ sim, onClick }: { sim: RunSim; onClick: () => void }) {
  const run = sim.run
  if (!run) return null
  return (
    <button
      onClick={onClick}
      className={`flex h-6 items-center gap-1.5 rounded-md px-2 text-[10.5px] font-medium ${
        run.stage === "failed"
          ? "bg-[var(--status-critical)]/12 text-[var(--status-critical)]"
          : run.stage === "done"
            ? "bg-[var(--status-good)]/12 text-[var(--status-good)]"
            : "bg-[var(--series-8)]/10 text-[var(--series-8)]"
      }`}
    >
      {sim.running && <span className="size-1.5 animate-pulse rounded-full bg-current" />}
      {STAGE_LABEL[run.stage]}
      {sim.running && <span className="tabular-nums text-foreground/60">{sim.elapsedS.toFixed(0)}s</span>}
    </button>
  )
}

export function StageStepper({ run }: { run: Run }) {
  const stages = stageList(run.spec)
  const activeIdx =
    run.stage === "done" ? stages.length : stages.findIndex((s) => s.id === run.stage)
  return (
    <div className="flex items-center gap-1">
      {stages.map((s, i) => {
        const state =
          run.stage === "failed" && i === Math.max(0, activeIdx)
            ? "failed"
            : i < activeIdx
              ? "done"
              : i === activeIdx
                ? "active"
                : "todo"
        return (
          <div key={s.id} className="flex items-center gap-1">
            {i > 0 && <div className={`h-px w-5 ${i <= activeIdx ? "bg-[var(--series-1)]" : "bg-border"}`} />}
            <div
              className={`flex items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium ${
                state === "done"
                  ? "border-transparent bg-[var(--series-1)]/10 text-[var(--series-1)]"
                  : state === "active"
                    ? "border-[var(--series-1)] text-foreground"
                    : state === "failed"
                      ? "border-[var(--status-critical)] text-[var(--status-critical)]"
                      : "text-muted-foreground"
              }`}
            >
              {state === "done" && <Check className="size-2.5" />}
              {state === "failed" && <X className="size-2.5" />}
              {state === "active" && <span className="size-1.5 animate-pulse rounded-full bg-[var(--series-1)]" />}
              {s.label}
            </div>
          </div>
        )
      })}
    </div>
  )
}
