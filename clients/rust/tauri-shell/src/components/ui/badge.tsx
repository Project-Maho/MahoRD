import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "cn"
import { Slot } from "radix-ui"

const badgeVariants = cva(
  "inline-flex h-6 w-fit shrink-0 items-center justify-center gap-1.5 overflow-hidden rounded-full px-2 text-xs font-medium whitespace-nowrap ring-1 ring-inset transition-[color,box-shadow] [&>svg]:pointer-events-none [&>svg]:size-3",
  {
    variants: {
      variant: {
        default: "bg-primary/12 text-brand-soft ring-primary/25",
        secondary: "bg-white/[0.05] text-muted-foreground ring-white/[0.06]",
        success: "bg-success/10 text-success ring-success/20",
        warning: "bg-warning/10 text-warning ring-warning/20",
        destructive: "bg-destructive/10 text-destructive ring-destructive/25",
        outline: "bg-transparent text-foreground ring-white/10",
        ghost: "ring-transparent [a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        link: "text-brand-soft underline-offset-4 ring-transparent [a&]:hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span"

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants }
