import { useEffect, useState } from "react"
import type { Scenario } from "@/lib/model"

export interface Target {
  id: string
  label: string
  kind: "local" | "ssh"
  detail: string
  provision?: boolean
}

export const TARGETS: Target[] = [
  { id: "local", label: "This machine", kind: "local", detail: "macOS · arm64 · mperf 0.5.0" },
  { id: "zen4", label: "alex@zen4-lab", kind: "ssh", detail: "Linux · x86_64 · mperf 0.5.0" },
  {
    id: "gracehopper",
    label: "alex@gracehopper",
    kind: "ssh",
    detail: "Linux · aarch64 · mperf missing — will upload",
    provision: true,
  },
]

export const SCENARIOS: { id: Scenario; blurb: string }[] = [
  { id: "Snapshot", blurb: "USE snapshot of the whole system" },
  { id: "Top-Down", blurb: "sampling + TMA counters" },
  { id: "Memory", blurb: "sampling + memory instrumentation" },
  { id: "Roofline", blurb: "cache-aware roofline model" },
]

export interface Proc {
  pid: number
  name: string
  user: string
  cpu: number
}

const LOCAL_PROCS: Proc[] = [
  { pid: 41250, name: "physim", user: "alex", cpu: 187.4 },
  { pid: 812, name: "WindowServer", user: "_windowserver", cpu: 24.1 },
  { pid: 38119, name: "node", user: "alex", cpu: 12.8 },
  { pid: 40031, name: "Google Chrome Helper (Renderer)", user: "alex", cpu: 9.2 },
  { pid: 512, name: "mds_stores", user: "root", cpu: 3.5 },
  { pid: 39544, name: "zsh", user: "alex", cpu: 0.4 },
  { pid: 1, name: "launchd", user: "root", cpu: 0.1 },
]

const REMOTE_PROCS: Proc[] = [
  { pid: 21744, name: "physim", user: "alex", cpu: 771.0 },
  { pid: 1892, name: "postgres", user: "postgres", cpu: 41.7 },
  { pid: 30201, name: "python3", user: "alex", cpu: 18.9 },
  { pid: 990, name: "containerd", user: "root", cpu: 4.2 },
  { pid: 24118, name: "sshd", user: "root", cpu: 0.8 },
  { pid: 611, name: "systemd-journald", user: "root", cpu: 0.3 },
  { pid: 1, name: "systemd", user: "root", cpu: 0.0 },
]

export function processesFor(target: Target): Proc[] {
  const procs = target.kind === "local" ? LOCAL_PROCS : REMOTE_PROCS
  return [...procs].sort((a, b) => b.cpu - a.cpu)
}

export interface RunSpec {
  target: Target
  scenario: Scenario
  mode: "launch" | "attach"
  command: string
  cwd: string
  env: string[]
  pid: string
  duration: string
  output: string
}

export type Stage =
  | "connecting"
  | "provisioning"
  | "recording"
  | "postprocessing"
  | "pulling"
  | "done"
  | "failed"

export const STAGE_LABEL: Record<Stage, string> = {
  connecting: "connecting",
  provisioning: "uploading mperf",
  recording: "recording",
  postprocessing: "postprocessing",
  pulling: "pulling results",
  done: "run finished",
  failed: "run failed",
}

export interface RunEvent {
  at: number
  stage: Stage
  line?: string
}

export interface Run {
  spec: RunSpec
  startedAt: number
  events: RunEvent[]
  cursor: number
  stage: Stage
  log: string[]
  stopped: boolean
}

export function stageList(spec: RunSpec): { id: Stage; label: string }[] {
  const remote = spec.target.kind === "ssh"
  return [
    ...(remote ? [{ id: "connecting" as Stage, label: "connect" }] : []),
    ...(spec.target.provision ? [{ id: "provisioning" as Stage, label: "upload mperf" }] : []),
    { id: "recording", label: "record" },
    { id: "postprocessing", label: "postprocess" },
    ...(remote ? [{ id: "pulling" as Stage, label: "pull results" }] : []),
  ]
}

function script(spec: RunSpec): RunEvent[] {
  const ev: RunEvent[] = []
  let t = 0
  const push = (dt: number, stage: Stage, line?: string) => {
    t += dt
    ev.push({ at: t, stage, line })
  }
  const remote = spec.target.kind === "ssh"
  const badHost = spec.target.id === "oneoff" && !spec.target.label.includes("@")
  if (remote) {
    push(0.2, "connecting", `$ ssh -o BatchMode=yes ${spec.target.label}`)
    if (badHost || spec.target.label.includes("nokey")) {
      push(1.4, "failed", `${spec.target.label}: Permission denied (publickey).`)
      push(0.1, "failed", `set up key-based auth for ${spec.target.label} and retry`)
      return ev
    }
    push(1.1, "connecting", `remote: Linux ${spec.target.id === "gracehopper" ? "aarch64" : "x86_64"} · perf_event_paranoid=1 ok`)
    if (spec.target.provision) {
      push(0.4, "provisioning", `remote: mperf not found — uploading mperf-0.5.0-linux-aarch64 (18.4 MB)`)
      push(2.6, "provisioning", `remote: mperf 0.5.0 installed to ~/.cache/mperf/bin`)
    }
  } else {
    push(0.3, "recording", `requesting kperf access… ok`)
  }
  const cmd = spec.mode === "launch" ? `-- ${spec.command}` : `--pid ${spec.pid}`
  push(0.5, "recording", `$ mperf record -s ${spec.scenario.toLowerCase()} -o ${spec.output} ${cmd}`)
  push(1.2, "recording", `[mperf] counters: cycles, instructions, llc-misses, br-misses (multiplexed ×2)`)
  push(2.4, "recording", `[mperf] 12.3k samples · 4 threads`)
  push(2.4, "recording", `[mperf] 28.1k samples · 6 threads`)
  push(2.2, "recording", spec.duration ? `[mperf] duration ${spec.duration} elapsed, stopping` : `[mperf] target exited (status 0)`)
  push(0.6, "postprocessing", `[mperf] resolving symbols… 214 modules`)
  push(1.8, "postprocessing", `[mperf] writing perf.db (33.4k samples)`)
  if (remote) {
    push(0.5, "pulling", `$ scp -C ${spec.target.label}:${spec.output} → ~/.local/share/mperf/recordings/`)
    push(2.3, "pulling", `perf.db 100%  46 MB  19.8 MB/s`)
  }
  push(0.4, "done", `✓ recording ready: ${spec.output}`)
  return ev
}

export function defaultSpec(): RunSpec {
  return {
    target: TARGETS[0],
    scenario: "Top-Down",
    mode: "launch",
    command: "./physim --steps 500",
    cwd: "~/work/physim",
    env: ["OMP_NUM_THREADS=8"],
    pid: "",
    duration: "",
    output: "results/physim-05",
  }
}

export interface RunSim {
  run: Run | null
  running: boolean
  elapsedS: number
  start: (spec: RunSpec) => void
  stop: () => void
  dismiss: () => void
}

export function useRunSim(): RunSim {
  const [run, setRun] = useState<Run | null>(null)
  const [now, setNow] = useState(0)

  const active = run !== null && run.stage !== "done" && run.stage !== "failed"

  useEffect(() => {
    if (!active) return
    const iv = setInterval(() => {
      setNow(Date.now())
      setRun((r) => {
        if (!r) return r
        const elapsed = (Date.now() - r.startedAt) / 1000
        let { cursor, stage } = r
        const log = [...r.log]
        while (cursor < r.events.length && r.events[cursor].at <= elapsed) {
          const e = r.events[cursor]
          stage = e.stage
          if (e.line) log.push(e.line)
          cursor++
        }
        return cursor === r.cursor ? r : { ...r, cursor, stage, log }
      })
    }, 250)
    return () => clearInterval(iv)
  }, [active])

  return {
    run,
    running: active,
    elapsedS: run ? Math.max(0, (now - run.startedAt) / 1000) : 0,
    start: (spec) =>
      setRun({
        spec,
        startedAt: Date.now(),
        events: script(spec),
        cursor: 0,
        stage: spec.target.kind === "ssh" ? "connecting" : "recording",
        log: [],
        stopped: false,
      }),
    stop: () =>
      setRun((r) => {
        if (!r) return r
        const elapsed = (Date.now() - r.startedAt) / 1000
        const remaining = r.events.filter((e) => e.at > elapsed && e.stage !== "recording")
        const shifted = remaining.map((e, i) => ({ ...e, at: elapsed + 0.3 + i * 0.6 }))
        return {
          ...r,
          stopped: true,
          events: [...r.events.filter((e) => e.at <= elapsed), ...shifted],
          log: [...r.log, "^C sent SIGINT — finalizing recording"],
        }
      }),
    dismiss: () => setRun(null),
  }
}

export function useSpec() {
  const [spec, setSpec] = useState<RunSpec>(defaultSpec)
  const [oneOffHost, setOneOffHost] = useState("")
  const set = <K extends keyof RunSpec>(k: K, v: RunSpec[K]) =>
    setSpec((s) => ({ ...s, [k]: v }))
  const valid =
    (spec.mode === "launch" ? spec.command.trim() !== "" : /^\d+$/.test(spec.pid)) &&
    (spec.target.id !== "oneoff" || oneOffHost.trim() !== "")
  const finalSpec = (): RunSpec =>
    spec.target.id === "oneoff"
      ? { ...spec, target: { ...spec.target, label: oneOffHost.trim() } }
      : spec
  const validationHint = valid
    ? spec.target.provision
      ? "mperf will be uploaded to the target first"
      : ""
    : spec.mode === "attach"
      ? "enter a numeric pid"
      : spec.target.id === "oneoff" && oneOffHost.trim() === ""
        ? "enter user@host"
        : "enter a command to launch"
  return { spec, set, oneOffHost, setOneOffHost, valid, finalSpec, validationHint }
}
