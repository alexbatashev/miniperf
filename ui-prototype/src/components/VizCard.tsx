import { Card, CardAction, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { cn } from "@/lib/utils"

export function VizCard({
  title,
  action,
  className,
  contentClassName,
  children,
}: {
  title?: React.ReactNode
  action?: React.ReactNode
  className?: string
  contentClassName?: string
  children: React.ReactNode
}) {
  return (
    <Card size="sm" className={cn("min-w-0 gap-1.5 rounded-lg [--card-spacing:--spacing(2.5)]", className)}>
      {title !== undefined && (
        <CardHeader className="gap-0">
          <CardTitle className="text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
            {title}
          </CardTitle>
          {action && <CardAction className="self-center text-[10px] text-muted-foreground">{action}</CardAction>}
        </CardHeader>
      )}
      <CardContent className={cn("min-w-0", contentClassName)}>{children}</CardContent>
    </Card>
  )
}
