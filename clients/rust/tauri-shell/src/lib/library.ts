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
}

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
  status: LibraryStatus;
  error: string | null;
  preferenceError: string | null;
}

export type LibraryListener<T extends Host = Host> = (snapshot: LibrarySnapshot<T>) => void;

export interface LibraryOptions<T extends Host = Host> {
  invoke: (command: string, ...args: unknown[]) => Promise<T[]> | Promise<unknown> | unknown;
  storage?: StorageLike | null;
  nativeAvailable: boolean;
}

export interface Library<T extends Host = Host> {
  snapshot(): LibrarySnapshot<T>;
  subscribe(listener: LibraryListener<T>): () => boolean;
  refresh(): Promise<void>;
  setQuery(value: string): void;
  setAvailableOnly(value: boolean): void;
  setFavoritesOnly(value: boolean): void;
  clearFilters(): void;
  toggleFavorite(ip: string): void;
}

const FAVORITES_KEY = 'mahord.favorites.v1';

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
  }: SelectHostsOptions = {}
): T[] {
  const needle = query.trim().toLowerCase();
  const favorites = new Set(favoriteIps.map(normalizeHostKey));
  return hosts.filter(
    host =>
      (!availableOnly || host.online === true || host.online === 1) &&
      (!favoritesOnly || favorites.has(normalizeHostKey(host.ip))) &&
      (!needle ||
        [host.name, host.ip, host.os].some(
          value => typeof value === 'string' && value.toLowerCase().includes(needle)
        ))
  );
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

  try {
    favoriteIps = preferences.load();
  } catch (err) {
    preferenceError =
      'Favorites could not be loaded. Choices are available for this session only. ' +
      displayError(err);
  }

  function snapshot(): LibrarySnapshot<T> {
    return {
      hosts: hosts.slice(),
      visibleHosts: selectHosts(hosts, { query, availableOnly, favoritesOnly, favoriteIps }),
      query,
      availableOnly,
      favoritesOnly,
      favoriteIps: favoriteIps.slice(),
      status,
      error,
      preferenceError,
    };
  }

  function emit(): void {
    for (const listener of listeners) listener(snapshot());
  }

  async function refresh(): Promise<void> {
    if (!nativeAvailable) return;
    const current = ++requestId;
    status = 'loading';
    error = null;
    emit();
    try {
      const result = await invoke('list_hosts');
      if (current !== requestId) return;
      if (!Array.isArray(result)) throw new Error('Computer list response was not an array.');
      hosts = result.slice() as T[];
      status = 'ready';
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
  };
}

// Display data only: the page must render errors with textContent, never HTML.
function displayError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
