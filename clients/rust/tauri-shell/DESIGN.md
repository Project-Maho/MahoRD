# MahoRD desktop design contract

Status: implementation contract for the 2026-09-26 redesign of the React shell
(`src/`, React 19 + Vite + Tailwind v4 + shadcn primitives). It supersedes the
2026-09-05 "quiet flat charcoal" contract. Presentation only: do not change
`src/lib/*`, `SessionCanvas.tsx`, `useRemoteInput.ts`, Rust commands, handler
logic, exported prop types, or tests. Documented exceptions: `ThisComputerCard`'s
`onCopyInfo` return type widens to `() => void | boolean | Promise<void | boolean>`
and `ComputersPage.handleCopyInfo` returns `true` after a successful copy
(`false` otherwise) so the button can confirm; `main.tsx` imports the two font
packages.

## 1. Direction

An operator console for reaching other computers: a near-black cool canvas,
layered surfaces that catch light on their top edge, hairline white-alpha
borders, and one warm coral accent taken from the app icon. Depth comes from
elevation (inset highlight + soft drop), not from color. Coral is punctuation:
the brand mark, the single primary CTA in view, focus rings, and the hover state
of a computer's Connect action. Nothing else is coral.

Principles:

- Content first. Computer identity (name, address, presence) is the loudest
  element on a card; chrome recedes.
- One primary action per region. Library cards use a neutral raised Connect
  that turns coral when its card is hovered or focused. Direct connection's
  submit is the page's only resting coral button.
- Sentence case everywhere. No all-caps labels or tracked micro-headings. A
  test-pinned uppercase string (`READY`) is displayed in sentence case with CSS.
- Honest state. Every label reflects real data from the existing commands
  (section 2). No invented previews, percentages, health checks or counts.
- Quiet motion. 140 ms color/border transitions, 220 ms panel entrances, a
  320 ms critically damped slide for the filter indicator (approximates the
  beui tabs spring: stiffness 245, damping 36, mass 1.2, damping ratio 1.05).
  `prefers-reduced-motion` removes all of it.

References: raycast.md (layered inset-highlight shadows, rgba-white 6-10%
borders, `#07080a`-class canvas) and the redesign skill guidance. MahoRD keeps
its own palette, name and orbit mark.

## 2. Product truth and copy

| Surface | Source | Required wording |
| --- | --- | --- |
| Availability | `list_hosts.online` (includes always-true fallback testbeds) | "Available" / "Offline" plus helper "Availability is a discovery hint, not a readiness check. Default testbeds may appear available without a probe." Never "Reachable", "Online", "Ready". |
| Pairing | `list_hosts.paired` (store match, not authentication) | "Saved pairing" / "PIN may be needed". Never expose keys. |
| This computer | `get_host_status` (`running`, `ip`, `pin`) | Title "This Computer (Host)", status `READY` / `UNAVAILABLE` (shown "Ready" / "Unavailable"). |
| Rendered FPS | client-side rendered frame count | Label "Rendered FPS", unavailable value "—". Not host capture FPS. |
| Decode | `stats.latency_p50_ms` / `latency_p99_ms` | "Decode p50" / "Decode p99", values like `14.2 ms`. Not network latency. |
| Counters | `frames_received`, `frames_decoded`, `audio_packets_received` | "Frames received", "Frames decoded", "Audio packets". |
| Connecting | phase `connecting` then `waiting-video` | "Connecting" then "Waiting for video". A connected session without a frame is never "Streaming". |
| No Tauri bridge | `nativeAvailable === false` | "Desktop connection controls are unavailable in this browser." Native actions disabled. |

Tone: short, calm, specific. "Connect once with a host PIN to save one." not
"Oops! Nothing here yet!". No exclamation marks.

## 3. Tokens

Defined in `src/styles/globals.css` as `:root` custom properties and bridged to
Tailwind through `@theme inline` (color utilities such as `bg-surface-2`,
`text-subtle-foreground`, `border-border-strong`).

| Token (`--name`) | Value | Use | Contrast notes |
| --- | --- | --- | --- |
| `background` | `#09090b` | App canvas, main view | |
| `sidebar` | `#0c0d10` | Navigation rail | |
| `card` = `surface-1` | `#121316` | Cards, panels | fg 17.1, fg-2 7.65, fg-3 4.9 |
| `surface-2` (= `muted`, `popover`) | `#18191d` | Hovered card, popovers | fg-3 4.64 |
| `surface-3` (= `secondary`) | `#1e1f24` | Raised controls, icon tiles | fg-2 6.77; fg-3 4.34, so no fg-3 text here |
| `surface-4` | `#24252b` | Hover of raised controls | fg 14.0, fg-2 6.29 |
| `well` | `#0b0c0e` | Inputs, segmented track | fg-3 5.17 |
| `foreground` (fg) | `#f5f5f7` | Primary text | |
| `muted-foreground` (fg-2) | `#a4a6ae` | Secondary text, labels | |
| `subtle-foreground` (fg-3) | `#80838c` | Tertiary text, placeholders | 4.5+ on canvas, sidebar, surface-1, well only |
| `border` | `rgb(255 255 255 / 0.07)` | Hairlines, card edges | decorative |
| `border-strong` | `rgb(255 255 255 / 0.12)` | Hover/focus-within edges | decorative |
| `input` | `#62656e` | Input boundaries | 3.19 on surface-1, 3.36 on well |
| `primary` | `#ff6353` | Primary fill, brand | ink 6.56 |
| `primary-hover` / `primary-active` | `#ff7d6b` / `#eb4d3d` | Primary states | ink 7.7 / 5.19 |
| `primary-foreground` | `#1c0906` | Text on coral | |
| `brand-soft` | `#ff9e90` | Coral text/icons on dark | 9.3 on surface-1 |
| `ring` | `#ff9e90` | Focus outline | 9.3 |
| `success` | `#5ad19a` | Available, streaming | 9.75 on surface-1 |
| `warning` | `#f5c451` | Favorite star, pending | 11.4 |
| `destructive` | `#ff8a8a` | Errors, Disconnect, Forget hover | 8.19; on 10% tint 6.8 |
| `accent` | `rgb(255 255 255 / 0.06)` | Ghost hover fill | |

Glass (session overlay): `rgb(16 17 20 / 0.88)` + blur. Over a white remote
screen it resolves to about `#2d2e30`, where fg is 12.5 and fg-2 5.6 but fg-3
only 3.6. **On glass, text is fg or fg-2, never fg-3.**

Type: Geist Variable (`@fontsource-variable/geist` 5.3.0, OFL) for UI and Geist
Mono Variable for addresses, PINs, IDs and numbers, falling back to the system
stacks. Body is antialiased. Scale:

| Role | Size / line | Weight | Extra |
| --- | --- | --- | --- |
| Page title (h1) | 26 / 32 px | 600 | tracking -0.02em |
| Section title (h2) | 15 / 20 px | 600 | tracking -0.01em |
| Card title (h3) | 15 / 20 px (list rows 14 px) | 600 | truncate |
| Body / controls | 13-14 / 20 px | 400-500 | |
| Meta, chips | 12 / 16 px | 500 | |
| Metric value | 15 / 20 px mono | 500 | `tabular-nums` |

Radius: controls `rounded-lg` (10 px), cards and panels `rounded-panel`
(14 px), icon tiles 9-10 px, chips `rounded-full`. `:root` holds
`--radius: 10px` and `--radius-panel: 14px`; `@theme inline` maps
`--radius-sm: 6px`, `--radius-md: 8px`, `--radius-lg: var(--radius)`,
`--radius-xl: 12px`, `--radius-2xl: 16px`, `--radius-panel: var(--radius-panel)`.

Elevation: `:root` custom properties `--elev-raised`, `--elev-panel`,
`--elev-float`, `--elev-cta`, bridged in `@theme inline` as
`--shadow-raised: var(--elev-raised)` and so on, which yields the
`shadow-raised`, `shadow-panel`, `shadow-float` and `shadow-cta` utilities.
The names avoid color tokens on purpose: `shadow-card` or `shadow-primary`
would resolve to a shadow *color* because `--color-card` and `--color-primary`
exist. The `surface-*` utilities reference `var(--elev-*)` directly.

- `--elev-raised` / `shadow-raised`: `inset 0 1px 0 rgb(255 255 255 / 0.06), 0 1px 2px rgb(0 0 0 / 0.35)`
- `--elev-panel` / `shadow-panel`: `inset 0 1px 0 rgb(255 255 255 / 0.04), 0 1px 0 rgb(0 0 0 / 0.4), 0 10px 28px -14px rgb(0 0 0 / 0.7)`
- `--elev-float` / `shadow-float`: `0 0 0 1px rgb(0 0 0 / 0.35), 0 18px 48px -10px rgb(0 0 0 / 0.75), inset 0 1px 0 rgb(255 255 255 / 0.07)`
- `--elev-cta` / `shadow-cta`: `inset 0 1px 0 rgb(255 255 255 / 0.28), inset 0 -1px 0 rgb(0 0 0 / 0.12), 0 1px 2px rgb(0 0 0 / 0.4), 0 6px 16px -6px rgb(255 99 83 / 0.55)`

Motion: `ease-spring` = `cubic-bezier(0.22, 1, 0.36, 1)`; animations
`animate-status-pulse` (2 s success ring), `animate-panel-in` (220 ms, fade +
4 px rise), `animate-fade-in` (180 ms). A global reduced-motion rule sets all
animation and transition durations to 0.01 ms.

## 4. Surface utilities and recipes

`globals.css` defines these with `@utility`. Never combine a `surface-*` class
with base `bg-*`, `border-*` or `shadow-*` classes on the same element; adjust
it only through variants (`hover:`, `focus-within:`, `group-hover:`, `data-*`).

- `surface-card`: surface-1 background, a top sheen
  (`linear-gradient(180deg, rgb(255 255 255 / 0.028), transparent 42%)`),
  1 px `--border`, `var(--elev-panel)`. Set backgrounds with the
  `background-color` and `background-image` longhands, never the `background`
  shorthand, so `hover:bg-*` and `bg-background` still compose with them.
- `surface-raised`: surface-3 background, 1 px `rgb(255 255 255 / 0.08)`,
  `shadow-raised`.
- `surface-well`: well background, 1 px `--input`,
  `inset 0 1px 2px rgb(0 0 0 / 0.35)`.
- `surface-glass`: `rgb(16 17 20 / 0.88)`, `backdrop-filter: blur(20px)
  saturate(1.4)` (with `-webkit-` prefix), 1 px `rgb(255 255 255 / 0.10)`,
  `shadow-float`.
- `canvas-glow`: `radial-gradient(1100px 420px at 50% -160px,
  rgb(255 99 83 / 0.07), transparent 70%)`, no-repeat, with
  `background-attachment: local` (named without a `bg-` prefix so class
  merging never drops `bg-background`).

Shared recipes (copy these class strings):

- Section title: `text-[15px] font-semibold leading-5 tracking-[-0.01em] text-foreground`
- Count chip: `inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-white/[0.07] px-1.5 text-xs font-medium tabular-nums text-muted-foreground`
- Icon tile (40 px): `grid size-10 shrink-0 place-items-center rounded-[10px] surface-raised` with a `size-5` icon
- Chip: `inline-flex h-6 items-center gap-1.5 rounded-full px-2 text-xs font-medium ring-1 ring-inset`
  - success: `bg-success/10 text-success ring-success/20`
  - warning: `bg-warning/10 text-warning ring-warning/20`
  - neutral: `bg-white/[0.05] text-muted-foreground ring-white/[0.06]`
- Presence dot: `size-1.5 rounded-full bg-success shadow-[0_0_0_3px_rgb(90_209_154_/_0.16)]`; offline `bg-subtle-foreground` without the halo
- Inline error: `flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/10 px-3 py-2 text-[13px] text-destructive` plus a `size-4` CircleAlert icon
- Inline notice: same geometry with warning colors and a TriangleAlert icon
- Empty state: `flex flex-col items-center justify-center gap-3 rounded-panel border border-dashed border-white/10 bg-white/[0.015] px-6 py-12 text-center`, a muted icon tile, then `max-w-sm text-sm text-muted-foreground`
- Field label: `text-[12.5px] font-medium text-muted-foreground`; optional suffix `font-normal text-subtle-foreground`
- Glass icon button: `inline-flex size-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.08] hover:text-foreground` plus the focus recipe
- Focus (all custom controls): `outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring`. `outline-solid` is required: in Tailwind v4, `outline-none` sets `--tw-outline-style: none`, which `outline-2` would otherwise inherit.

Primitive variants (`src/components/ui`):

- Button `default` (primary): `bg-primary text-primary-foreground font-semibold shadow-cta hover:bg-primary-hover active:bg-primary-active`
- Button `secondary`: `surface-raised text-foreground hover:bg-surface-4 hover:border-white/[0.12]`
- Button `outline`: `border border-white/10 bg-transparent text-foreground hover:bg-white/[0.05] hover:border-white/[0.16]`
- Button `ghost`: `text-muted-foreground hover:bg-accent hover:text-foreground`
- Button `destructive`: `bg-destructive/10 text-destructive border border-destructive/25 hover:bg-destructive/15`
- Button `link`: `text-brand-soft underline-offset-4 hover:underline`
- Button sizes: default `h-9 px-3.5 text-[13px]`, sm `h-8 px-3 text-[13px]`, xs `h-7 px-2 text-xs`, lg `h-10 px-4 text-sm`, icon `size-9`, icon-sm `size-8`, icon-xs `size-7`, icon-lg `size-10`. Base: `rounded-lg font-medium gap-2`, focus recipe, `disabled:opacity-50 disabled:shadow-none disabled:pointer-events-none`, transitions on color, background-color, border-color, box-shadow and opacity at 150 ms (never `transition-all`).
- Input: `h-10 rounded-lg surface-well px-3 text-sm text-foreground placeholder:text-subtle-foreground hover:border-[#72757e] focus-visible:border-brand-soft/70 focus-visible:ring-3 focus-visible:ring-ring/20 outline-none aria-invalid:border-destructive disabled:opacity-50`
- Badge: chip geometry; variants `default` (coral: `bg-primary/12 text-brand-soft ring-primary/25`), `secondary` (neutral), `success`, `warning`, `destructive`, `outline`.
- Card: `flex flex-col gap-4 rounded-panel surface-card p-5 text-card-foreground` (padding overridable).

## 5. Brand mark

Taken from `src-tauri/icons/icon.png`: a phone in front-left, a monitor behind
it on the right, and a tilted coral orbit with an arrowhead and a dot. In the UI
it sits on a dark tile. Implement as `src/components/brand/BrandMark.tsx`
exporting `BrandMark({ className })`. The tile is a `div`, never a `span`
(section 8).

```tsx
<div aria-hidden="true" className={cn("grid size-7 shrink-0 place-items-center rounded-[9px] bg-[linear-gradient(180deg,#26272d,#17181c)] text-[#e7e8ec] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.10),0_1px_2px_rgba(0,0,0,0.5)]", className)}>
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
```

## 6. Layout

Shell (`App.tsx`, `Sidebar.tsx`):

- Sidebar 232 px wide (`bg-sidebar`, right hairline), collapsing to a 68 px icon
  rail below 1040 px. Header 56 px: brand mark + wordmark "MahoRD"
  (15 px/600); in the rail the wordmark becomes `sr-only` and the mark centers.
  A sentence-case "Library" label (12 px fg-3, hidden in the rail) sits above
  the nav. Nav rows are 36 px, `rounded-lg`, 13 px/500, `size-4` icons
  (Monitor, Star). Active: `bg-white/[0.06]`, fg text,
  `shadow-[inset_0_1px_0_rgba(255,255,255,0.05),0_0_0_1px_rgba(255,255,255,0.05)]`,
  icon `text-brand-soft`. Inactive: fg-2 with `hover:bg-white/[0.04]
  hover:text-foreground`. In the rail, rows are 40 px squares. No footer.
- Main view: `canvas-glow bg-background`, padding `px-10 py-9`, `px-6 py-7` below 1040 px,
  `px-4 py-5` below 720 px. Content column `mx-auto w-full max-w-[1120px]`.

Computers page (`ComputersPage.tsx`) order, with `gap-8` between blocks:

1. Header: h1 "Computers" or "Favorites", subtitle (14 px fg-2), and Refresh on
   the right (secondary sm, RefreshCw icon that spins while refreshing).
2. This Computer strip (Computers view only).
3. Library block (`gap-4`): toolbar (Computers view only), then the grid section
   (heading row, helper, grid or status panel), then the preference notice.
4. Saved credentials (Computers view only).
5. Direct connection (when `showDirect`).

Grid: `grid gap-3 grid-cols-[repeat(auto-fill,minmax(min(17rem,100%),1fr))]`,
which gives 3 columns at 1280, 2 at 768 and 1 at 375. The `min()` keeps a
single column from overflowing narrow windows. Rows grow with content and
nothing scrolls horizontally.

Session (`SessionView.tsx`): video owns the window with black letterboxing.
The height-priority `#video-canvas` rule in `globals.css` is unchanged and no
session element may cause a scrollbar. A floating glass launcher is centered
12 px from the top, `w-[min(44rem,calc(100%-24px))]`, and the overlay root is a
`@container`. Details open 8 px below it (340 px). The connecting dialog is
centered over a `bg-black/65 backdrop-blur-md` scrim. The render-error toast
sits at `top-[76px]`, `z-[1050]`, so it never hides under the launcher bar
while an opened details panel may cover it.

## 7. Components

This Computer strip (`ThisComputerCard.tsx`): `surface-card rounded-panel`,
`p-4 pl-5`, one row that stacks below 720 px.

- Left: an icon tile (ScreenShare; fg when running, fg-3 when not), the
  title, and the status badge (success chip or warning chip with a presence
  dot).
- Second line: `IP` label (fg-3) with the address in mono fg, a dot separator,
  and a `PIN` label with a PIN chip (`rounded-md bg-well px-1.5 py-0.5
  font-mono tracking-[0.08em] ring-1 ring-white/[0.08]`). An 8-digit PIN renders
  as two 4-digit spans with `ml-1` and no whitespace text node between them.
- Right: secondary sm buttons, Pause or Play icon plus the toggle label, and
  Copy plus "Copy Info". When `onCopyInfo` resolves to `true`, show a Check icon
  and "Copied" for 1.6 s (clear the timer on unmount). Otherwise the label stays.

Host card (`HostGrid.tsx`): `Card` with `group relative gap-4 p-4 transition-[border-color,background-color] duration-150 hover:border-border-strong hover:bg-surface-2 focus-within:border-border-strong`.

- Top row: a Monitor icon tile (icon fg when available, fg-3 when offline),
  the h3 name and the mono 12.5 px fg-2 address (selectable), and a favorite
  icon-sm ghost button top-right: star fg-3, or `text-warning fill-current`
  when favorited.
- Chip row: an availability chip (success or neutral, with a dot) and a pairing
  chip (neutral; KeyRound icon for "Saved pairing", Hash icon for "PIN may be
  needed").
- Footer: a `border-t border-white/[0.06] pt-3` row with the OS text (12 px
  fg-3, "Unknown OS" when absent) and Connect (secondary sm, ArrowRight after the
  text). On an available card, `group-hover:` and `group-focus-within:` switch
  it to `bg-primary text-primary-foreground border-transparent shadow-cta`,
  and `hover:bg-primary-hover hover:border-transparent` replaces the secondary
  hover (otherwise the button's own hover wins over the group hover). Apply
  these classes only when the host is available and `busy` is false. The whole
  card is never clickable.
- Grid heading row: the "Listed computers" h2 with a count chip, and the helper
  (12 px fg-3 with an Info icon) wrapping beside it or below.
- Status panel: the empty-state recipe with a muted Monitor tile and the
  message only.

Toolbar (`LibraryToolbar.tsx`): one wrapping row.

- Search field: `flex-1 min-w-[240px]`, a Search icon inset left (`pl-9`), and
  an sr-only label "Search computers". While the query is non-empty, a
  clear-search ghost icon-xs (X) is inset right (`pr-9`).
- Filter: a 3-column segmented control, `surface-well h-10 p-1 rounded-[10px]
  w-[288px]` (full width below 720 px). A sliding `surface-raised rounded-lg`
  indicator is placed with an inline `transform: translateX(<index * 100>%)`
  and `transition-transform duration-300 ease-spring`. Segment labels are
  13 px/500: fg when active, fg-2 with `hover:text-foreground` otherwise.

Saved credentials (`SavedCredentials.tsx`):

- Heading row: h2, count chip, and an icon-sm ghost refresh button (RefreshCw,
  aria-label and title "Refresh saved credentials").
- Helper: 12 px fg-3.
- Rows live in one `surface-card rounded-panel overflow-hidden divide-y
  divide-white/[0.06]` list, `px-4 py-3`. Each row has a KeyRound tile (32 px,
  `text-brand-soft`), the h3 name (14 px), the endpoint (mono 12 px fg-2,
  truncate) and an `ID <id>` line (mono 11 px fg-3, truncate). Actions: Connect
  (secondary sm) and Forget
  (ghost sm, Trash2, `hover:text-destructive hover:bg-destructive/10`).
- Empty: a dashed row with a KeyRound icon, "No saved credentials yet." and
  "Connect once with a host PIN to save one." (fg-3).
- No uppercase badges.

Direct connection (`DirectConnect.tsx`):

- `surface-card rounded-panel p-5` card. Header: a Cable icon tile (fg-2
  icon), the h2 title, and a 13 px fg-2 description.
- Form grid: `grid gap-x-3 gap-y-4 grid-cols-1
  min-[560px]:grid-cols-[minmax(0,1fr)_12rem]`. DOM, tab and visual order are
  identical: host and PIN (row 1), relay and submit (row 2, submit `self-end
  w-full`), error (`col-span-full`).
- Inputs: mono. The host placeholder is "e.g. 192.168.1.20". The PIN label shows
  "(optional)" in fg-3.
- Submit: primary with ArrowRight. While busy it shows a spinning LoaderCircle
  and "Connecting".

Session launcher (`SessionView.tsx`): `launcher-bar surface-glass
pointer-events-auto min-h-12 rounded-[14px] p-1.5 pl-3 gap-1`.

- Identity: a status dot (streaming `bg-success animate-status-pulse`,
  connecting or waiting `bg-warning`, otherwise fg-3), the 13 px/600 host
  name (truncated), and the `via relay` coral chip (always visible).
- Stats group, hidden with `@max-[34rem]:hidden`: label (fg-2) plus value (mono
  fg) pairs separated by `{" "}`.
- A `h-5 w-px bg-white/10` divider.
- Home (House) and Expand (ChevronDown, rotates 180 degrees when open) glass
  icon buttons.
- Disconnect: `h-8 px-2.5 rounded-lg text-xs font-semibold text-destructive
  hover:bg-destructive/12` with a Power icon.

Session details panel: `launcher-panel surface-glass pointer-events-auto
w-[340px] rounded-[14px] p-4 gap-4 animate-panel-in`.

- Title "Session details" (13 px/600 fg).
- Stats error (inline-error recipe).
- `dl` grid, 2 columns, of metric tiles: `rounded-lg bg-white/[0.04] ring-1
  ring-inset ring-white/[0.06] px-3 py-2.5`, `dt` 11 px/500 fg-2, `dd` mono
  15 px fg. First a full-width State row (phase with CSS `capitalize` and a
  dot), then Rendered FPS (no id, so the launcher keeps the only
  `session-stat-fps`), Decode p50, Decode p99, Frames received, Frames decoded
  and Audio packets. The FPS tile keeps the metric visible when narrow
  launchers hide their stats group.
- Footnote, 11.5 px fg-2: "Rendered FPS counts frames drawn by this client.
  Decode is local receive-to-decode time, not network latency."
- A full-width raised Fullscreen button (Maximize2 or Minimize2).

Connecting dialog: `surface-glass rounded-2xl p-6 w-[min(360px,calc(100%-32px))]`,
centered column.

- A 56 px spinner: `border-2 border-white/10 border-t-primary animate-spin`
  ring around a static Monitor icon in `text-brand-soft`.
- Status text (15 px/600) and the mono host line.
- Helper: "Establishing a secure session" while connecting; "Connected. Waiting
  for the first frame." while waiting.
- Two step pills, "Connect" and "First frame". Connecting: step 1 active, step
  2 pending. Waiting: step 1 done (Check, success), step 2 active.
- A secondary Cancel button.

## 8. DOM and test contract (must not break)

Unit tests (`bun test src`) render into a hand-written fake DOM, and page tests
(`bun test tests`, served from `dist/`) query real IDs. Keep every ID and
pinned string below exactly, and keep each handler on the element that
carries its ID or `data-action`.

- Sidebar: the first `span` inside `aside header` must have text exactly
  `MahoRD` (so the brand tile is a `div`). `#nav-computers` and
  `#nav-favorites` keep `aria-current` "page" or "false".
- This computer:
  - `#this-computer-card`, `#this-computer-title` = `This Computer (Host)`.
  - `#this-computer-badge` whole text = `READY` or `UNAVAILABLE`. Only an empty
    dot span may be added. Display it via an inner
    `inline-block lowercase first-letter:uppercase` span.
  - `#this-computer-ip` contains the IP.
  - `#this-computer-pin` whole text = the PIN (no spaces or labels inside).
  - `#btn-host-toggle` whole text = `Pause Sharing` or `Resume Sharing`.
  - `#btn-host-copy` initial whole text = `Copy Info`.
  - Icons are fine because SVGs add no text.
- Library:
  - `#library-heading` contains `Listed computers`; `#library-count` whole text
    = the count.
  - `#host-grid` has `role="list"` and `aria-busy`. Each card has
    `role="listitem"`, `data-host-id` and `data-availability`. It is the only
    `role="list"` and the only listitems HostGrid renders.
  - `[data-action="favorite"]` keeps `aria-label="Favorite <name>"` and
    `aria-pressed`.
  - `[data-action="connect"]` whole text = `Connect` and is disabled when
    offline or busy.
  - `#library-status` (`role="status"`) replaces the grid and contains the
    message.
- Toolbar: `#host-search` (`type="search"`); `#btn-clear-search` present while
  the query is non-empty, and clicking it clears. `#filter-all`,
  `#filter-available`, `#filter-favorites` keep `aria-pressed` "true" or
  "false" and the `role="group"` wrapper.
- Saved credentials:
  - Section and list: `#saved-pairings-section`,
    `data-testid="saved-credentials"`, `#saved-pairings-heading`,
    `#saved-pairings-count` (whole text = count), `#btn-refresh-pairings`,
    `#saved-pairings-list` (`role="list"`).
  - Rows: `[data-pairing-id]` with `role="listitem"`, the `h3` whole text = the
    name, and row text containing the endpoint string.
  - Actions: `[data-action="connect-saved"]`, `[data-action="forget-saved"]`.
  - `#saved-pairings-empty` contains `No saved credentials yet`.
- Direct: `#direct-heading`, `#direct-help`, `#direct-form` (form,
  `noValidate`, `onSubmit`), `#direct-ip`, `#direct-pin` (password),
  `#direct-relay`, `#btn-direct-connect` (`type="submit"`), and
  `#direct-error` (`role="alert"`, contains the message).
- Page and app: `#btn-refresh` (disabled without the native bridge),
  `#preference-error`, `#main-view` with `data-ui-scope`, `data-app-shell`.
- Session:
  - Overlay and bar: `#viewport-container` (unchanged), `#session-overlay`
    (`data-ui-scope`, `role="group"`, class contains `pointer-events-none`),
    `#launcher-bar` (class contains `pointer-events-auto`).
  - Header stats: `#session-host-name`, `#session-stat-fps` (contains
    `Rendered FPS`), `#session-stat-lat` (contains `Decode p50` and the ms
    value).
  - Controls: `#btn-home` with title and aria-label exactly `Home — end session
    and return to main screen`. `#btn-expand` with `aria-expanded` and
    `aria-controls="launcher-panel"`. `#launcher-panel` is rendered only while
    expanded. `#btn-disconnect` contains `Disconnect`.
  - Panel stats: `#session-stat-state` whole text = the phase;
    `#session-stat-p50`, `#session-stat-p99`, `#session-stat-frames`,
    `#session-stat-decoded`, `#session-stat-audio` whole text = the value (e.g.
    `14.2 ms`, `300`). `#session-stats-notice`.
  - Fullscreen: `#btn-fullscreen` (`aria-pressed`, contains `Fullscreen`),
    `#btn-fullscreen-label`.
  - Dialog and alerts: `#session-connecting-modal` (`role="dialog"`,
    `aria-modal`, `data-ui-scope`), `#modal-connecting-text` (`role="status"`),
    `#modal-host-name`, `#btn-cancel-connect`, `#render-error-alert`,
    `#session-via-relay`.
- Remote input: `REMOTE_TARGET_IDS` (viewport, viewport-container,
  screen-canvas, video-canvas) and `closest("#btn-home")` /
  `closest("#btn-disconnect")` in `useRemoteInput` depend on these IDs.

Fake-DOM safety for every component rendered by `App` (unit tests mount the
whole idle app):

- No `matchMedia`, `ResizeObserver`, `IntersectionObserver`,
  `getBoundingClientRect`, `getComputedStyle` or other layout measurement.
- No CSS custom properties in inline `style` (React calls
  `style.setProperty`, which the fake lacks). Plain properties such as
  `transform` are fine.
- No portals and no new dependencies.

## 9. Accessibility and motion

- Landmarks: `aside` > `nav[aria-label="Primary"]`, `main`, one h1, h2 per
  section, h3 per card or row.
- Every icon-only control has an `aria-label` and a `title`. Decorative icons
  are `aria-hidden`.
- Status is never color-only: dots always sit beside a text label.
- Focus: a 2 px `--ring` outline with 2 px offset on every interactive control.
  The remote viewport and canvas get no focus outline.
- Addresses, PINs and IDs stay selectable. Only the session container is
  `select-none`.
- Reduced motion: the global rule plus `motion-reduce:animate-none` on
  spinners. Pending states still read through text.

## 10. Acceptance

1. `bun test src` and `bunx tsc -b --noEmit` pass. After the single final
   `bun run build`, `bun test tests` passes.
2. Screenshots at 1280x800, 768x1024 and 375x812 of the library (Computers and
   Favorites), the session bar collapsed and expanded, and the connecting
   dialog show no clipped text, no overlaps and no horizontal scroll.
3. Coral appears only on the brand mark, the 7% ambient canvas glow, the
   resting primary CTA (Direct connection submit), focus rings, the active nav
   icon, hovered or focused card Connect actions, the saved-row key icons, the
   relay chip, and the connecting dialog's spinner and active step.
4. All-caps text appears nowhere on screen.
