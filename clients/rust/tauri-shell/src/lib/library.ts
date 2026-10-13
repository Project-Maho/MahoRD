export interface Host {
  id: string;
  name?: string | null;
  ip: string;
  os?: string | null;
  online?: boolean | number | null;
  [key: string]: unknown;
}

export interface SelectHostsOptions {
  query?: string;
  availableOnly?: boolean;
  favoritesOnly?: boolean;
  favoriteIps?: string[];
  favoriteIds?: string[];
  nicknames?: Record<string, string>;
  lastConnected?: Record<string, number>;
  sort?: LibrarySort;
}

export type LibrarySort = 'name' | 'last-connected' | 'status';

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface FavoritesStorage {
  load(): string[];
  save(ips: string[]): void;
}

export type LibraryStatus = 'idle' | 'unavailable' | 'loading' | 'ready' | 'error';

export interface LibrarySnapshot<T extends Host = Host> {
  hosts: T[];
  visibleHosts: T[];
  query: string;
  availableOnly: boolean;
  favoritesOnly: boolean;
  favoriteIps: string[];
  favoriteIds: string[];
  nicknames: Record<string, string>;
  lastSeen: Record<string, number>;
  lastConnected: Record<string, number>;
  sort: LibrarySort;
  status: LibraryStatus;
  error: string | null;
  preferenceError: string | null;
}

export type LibraryListener<T extends Host = Host> = (snapshot: LibrarySnapshot<T>) => void;

export interface LibraryOptions<T extends Host = Host> {
  invoke: (command: string, ...args: unknown[]) => Promise<T[]> | Promise<unknown> | unknown;
  storage?: StorageLike | null;
  nativeAvailable: boolean;
  savedHosts?: () => Promise<T[]>;
  hiddenNames?: () => Promise<string[]>;
}

export interface RefreshOptions {
  silent?: boolean;
}

export interface Library<T extends Host = Host> {
  snapshot(): LibrarySnapshot<T>;
  subscribe(listener: LibraryListener<T>): () => boolean;
  refresh(options?: RefreshOptions): Promise<void>;
  setQuery(value: string): void;
  setAvailableOnly(value: boolean): void;
  setFavoritesOnly(value: boolean): void;
  clearFilters(): void;
  toggleFavorite(ip: string): void;
  setNickname(id: string, value: string): void;
  setSort(value: LibrarySort): void;
  recordConnection(host: T, at?: number): void;
}

const FAVORITES_KEY = 'mahord.favorites.v1';
const LIBRARY_KEY = 'mahord.library.v2';

export function normalizeHostKey(ip: string): string {
  return ip.trim().toLowerCase();
}

export function selectHosts<T extends Host = Host>(
  hosts: T[],
  {
    query = '',
    availableOnly = false,
    favoritesOnly = false,
    favoriteIps = [],
    favoriteIds = [], nicknames = {}, lastConnected = {}, sort,
  }: SelectHostsOptions = {}
): T[] {
  const needle = query.trim().toLowerCase();
  const favorites = new Set(favoriteIps.map(normalizeHostKey));
  const selected = hosts.filter(
    host =>
      (!availableOnly || host.online === true || host.online === 1) &&
      (!favoritesOnly || favoriteIds.includes(host.id) || favorites.has(normalizeHostKey(host.ip))) &&
      (!needle ||
        [nicknames[host.id], host.name, host.ip, host.os].some(
          value => typeof value === 'string' && value.toLowerCase().includes(needle)
        ))
  );
  if (sort) selected.sort((a, b) => {
    const name = (nicknames[a.id] || a.name || a.ip).localeCompare(nicknames[b.id] || b.name || b.ip);
    if (sort === 'last-connected') return (lastConnected[b.id] || 0) - (lastConnected[a.id] || 0) || name;
    if (sort === 'status') return Number(b.online === true || b.online === 1) - Number(a.online === true || a.online === 1);
    return name;
  });
  return selected;
}

function normalizedIps(ips: unknown): string[] {
  if (!Array.isArray(ips) || !ips.every(ip => typeof ip === 'string')) {
    throw new Error('Saved favorites must be an array of IP strings.');
  }
  return [...new Set(ips.map(normalizeHostKey))];
}

export function createFavoritesStorage(storage?: StorageLike | null): FavoritesStorage {
  // A null/absent backend degrades to session-only in-memory favorites
  // instead of crashing on the nullable parameter.
  const backend: StorageLike =
    storage ?? {
      getItem: () => null,
      setItem: () => {},
    };
  return {
    load(): string[] {
      const value = backend.getItem(FAVORITES_KEY);
      return value === null ? [] : normalizedIps(JSON.parse(value));
    },
    save(ips: string[]): void {
      backend.setItem(FAVORITES_KEY, JSON.stringify(normalizedIps(ips)));
    },
  };
}

export function createLibrary<T extends Host = Host>({
  invoke,
  storage,
  nativeAvailable,
  savedHosts,
  hiddenNames,
}: LibraryOptions<T>): Library<T> {
  const preferences = createFavoritesStorage(storage);
  const listeners = new Set<LibraryListener<T>>();
  let hosts: T[] = [];
  let favoriteIps: string[] = [];
  let query = '';
  let availableOnly = false;
  let favoritesOnly = false;
  let status: LibraryStatus = nativeAvailable ? 'idle' : 'unavailable';
  let error: string | null = null;
  let preferenceError: string | null = null;
  let requestId = 0;
  let favoriteIds: string[] = [];
  let nicknames: Record<string, string> = {};
  let lastSeen: Record<string, number> = {};
  let lastConnected: Record<string, number> = {};
  let sort: LibrarySort = 'status';

  try {
    favoriteIps = preferences.load();
  } catch (err) {
    preferenceError =
      'Favorites could not be loaded. Choices are available for this session only. ' +
      displayError(err);
  }

  try {
    const value = storage?.getItem(LIBRARY_KEY);
    if (value) {
      const data = JSON.parse(value);
      if (!Array.isArray(data.hosts) || !data.hosts.every((host: Host) => typeof host.id === 'string' && typeof host.ip === 'string')) throw new Error('Invalid saved computer inventory.');
      hosts = data.hosts.map((host: T) => ({ ...host, online: false }));
      if (!Array.isArray(data.favoriteIds) || !data.favoriteIds.every((id: unknown) => typeof id === 'string')) throw new Error('Invalid saved favorite identities.');
      favoriteIds = [...new Set<string>(data.favoriteIds)];
      for (const key of ['nicknames', 'lastSeen', 'lastConnected']) {
        if (!data[key] || typeof data[key] !== 'object' || Array.isArray(data[key]) || !Object.values(data[key]).every(value => key === 'nicknames' ? typeof value === 'string' : typeof value === 'number' && Number.isFinite(value))) throw new Error('Invalid saved computer preferences.');
      }
      nicknames = data.nicknames;
      lastSeen = data.lastSeen;
      lastConnected = data.lastConnected;
      if (['name', 'last-connected', 'status'].includes(data.sort)) sort = data.sort;
    }
  } catch (err) {
    preferenceError = 'Computer preferences could not be loaded. ' + displayError(err);
  }

  function persist(): void {
    try {
      storage?.setItem(LIBRARY_KEY, JSON.stringify({ hosts, favoriteIds, nicknames, lastSeen, lastConnected, sort }));
      preferenceError = null;
    } catch (err) {
      preferenceError = 'Computer preferences could not be saved. ' + displayError(err);
    }
  }

  function snapshot(): LibrarySnapshot<T> {
    return {
      hosts: hosts.slice(),
      visibleHosts: selectHosts(hosts, { query, availableOnly, favoritesOnly, favoriteIps, favoriteIds, nicknames, lastConnected }),
      query,
      availableOnly,
      favoritesOnly,
      favoriteIps: favoriteIps.slice(),
      favoriteIds: favoriteIds.slice(), nicknames: { ...nicknames }, lastSeen: { ...lastSeen }, lastConnected: { ...lastConnected }, sort,
      status,
      error,
      preferenceError,
    };
  }

  function emit(): void {
    for (const listener of listeners) listener(snapshot());
  }

  async function refresh(options?: RefreshOptions): Promise<void> {
    if (!nativeAvailable) return;
    const current = ++requestId;
    const silent = Boolean(options?.silent);
    if (!silent) {
      status = 'loading';
      error = null;
      emit();
    }
    try {
      const result = await invoke('list_hosts');
      if (current !== requestId) return;
      if (!Array.isArray(result)) throw new Error('Computer list response was not an array.');
      const saved = savedHosts ? await savedHosts() : [];
      const hidden = hiddenNames ? await hiddenNames() : [];
      if (current !== requestId) return;
      const merged = new Map<string, T>(hosts.map(host => [host.id, { ...host, online: false }]));
      if (savedHosts) for (const [id, host] of merged) merged.set(id, { ...host, paired: saved.some(pairing => pairing.id === id) });
      for (const host of saved) merged.set(host.id, { ...merged.get(host.id), ...host, online: false });
      const now = Date.now();
      for (const host of [...saved, ...result as T[]]) {
        // Discovery IDs become pairing IDs on first pairing. Carry preferences
        // across that transition, without ever matching on a local nickname.
        const previous = [...merged.values()].find(item => item.id !== host.id && normalizeHostKey(item.ip) === normalizeHostKey(host.ip));
        if (previous) {
          if (nicknames[previous.id]) nicknames[host.id] ||= nicknames[previous.id];
          if (lastSeen[previous.id]) lastSeen[host.id] ||= lastSeen[previous.id];
          if (lastConnected[previous.id]) lastConnected[host.id] ||= lastConnected[previous.id];
          if (favoriteIds.includes(previous.id)) favoriteIds = [...favoriteIds.filter(id => id !== previous.id), host.id];
          merged.delete(previous.id);
        }
        merged.set(host.id, host);
        if (host.online === true || host.online === 1) lastSeen[host.id] = now;
        if (favoriteIps.includes(normalizeHostKey(host.ip)) && !favoriteIds.includes(host.id)) favoriteIds.push(host.id);
      }
      const order = [...new Set([...result.map((host: T) => host.id), ...saved.map(host => host.id), ...merged.keys()])];
      hosts = order.flatMap(id => {
        const host = merged.get(id);
        return host && !hidden.includes(host.name || '') ? [host] : [];
      });
      persist();
      status = 'ready';
      error = null;
    } catch (err) {
      if (current !== requestId) return;
      status = 'error';
      error = displayError(err);
    }
    emit();
  }

  return {
    snapshot,
    subscribe(listener: LibraryListener<T>) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    refresh,
    setQuery(value: string) {
      query = value;
      emit();
    },
    setAvailableOnly(value: boolean) {
      availableOnly = value;
      emit();
    },
    setFavoritesOnly(value: boolean) {
      favoritesOnly = value;
      emit();
    },
    clearFilters() {
      availableOnly = false;
      favoritesOnly = false;
      emit();
    },
    toggleFavorite(ip: string) {
      const host = hosts.find(host => host.id === ip);
      if (host) {
        favoriteIds = favoriteIds.includes(host.id) ? favoriteIds.filter(id => id !== host.id) : [...favoriteIds, host.id];
        favoriteIps = favoriteIps.filter(ip => normalizeHostKey(ip) !== normalizeHostKey(host.ip) && !favoriteIds.some(id => hosts.find(item => item.id === id)?.ip === ip));
        try { preferences.save(favoriteIps); }
        catch (err) { preferenceError = 'Favorites could not be saved. ' + displayError(err); }
        persist();
        emit();
        return;
      }
      const key = normalizeHostKey(ip);
      favoriteIps = favoriteIps.includes(key)
        ? favoriteIps.filter(value => value !== key)
        : [...favoriteIps, key];
      try {
        preferences.save(favoriteIps);
        preferenceError = null;
      } catch (err) {
        preferenceError =
          'Favorites could not be saved. Choices are available for this session only. ' +
          displayError(err);
      }
      emit();
    },
    setNickname(id, value) {
      nicknames = { ...nicknames, [id]: value.trim() };
      persist(); emit();
    },
    setSort(value) { sort = value; persist(); emit(); },
    recordConnection(host, at = Date.now()) {
      const previous = hosts.find(item => item.id === host.id);
      hosts = [...hosts.filter(item => item.id !== host.id), { ...previous, ...host }];
      lastConnected = { ...lastConnected, [host.id]: at };
      lastSeen = { ...lastSeen, [host.id]: at };
      persist(); emit();
    },
  };
}

// Display data only: the page must render errors with textContent, never HTML.
function displayError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
