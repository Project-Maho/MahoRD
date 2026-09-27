import { Search, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { cn } from "@/lib/utils"

export interface Props {
  query: string;
  onQueryChange: (q: string) => void;
  onClearSearch: () => void;
  className?: string;
}

export type LibraryToolbarProps = Props;

export function LibraryToolbar({ query, onQueryChange, onClearSearch, className }: Props) {
  return (
    <div className={cn("library-toolbar relative w-full max-w-md", className)}>
      <Label htmlFor="host-search" className="sr-only">
        Search computers
      </Label>
      <Search
        className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-subtle-foreground"
        aria-hidden="true"
      />
      <Input
        id="host-search"
        type="search"
        placeholder="Search computers"
        spellCheck={false}
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        className={cn("pl-9", query && "pr-9")}
      />
      {query ? (
        <Button
          id="btn-clear-search"
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label="Clear search"
          title="Clear search"
          onClick={onClearSearch}
          className="absolute top-1/2 right-1.5 -translate-y-1/2"
        >
          <X aria-hidden="true" />
        </Button>
      ) : null}
    </div>
  )
}
