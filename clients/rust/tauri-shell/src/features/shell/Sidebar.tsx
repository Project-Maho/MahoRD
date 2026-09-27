import { Monitor, Settings } from "lucide-react"
import { BrandMark } from "@/components/brand/BrandMark"
import { cn } from "@/lib/utils"

export type AppView = "computers" | "settings"

export interface Props {
  active: AppView
  onSelect: (view: AppView) => void
}

export type SidebarProps = Props

const NAV_ITEMS = [
  { id: "computers", label: "Computers", icon: Monitor },
  { id: "settings", label: "Settings", icon: Settings },
] as const

export function Sidebar({ active, onSelect }: Props) {
  return (
    <aside className="flex h-full w-[232px] shrink-0 flex-col border-r border-sidebar-border bg-sidebar max-[1040px]:w-[68px]">
      <header className="flex h-14 items-center gap-2.5 px-4 max-[1040px]:justify-center max-[1040px]:px-0">
        <BrandMark />
        <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground max-[1040px]:sr-only">
          MahoRD
        </span>
      </header>

      <nav
        aria-label="Primary"
        className="flex flex-1 flex-col gap-0.5 px-3 pt-3 max-[1040px]:items-center max-[1040px]:px-0"
      >
        {NAV_ITEMS.map(({ id, label, icon: Icon }) => {
          const isActive = active === id
          return (
            <button
              key={id}
              id={`nav-${id}`}
              type="button"
              title={label}
              aria-current={isActive ? "page" : "false"}
              onClick={() => onSelect(id)}
              className={cn(
                "group flex h-9 w-full items-center gap-2.5 rounded-lg px-2.5 text-[13px] font-medium transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring max-[1040px]:size-10 max-[1040px]:justify-center max-[1040px]:px-0",
                isActive
                  ? "bg-white/[0.06] text-foreground shadow-[inset_0_1px_0_rgba(255,255,255,0.05),0_0_0_1px_rgba(255,255,255,0.05)]"
                  : "text-muted-foreground hover:bg-white/[0.04] hover:text-foreground"
              )}
            >
              <Icon
                className={cn(
                  "size-4 shrink-0",
                  isActive
                    ? "text-brand-soft"
                    : "text-subtle-foreground group-hover:text-muted-foreground"
                )}
                aria-hidden="true"
              />
              <span className="max-[1040px]:sr-only">{label}</span>
            </button>
          )
        })}
      </nav>
    </aside>
  )
}

export default Sidebar
