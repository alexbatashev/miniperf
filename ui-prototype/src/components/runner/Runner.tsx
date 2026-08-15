import { useState } from "react"
import type { Scenario } from "@/lib/model"
import type { RunSim } from "@/components/runner/sim"
import { RunChip } from "@/components/runner/common"
import { RunnerWizard } from "@/components/runner/RunnerWizard"
import { RunDrawer } from "@/components/runner/RunDrawer"
import { Button } from "@/components/ui/button"
import { CircleDot } from "lucide-react"

export function Runner({
  sim,
  onOpenRecording,
}: {
  sim: RunSim
  onOpenRecording: (scenario: Scenario) => void
}) {
  const [wizardOpen, setWizardOpen] = useState(false)
  const [drawerOpen, setDrawerOpen] = useState(false)

  return (
    <>
      {!sim.run && (
        <Button variant="outline" size="sm" className="h-6 gap-1 px-2 text-[10.5px]" onClick={() => setWizardOpen(true)}>
          <CircleDot className="size-3 text-[var(--series-8)]" /> New profile
        </Button>
      )}
      {sim.run && <RunChip sim={sim} onClick={() => setDrawerOpen(!drawerOpen)} />}

      <RunnerWizard
        open={wizardOpen}
        onOpenChange={setWizardOpen}
        onStart={(spec) => {
          sim.start(spec)
          setWizardOpen(false)
          setDrawerOpen(true)
        }}
      />

      {drawerOpen && <RunDrawer sim={sim} onClose={() => setDrawerOpen(false)} onOpenRecording={onOpenRecording} />}
    </>
  )
}
