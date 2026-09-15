import { test, expect } from 'bun:test';
import * as libraryModule from './library';
import {
  normalizeHostKey,
  selectHosts,
  createFavoritesStorage,
  createLibrary,
  type StorageLike,
} from './library';

const key = 'mahord.favorites.v1';
const hosts = [
  { id: 'unpaired-a', name: 'Studio', ip: ' FE80::AB ', os: 'Linux', online: true },
  { id: 'b', name: 'Office', ip: '10.0.0.2', os: 'Windows', online: false },
  { id: 'c', name: 'Studio Mini', ip: '10.0.0.3', os: 'macOS', online: true },
  { id: 'd', name: 'Other', ip: '10.0.0.4', os: null, online: 1 },
];

function memory(value: string | null = null) {
  const values = new Map<string, string>(value === null ? [] : [[key, value]]);
  return {
    values,
    getItem: (name: string) => values.get(name) ?? null,
    setItem: (name: string, data: string) => values.set(name, data),
  };
}

function deferred<T = any>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: any) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function library(options: Record<string, any> = {}) {
  return createLibrary({
    invoke: async () => hosts,
    storage: memory(),
    nativeAvailable: true,
    ...options,
  });
}

test('module exposes the same four contract exports', () => {
  expect(Object.keys(libraryModule).sort()).toEqual([
    'createFavoritesStorage',
    'createLibrary',
    'normalizeHostKey',
    'selectHosts',
  ]);
});

test('selection matches name, IP and OS; preserves order, identity and input', () => {
  expect(normalizeHostKey(' FE80::AB ')).toBe('fe80::ab');
  const all = selectHosts(hosts);
  expect(all).toEqual(hosts);
  expect(all).not.toBe(hosts);
  expect(all[0]).toBe(hosts[0]);
  const cases: Array<[string, typeof hosts]> = [
    [' STUDIO ', [hosts[0], hosts[2]]],
    ['fe80::ab', [hosts[0]]],
    [' WINDOWS ', [hosts[1]]],
    ['absent', []],
  ];
  for (const [query, expected] of cases) {
    expect(selectHosts(hosts, { query })).toEqual(expected);
  }
  expect(selectHosts(hosts, { availableOnly: true })).toEqual([hosts[0], hosts[2], hosts[3]]);
  // Numeric SQLite-style flags count as online; null/undefined and false do not.
  expect(
    selectHosts(hosts, { availableOnly: true, query: "other" })
  ).toEqual([hosts[3]]);
  expect(
    selectHosts(hosts, {
      query: 'studio',
      availableOnly: true,
      favoritesOnly: true,
      favoriteIps: [' FE80::AB ', '10.0.0.2', 'missing'],
    })
  ).toEqual([hosts[0]]);
  expect(selectHosts(hosts, { favoritesOnly: true })).toEqual([]);
  expect(hosts).toHaveLength(4);
});

test('storage is a strict string-array boundary with normalized unique IP keys', () => {
  const storage = memory();
  const adapter = createFavoritesStorage(storage);
  expect(adapter.load()).toEqual([]);
  adapter.save([' FE80::AB ', 'fe80::ab', '10.0.0.2']);
  expect([...storage.values]).toEqual([[key, '["fe80::ab","10.0.0.2"]']]);
  expect(adapter.load()).toEqual(['fe80::ab', '10.0.0.2']);
  for (const invalid of [
    '{',
    '{}',
    'null',
    '"ip"',
    '[1]',
    '["ip",null]',
    '[{"ip":"ip"}]',
  ]) {
    expect(() => createFavoritesStorage(memory(invalid)).load()).toThrow();
  }
  expect(() => adapter.save(['ip', 2 as any])).toThrow();
  expect(() =>
    createFavoritesStorage({
      getItem() {
        throw new Error('denied');
      },
      setItem() {},
    }).load()
  ).toThrow();
  expect(() =>
    createFavoritesStorage({
      getItem() {
        return null;
      },
      setItem() {
        throw new Error('quota');
      },
    }).save(['ip'])
  ).toThrow();
});

test('non-default favorites persist across reload and changed pairing IDs, not missing records', async () => {
  const storage = memory();
  const first = library({ storage });
  first.toggleFavorite(' FE80::AB ');
  first.toggleFavorite('10.0.0.2');
  first.toggleFavorite('fe80::ab');
  first.toggleFavorite('missing');
  expect(first.snapshot().favoriteIps).toEqual(['10.0.0.2', 'missing']);
  const paired = { ...hosts[1], id: 'new-pairing-id', paired: true };
  const second = library({ storage, invoke: async () => [paired, hosts[0]] });
  second.setFavoritesOnly(true);
  await second.refresh();
  expect(second.snapshot().visibleHosts).toEqual([paired]);
  expect(second.snapshot().favoriteIps).toEqual(['10.0.0.2', 'missing']);
  expect([...storage.values]).toEqual([[key, '["10.0.0.2","missing"]']]);
});

test('filter changes emit synchronously; All clears flags but not query; snapshots isolate arrays', async () => {
  const model = library();
  const events: any[] = [];
  const unsubscribe = model.subscribe((snapshot: any) => events.push(snapshot));
  expect(events).toEqual([]);
  expect(model.snapshot().status).toBe('idle');
  await model.refresh();
  model.setQuery('Studio');
  model.setAvailableOnly(true);
  model.setFavoritesOnly(true);
  expect(model.snapshot().visibleHosts).toEqual([]);
  model.toggleFavorite('fe80::ab');
  expect(events.at(-1).visibleHosts).toEqual([hosts[0]]);
  model.clearFilters();
  expect(events.at(-1)).toMatchObject({
    query: 'Studio',
    availableOnly: false,
    favoritesOnly: false,
    visibleHosts: [hosts[0], hosts[2]],
  });
  const snapshot = model.snapshot();
  snapshot.hosts.length = 0;
  snapshot.visibleHosts.length = 0;
  snapshot.favoriteIps.push('injected');
  expect(model.snapshot().hosts).toEqual(hosts);
  expect(model.snapshot().visibleHosts).toEqual([hosts[0], hosts[2]]);
  expect(model.snapshot().favoriteIps).toEqual(['fe80::ab']);
  const count = events.length;
  unsubscribe();
  model.setQuery('');
  expect(events).toHaveLength(count);
});

test('null or missing storage degrades to session-only favorites instead of crashing', () => {
  for (const absent of [undefined, null]) {
    const adapter = createFavoritesStorage(absent as StorageLike | null | undefined);
    expect(adapter.load()).toEqual([]);
    // The in-memory fallback keeps the session functional: saving must not
    // throw, and choices simply do not persist beyond the session.
    expect(() => adapter.save([' FE80::AB ', 'fe80::ab'])).not.toThrow();
    expect(adapter.load()).toEqual([]);
  }
});

test('read/write storage failure is visible while session choices survive refresh', async () => {
  for (const storage of [
    memory('{}'),
    {
      getItem() {
        throw new Error('denied');
      },
      setItem() {
        throw new Error('denied');
      },
    },
  ]) {
    const model = library({ storage });
    expect(typeof model.snapshot().preferenceError).toBe('string');
    model.toggleFavorite('fe80::ab');
    await model.refresh();
    expect(model.snapshot().favoriteIps).toEqual(['fe80::ab']);
  }
  const model = library({
    storage: {
      getItem: () => '["10.0.0.2"]',
      setItem() {
        throw new Error('quota exceeded');
      },
    },
  });
  expect(model.snapshot().preferenceError).toBeNull();
  model.toggleFavorite('fe80::ab');
  expect(typeof model.snapshot().preferenceError).toBe('string');
  expect(model.snapshot().favoriteIps).toEqual(['10.0.0.2', 'fe80::ab']);
  model.toggleFavorite('10.0.0.2');
  expect(model.snapshot().favoriteIps).toEqual(['fe80::ab']);
});

test('latest requested refresh wins against stale success and rejection', async () => {
  for (const staleFails of [false, true]) {
    const requests: any[] = [];
    const model = library({
      invoke: (command: string) => {
        expect(command).toBe('list_hosts');
        const request = deferred();
        requests.push(request);
        return request.promise;
      },
    });
    const older = model.refresh();
    expect(model.snapshot().status).toBe('loading');
    const newer = model.refresh();
    requests[1].resolve([hosts[2]]);
    await newer;
    if (staleFails) requests[0].reject(new Error('stale failure'));
    else requests[0].resolve([hosts[0]]);
    await older;
    expect(model.snapshot()).toMatchObject({ hosts: [hosts[2]], status: 'ready', error: null });
  }
});

test('loading/error retain prior inventory; empty success and unavailable remain distinct', async () => {
  const requests: any[] = [];
  const model = library({
    invoke: () => {
      const request = deferred();
      requests.push(request);
      return request.promise;
    },
  });
  const initial = model.refresh();
  const returned = [hosts[0]];
  requests[0].resolve(returned);
  await initial;
  returned.length = 0;
  const failing = model.refresh();
  expect(model.snapshot()).toMatchObject({ status: 'loading', hosts: [hosts[0]], error: null });
  requests[1].reject(new Error('discovery failed'));
  await failing;
  expect(model.snapshot()).toMatchObject({ status: 'error', hosts: [hosts[0]] });
  expect(typeof model.snapshot().error).toBe('string');
  const empty = model.refresh();
  requests[2].resolve([]);
  await empty;
  expect(model.snapshot()).toMatchObject({ status: 'ready', hosts: [], error: null });
  let calls = 0;
  const unavailable = library({
    nativeAvailable: false,
    invoke: () => {
      calls++;
    },
  });
  expect(unavailable.snapshot().status).toBe('unavailable');
  await unavailable.refresh();
  expect(calls).toBe(0);
  expect(unavailable.snapshot()).toMatchObject({ status: 'unavailable', hosts: [], visibleHosts: [] });
});

test('newest error cannot be erased by an older successful refresh', async () => {
  const requests: any[] = [];
  const model = library({
    invoke: () => {
      const request = deferred();
      requests.push(request);
      return request.promise;
    },
  });
  const older = model.refresh(),
    newer = model.refresh();
  requests[1].reject('latest failure');
  await newer;
  requests[0].resolve(hosts);
  await older;
  expect(model.snapshot()).toMatchObject({ status: 'error', hosts: [] });
  expect(typeof model.snapshot().error).toBe('string');
});
