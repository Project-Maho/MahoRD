import { Search, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { cn } from "@/lib/utils"
import type { LibrarySort } from "@/lib/library"

export interface Props {
  query: string;
  onQueryChange: (q: string) => void;
  onClearSearch: () => void;
  className?: string;
  availableOnly?: boolean;
  favoritesOnly?: boolean;
  sort?: LibrarySort;
  onAvailableChange?: (value: boolean) => void;
  onFavoritesChange?: (value: boolean) => void;
  onSortChange?: (value: LibrarySort) => void;
}

export type LibraryToolbarProps = Props;

export function LibraryToolbar({ query, onQueryChange, onClearSearch, className, availableOnly, favoritesOnly, sort, onAvailableChange, onFavoritesChange, onSortChange }: Props) {
  return (
    <div className="flex flex-wrap items-center gap-2">
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
    {onAvailableChange && <Button variant={availableOnly ? "secondary" : "ghost"} size="sm" aria-pressed={Boolean(availableOnly)} onClick={() => onAvailableChange(!availableOnly)}>Available only</Button>}
    {onFavoritesChange && <Button variant={favoritesOnly ? "secondary" : "ghost"} size="sm" aria-pressed={Boolean(favoritesOnly)} onClick={() => onFavoritesChange(!favoritesOnly)}>Pinned only</Button>}
    {onSortChange && <div role="group" aria-label="Sort computers" className="flex flex-wrap items-center gap-1 text-sm text-muted-foreground">Sort:
      {(["name", "last-connected", "status"] as const).map(value => <Button key={value} variant={sort === value ? "secondary" : "ghost"} size="sm" aria-pressed={sort === value} onClick={() => onSortChange(value)}>{value === "last-connected" ? "Last connected" : value === "name" ? "Name" : "Status"}</Button>)}
    </div>}
    </div>
  )
}
