import { Progress } from "@/components/ui/progress"
import { cn } from "@/lib/utils"

export function Meter({
  value,
  color = "var(--series-1)",
  className,
}: {
  value: number
  color?: string
  className?: string
}) {
  return (
    <Progress
      value={Math.min(100, Math.max(0, value * 100))}
      className={cn("h-[6px] [&_[data-slot=progress-indicator]]:bg-(--meter)", className)}
      style={
        {
          "--meter": color,
          background: `color-mix(in oklab, ${color} 16%, var(--viz-surface))`,
        } as React.CSSProperties
      }
    />
  )
}
