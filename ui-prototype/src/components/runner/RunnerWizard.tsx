import { useEffect, useState } from "react"
import type { Scenario } from "@/lib/model"
import type { RunSpec, Target } from "@/components/runner/sim"
import { SCENARIOS, TARGETS, processesFor, useSpec } from "@/components/runner/sim"
import { inputCls } from "@/components/runner/common"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { ArrowLeft, ArrowRight, Check, Play, Plus, Server, Terminal } from "lucide-react"

const STEPS = ["Target", "Workload", "Recording"] as const

function WizardSteps({ step }: { step: number }) {
  return (
    <div className="flex items-center gap-1.5">
      {STEPS.map((label, i) => (
        <div key={label} className="flex items-center gap-1.5">
          {i > 0 && <div className={`h-px w-6 ${i <= step ? "bg-[var(--series-1)]" : "bg-border"}`} />}
          <div
            className={`flex items-center gap-1.5 text-[11px] ${
              i === step ? "font-medium text-foreground" : i < step ? "text-[var(--series-1)]" : "text-muted-foreground"
            }`}
          >
            <span
              className={`flex size-4.5 items-center justify-center rounded-full text-[10px] font-semibold ${
                i < step
                  ? "bg-[var(--series-1)]/12 text-[var(--series-1)]"
                  : i === step
                    ? "bg-primary text-primary-foreground"
                    : "bg-muted text-muted-foreground"
              }`}
            >
              {i < step ? <Check className="size-2.5" /> : i + 1}
            </span>
            {label}
          </div>
        </div>
      ))}
    </div>
  )
}

function SelectCard({
  selected,
  onSelect,
  title,
  detail,
  icon,
  mono,
}: {
  selected: boolean
  onSelect: () => void
  title: string
  detail: string
  icon?: React.ReactNode
  mono?: boolean
}) {
  return (
    <button
      onClick={onSelect}
      className={`flex flex-col gap-0.5 rounded-lg border p-2.5 text-left transition-colors ${
        selected ? "border-[var(--series-1)] bg-[var(--series-1)]/5" : "hover:bg-muted/50"
      }`}
    >
      <span className={`flex items-center gap-1.5 text-[11.5px] font-medium ${mono ? "font-mono" : ""}`}>
        {icon}
        {title}
      </span>
      <span className="text-[10px] text-muted-foreground">{detail}</span>
    </button>
  )
}

export function RunnerWizard({
  open,
  onOpenChange,
  onStart,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onStart: (spec: RunSpec) => void
}) {
  const [step, setStep] = useState(0)
  const [procFilter, setProcFilter] = useState("")
  const { spec, set, oneOffHost, setOneOffHost, finalSpec } = useSpec()

  useEffect(() => {
    if (open) setStep(0)
  }, [open])

  const procs = processesFor(spec.target).filter(
    (p) =>
      procFilter === "" ||
      p.name.toLowerCase().includes(procFilter.toLowerCase()) ||
      String(p.pid).includes(procFilter)
  )
  const maxCpu = Math.max(...processesFor(spec.target).map((p) => p.cpu), 1)

  const stepValid =
    step === 0
      ? spec.target.id !== "oneoff" || oneOffHost.trim() !== ""
      : step === 1
        ? spec.mode === "launch"
          ? spec.command.trim() !== ""
          : /^\d+$/.test(spec.pid)
        : true

  const hint = stepValid
    ? step === 2 && spec.target.provision
      ? "mperf will be uploaded to the target first"
      : ""
    : step === 0
      ? "enter user@host"
      : spec.mode === "attach"
        ? "select a process to attach to"
        : "enter a command to launch"

  const cmdPreview = `mperf record -s ${spec.scenario.toLowerCase()} -o ${spec.output} ${
    spec.mode === "launch" ? `-- ${spec.command}` : `--pid ${spec.pid}`
  }`
  const targetLabel = spec.target.id === "oneoff" ? oneOffHost : spec.target.label

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[640px] max-w-[calc(100vw-2rem)] gap-0 p-0 sm:max-w-[640px]">
        <DialogHeader className="flex-row items-center gap-6 space-y-0 border-b px-4 py-3 pr-12">
          <DialogTitle className="text-[13px] whitespace-nowrap">New profile</DialogTitle>
          <WizardSteps step={step} />
        </DialogHeader>

        <div className="h-[340px] overflow-y-auto px-4 py-3.5">
          {step === 0 && (
            <div className="grid grid-cols-2 gap-1.5">
              {TARGETS.map((t: Target) => (
                <SelectCard
                  key={t.id}
                  selected={spec.target.id === t.id}
                  onSelect={() => set("target", t)}
                  title={t.label}
                  detail={t.detail}
                  mono={t.kind === "ssh"}
                  icon={
                    t.kind === "ssh" ? (
                      <Server className="size-3 text-muted-foreground" />
                    ) : (
                      <Terminal className="size-3 text-muted-foreground" />
                    )
                  }
                />
              ))}
              <button
                onClick={() => set("target", { id: "oneoff", label: oneOffHost, kind: "ssh", detail: "one-off ssh host" })}
                className={`flex flex-col justify-center gap-1 rounded-lg border border-dashed p-2.5 text-left ${
                  spec.target.id === "oneoff" ? "border-[var(--series-1)] bg-[var(--series-1)]/5" : "hover:bg-muted/50"
                }`}
              >
                {spec.target.id === "oneoff" ? (
                  <Input
                    autoFocus
                    className="h-6 border-none bg-transparent p-0 font-mono text-[11.5px] shadow-none focus-visible:ring-0"
                    placeholder="user@host"
                    value={oneOffHost}
                    onChange={(e) => setOneOffHost(e.target.value)}
                    onClick={(e) => e.stopPropagation()}
                  />
                ) : (
                  <span className="flex items-center gap-1.5 text-[11.5px] font-medium text-muted-foreground">
                    <Plus className="size-3" /> Other host…
                  </span>
                )}
                <span className="text-[10px] text-muted-foreground">key-based ssh auth only</span>
              </button>
            </div>
          )}

          {step === 1 && (
            <div className="flex flex-col gap-2.5">
              <ToggleGroup
                type="single"
                value={spec.mode}
                onValueChange={(v) => v && set("mode", v as RunSpec["mode"])}
                className="h-7 w-fit"
              >
                <ToggleGroupItem value="launch" className="h-7 px-2.5 text-[11px]">
                  Launch command
                </ToggleGroupItem>
                <ToggleGroupItem value="attach" className="h-7 px-2.5 text-[11px]">
                  Attach to PID
                </ToggleGroupItem>
              </ToggleGroup>
              {spec.mode === "launch" ? (
                <div className="grid grid-cols-2 gap-2">
                  <div className="col-span-2">
                    <Input
                      className={inputCls}
                      value={spec.command}
                      onChange={(e) => set("command", e.target.value)}
                      placeholder="command to profile"
                    />
                  </div>
                  <Input className={inputCls} value={spec.cwd} onChange={(e) => set("cwd", e.target.value)} placeholder="working dir" />
                  <Input
                    className={inputCls}
                    value={spec.env.join(" ")}
                    onChange={(e) => set("env", e.target.value.split(" "))}
                    placeholder="KEY=value KEY2=value"
                  />
                </div>
              ) : (
                <>
                  <Input
                    className="h-7 text-[11px]"
                    placeholder={`filter processes on ${targetLabel || "the target"} — by name or pid`}
                    value={procFilter}
                    onChange={(e) => setProcFilter(e.target.value)}
                  />
                  <div className="max-h-[200px] divide-y overflow-y-auto rounded-lg border">
                    {procs.map((p) => {
                      const selected = spec.pid === String(p.pid)
                      return (
                        <button
                          key={p.pid}
                          onClick={() => set("pid", String(p.pid))}
                          className={`grid w-full grid-cols-[1fr_64px_88px_120px] items-center gap-2 px-2.5 py-1.5 text-left text-[11px] ${
                            selected ? "bg-[var(--series-1)]/8" : "hover:bg-muted/50"
                          }`}
                        >
                          <span className={`truncate font-mono ${selected ? "font-medium text-[var(--series-1)]" : ""}`}>
                            {p.name}
                          </span>
                          <span className="tabular-nums text-muted-foreground">{p.pid}</span>
                          <span className="truncate text-[10px] text-muted-foreground">{p.user}</span>
                          <span className="flex items-center gap-1.5">
                            <span className="h-1.5 w-14 overflow-hidden rounded-full bg-muted">
                              <span
                                className="block h-full rounded-full bg-[var(--series-1)]"
                                style={{ width: `${Math.min(100, (p.cpu / maxCpu) * 100)}%` }}
                              />
                            </span>
                            <span className="tabular-nums text-[10px] text-muted-foreground">
                              {p.cpu.toFixed(1)}%
                            </span>
                          </span>
                        </button>
                      )
                    })}
                    {procs.length === 0 && (
                      <div className="px-2.5 py-3 text-center text-[10.5px] text-muted-foreground">
                        no process matches "{procFilter}"
                      </div>
                    )}
                  </div>
                </>
              )}
              <span className="text-[10px] text-muted-foreground">
                {spec.mode === "launch"
                  ? `runs on ${targetLabel || "the selected target"}`
                  : `sorted by CPU · attaches to a process already running on ${targetLabel || "the selected target"}`}
              </span>
            </div>
          )}

          {step === 2 && (
            <div className="flex flex-col gap-2.5">
              <div className="grid grid-cols-2 gap-1.5">
                {SCENARIOS.map((s) => (
                  <SelectCard
                    key={s.id}
                    selected={spec.scenario === s.id}
                    onSelect={() => set("scenario", s.id as Scenario)}
                    title={s.id}
                    detail={s.blurb}
                  />
                ))}
              </div>
              <div className="flex items-center gap-2">
                <Input
                  className={`${inputCls} w-36`}
                  placeholder="duration, e.g. 30s"
                  value={spec.duration}
                  onChange={(e) => set("duration", e.target.value)}
                />
                <span className="text-[10px] text-muted-foreground">empty = until the workload exits</span>
              </div>
              <div className="flex flex-col gap-1.5 rounded-lg border bg-muted/30 p-2.5">
                <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">Will run</span>
                <code className="rounded bg-background p-2 font-mono text-[10px] leading-relaxed break-all">{cmdPreview}</code>
                <span className="text-[10px] text-muted-foreground">
                  on <span className="font-medium text-foreground">{targetLabel}</span>
                  {spec.target.provision && " · mperf uploaded first"}
                  {spec.target.kind === "ssh" && " · results pulled here when done"}
                </span>
              </div>
            </div>
          )}
        </div>

        <div className="flex items-center gap-2 border-t px-4 py-2.5">
          <span className="mr-auto text-[10.5px] text-muted-foreground">{hint}</span>
          {step > 0 && (
            <Button variant="ghost" size="sm" className="gap-1" onClick={() => setStep(step - 1)}>
              <ArrowLeft className="size-3" /> Back
            </Button>
          )}
          {step < STEPS.length - 1 ? (
            <Button size="sm" className="gap-1" disabled={!stepValid} onClick={() => setStep(step + 1)}>
              Next <ArrowRight className="size-3" />
            </Button>
          ) : (
            <Button size="sm" className="gap-1" onClick={() => onStart(finalSpec())}>
              <Play className="size-3" /> Start recording
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
