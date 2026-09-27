import { cn } from "@/lib/utils"

export function BrandMark({ className }: { className?: string }) {
  return (
    <div
      aria-hidden="true"
      className={cn(
        "grid size-7 shrink-0 place-items-center rounded-[9px] bg-[linear-gradient(180deg,#26272d,#17181c)] text-[#e7e8ec] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.10),0_1px_2px_rgba(0,0,0,0.5)]",
        className
      )}
    >
      <svg viewBox="0 0 32 32" fill="none" className="size-full">
        <g transform="translate(16 16) scale(0.9) translate(-16 -16)">
          <rect x="11.5" y="7" width="15.5" height="11" rx="2" stroke="currentColor" strokeWidth="1.5" />
          <path d="M19.25 18v3.1M16.25 21.6h6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <rect x="5.5" y="8.5" width="8" height="15.5" rx="2.1" fill="#1e1f24" stroke="currentColor" strokeWidth="1.5" />
          <path d="M8.5 10.9h2" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
          <ellipse cx="16" cy="16.5" rx="10" ry="4.4" transform="rotate(-20 16 16.5)" stroke="#ff6353" strokeWidth="1.6" />
          <path d="M10.31 14.31L9.05 17.52L6.91 14.87Z" fill="#ff6353" stroke="#ff6353" strokeWidth="0.6" strokeLinejoin="round" />
          <circle cx="24.33" cy="16.35" r="1.45" fill="#ff6353" />
        </g>
      </svg>
    </div>
  )
}

export default BrandMark
