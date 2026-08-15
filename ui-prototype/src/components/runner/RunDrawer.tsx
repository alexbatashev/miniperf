import type { Scenario } from "@/lib/model"
import type { RunSim } from "@/components/runner/sim"
import { LogView, StageStepper } from "@/components/runner/common"
import { Button } from "@/components/ui/button"
import { CircleDot, FolderOpen, Square, X } from "lucide-react"

export function RunDrawer({
  sim,
  onClose,
  onOpenRecording,
}: {
  sim: RunSim
  onClose: () => void
  onOpenRecording: (scenario: Scenario) => void
}) {
  const run = sim.run
  if (!run) return null
  return (
    <div className="fixed inset-x-0 bottom-6 z-40 flex h-[210px] flex-col border-t bg-background shadow-[0_-4px_16px_rgb(0_0_0/0.08)]">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b bg-muted/40 px-2.5">
        <CircleDot className="size-3 text-[var(--series-8)]" />
        <span className="text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
          Profile runner
        </span>
        <StageStepper run={run} />
        <span className="tabular-nums text-[10.5px] text-muted-foreground">{sim.elapsedS.toFixed(0)}s</span>
        <Button variant="ghost" size="icon-xs" className="ml-auto text-muted-foreground" onClick={onClose} title="Hide — the run keeps going">
          <X className="size-3" />
        </Button>
      </div>
      <LogView run={run} className="min-h-0 flex-1 bg-muted/20 px-2.5 py-1.5" />
      <div className="flex h-9 shrink-0 items-center gap-2 border-t px-2.5">
        <span className="font-mono text-[10px] text-muted-foreground">
          {run.spec.target.label} · {run.spec.scenario.toLowerCase()} ·{" "}
          {run.spec.mode === "launch" ? run.spec.command : `pid ${run.spec.pid}`}
        </span>
        <div className="ml-auto flex items-center gap-2">
          {sim.running && (
            <Button variant="destructive" size="xs" className="gap-1" onClick={sim.stop} disabled={run.stopped}>
              <Square className="size-2.5" /> {run.stopped ? "stopping…" : "Stop"}
            </Button>
          )}
          {run.stage === "done" && (
            <Button
              size="xs"
              className="gap-1"
              onClick={() => {
                onOpenRecording(run.spec.scenario)
                sim.dismiss()
                onClose()
              }}
            >
              <FolderOpen className="size-3" /> Open recording
            </Button>
          )}
          {!sim.running && (
            <Button
              variant="ghost"
              size="xs"
              onClick={() => {
                sim.dismiss()
                onClose()
              }}
            >
              Dismiss
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
