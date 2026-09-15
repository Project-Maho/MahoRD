export type ConnectionPhase =
  | 'idle'
  | 'unavailable'
  | 'connecting'
  | 'waiting-video'
  | 'streaming'
  | 'disconnecting'
  | 'error';

export type StatsStatus = 'idle' | 'loading' | 'ready' | 'error';

export interface HostInfo {
  ip: string;
  name: string;
}

export interface ConnectionStats {
  connected?: boolean;
  latency_p50_ms?: number | null;
  latency_p99_ms?: number | null;
  frames_decoded?: number;
  [key: string]: unknown;
}

export interface ConnectionSnapshot {
  phase: ConnectionPhase;
  host: HostInfo | null;
  fieldErrors: Record<string, string>;
  error: string | null;
  cleanupError: string | null;
  stats: ConnectionStats | null;
  statsStatus: StatsStatus;
  statsError: string | null;
  generation: number;
  busy: boolean;
}

export interface ValidateConnectionOptions {
  host?: string;
  name?: string;
  pin?: string | null;
  tcpPort?: number | string | null;
  udpPort?: number | string | null;
  pairingId?: string | null;
  [key: string]: unknown;
}

export interface ValidatedConnectionArgs {
  host: string;
  tcpPort: number;
  udpPort: number;
  pin: string | null;
  pairingId?: string;
}

export type ValidationResult =
  | { ok: true; args: ValidatedConnectionArgs }
  | { ok: false; errors: Record<string, string> };

export interface ConnectionDependencies {
  invoke: (command: string, args?: unknown) => Promise<unknown>;
  nativeAvailable: boolean;
  releaseInputs: () => Promise<void> | void;
}

export interface ConnectionInstance {
  snapshot(): ConnectionSnapshot;
  subscribe(fn: (state: ConnectionSnapshot) => void): () => void;
  connect(request: ValidateConnectionOptions): Promise<boolean>;
  cancel(): Promise<void>;
  disconnect(): Promise<void>;
  retryCleanup(): Promise<void>;
  refreshStats(): Promise<void>;
  token(): number;
  isCurrent(token: number): boolean;
  markFrameRendered(token: number): Promise<void>;
  reportFrameError(token: number, error: unknown): Promise<void>;
}

export function validateConnection({
  host = '',
  pin = null,
  tcpPort = null,
  udpPort = null,
  pairingId = null,
}: ValidateConnectionOptions = {}): ValidationResult {
  host = host.trim();
  pin = pin == null ? '' : pin.trim();
  pairingId = pairingId == null ? null : typeof pairingId === 'string' ? pairingId.trim() || null : null;
  const errors: Record<string, string> = {};
  if (!host) errors.host = 'required';
  if (pin && !/^[0-9]{8}$/.test(pin)) errors.pin = 'invalid-pin';
  // The backend requires either a PIN or a stored pairing id; reject an
  // unpairable request locally instead of round-tripping a guaranteed failure.
  if (!pin && !pairingId) errors.pin = 'required';

  function checkPort(val: unknown, fieldName: string, defaultPort: number): number | null {
    if (val === null || val === undefined || val === '') return defaultPort;
    let num: number;
    if (typeof val === 'number') {
      num = val;
    } else if (typeof val === 'string' && val.trim() !== '') {
      const trimmed = val.trim();
      num = Number(trimmed);
      if (String(num) !== trimmed) {
        errors[fieldName] = 'invalid-port';
        return null;
      }
    } else {
      errors[fieldName] = 'invalid-port';
      return null;
    }
    if (!Number.isInteger(num) || num < 1 || num > 65535) {
      errors[fieldName] = 'invalid-port';
      return null;
    }
    return num;
  }

  const finalTcp = checkPort(tcpPort, 'tcpPort', 19730);
  const finalUdp = checkPort(udpPort, 'udpPort', 19731);
  const args: ValidatedConnectionArgs = { host, tcpPort: finalTcp as number, udpPort: finalUdp as number, pin: pin || null };
  if (pairingId) args.pairingId = pairingId;
  return Object.keys(errors).length
    ? { ok: false, errors }
    : { ok: true, args };
}

const errorText = (error: unknown): string =>
  error instanceof Error
    ? error.message
    : typeof error === 'object' && error !== null && 'message' in error && Boolean((error as { message?: unknown }).message)
    ? String((error as { message?: unknown }).message)
    : String(error);

export function createConnection({ invoke, nativeAvailable, releaseInputs }: ConnectionDependencies): ConnectionInstance {
  const state: ConnectionSnapshot = {
    phase: nativeAvailable ? 'idle' : 'unavailable',
    host: null,
    fieldErrors: {},
    error: null,
    cleanupError: null,
    stats: null,
    statsStatus: 'idle',
    statsError: null,
    generation: 0,
    busy: false,
  };
  const listeners = new Set<(snapshot: ConnectionSnapshot) => void>();
  let pendingConnect: Promise<{ ok: true } | { ok: false; error: string }> | null = null;
  let pendingCleanup: Promise<void> | null = null;
  let pendingStats: Promise<void> | null = null;

  function snapshot(): ConnectionSnapshot {
    return {
      ...state,
      host: state.host ? { ...state.host } : null,
      fieldErrors: { ...state.fieldErrors },
      stats: state.stats ? { ...state.stats } : null,
    };
  }

  function emit(): void {
    for (const listener of listeners) listener(snapshot());
  }

  function clearStats(): void {
    state.stats = null;
    state.statsStatus = 'idle';
    state.statsError = null;
    pendingStats = null;
  }

  function isCurrent(token: number): boolean {
    return (
      token === state.generation &&
      (state.phase === 'waiting-video' || state.phase === 'streaming')
    );
  }

  function cleanup(retry = false): Promise<void> {
    if (pendingCleanup) return pendingCleanup;
    // A retry after a failed teardown has already released `busy`, but native
    // disconnect still has to be re-run, so `cleanupError` keeps the path open.
    if (!state.busy && !(retry && state.cleanupError)) return Promise.resolve();
    const connectToSettle = pendingConnect;
    ++state.generation;
    state.phase = 'disconnecting';
    state.cleanupError = null;
    clearStats();
    // Install ownership before synchronous notification can reenter an action.
    pendingCleanup = Promise.resolve().then(async () => {
      const errors: string[] = [];
      if (!retry) {
        try {
          await releaseInputs();
        } catch (error) {
          errors.push(errorText(error));
        }
      }
      // Native connect publishes only on settlement: an earlier disconnect
      // cannot cancel it. Its outcome is consumed, never allowed to revive UI.
      if (connectToSettle) await connectToSettle;
      try {
        await invoke('disconnect');
      } catch (error) {
        errors.push(errorText(error));
      }
      // Both branches release the session: `cleanupError` is a surfaced,
      // dismissible message, never a latched state that blocks connect().
      state.busy = false;
      state.host = null;
      if (errors.length) {
        state.cleanupError = errors.join('\n');
        state.phase = 'error';
      } else {
        state.phase = state.error ? 'error' : 'idle';
      }
      pendingCleanup = null;
      emit();
    });
    emit();
    return pendingCleanup;
  }

  async function connect(request: ValidateConnectionOptions): Promise<boolean> {
    if (!nativeAvailable || state.busy) return false;
    const validation = validateConnection(request);
    if (!validation.ok) {
      state.fieldErrors = validation.errors;
      emit();
      return false;
    }
    const generation = ++state.generation;
    state.phase = 'connecting';
    state.host = { ip: validation.args.host, name: request.name || validation.args.host };
    state.fieldErrors = {};
    state.error = null;
    state.cleanupError = null;
    state.busy = true;
    clearStats();
    const operation = Promise.resolve()
      .then(() => invoke('connect', validation.args))
      .then(
        () => ({ ok: true as const }),
        (error: unknown) => ({ ok: false as const, error: errorText(error) })
      );
    pendingConnect = operation;
    emit();
    const result = await operation;
    if (pendingConnect === operation) pendingConnect = null;
    if (generation !== state.generation) return true;
    if (result.ok) {
      state.phase = 'waiting-video';
      emit();
    } else {
      state.error = result.error;
      await cleanup();
    }
    return true;
  }

  function refreshStats(): Promise<void> {
    const generation = state.generation;
    if (!isCurrent(generation)) return Promise.resolve();
    if (pendingStats) return pendingStats;
    state.statsStatus = 'loading';
    state.statsError = null;
    const operation = Promise.resolve().then(async () => {
      try {
        const stats = (await invoke('stats')) as (ConnectionStats & { connected?: boolean }) | null;
        if (!isCurrent(generation)) return;
        if (stats && stats.connected === false) {
          state.error = 'Remote session ended.';
          await cleanup();
          return;
        }
        state.stats = stats ? { ...stats } : null;
        state.statsStatus = 'ready';
      } catch (error) {
        if (!isCurrent(generation)) return;
        state.stats = null;
        state.statsStatus = 'error';
        state.statsError = errorText(error);
      } finally {
        if (pendingStats === operation) {
          pendingStats = null;
          emit();
        }
      }
    });
    pendingStats = operation;
    emit();
    return operation;
  }

  return {
    snapshot,
    subscribe(fn: (snapshot: ConnectionSnapshot) => void): () => void {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    connect,
    cancel: () => cleanup(Boolean(state.cleanupError)),
    disconnect: () => cleanup(Boolean(state.cleanupError)),
    retryCleanup: () => (state.cleanupError ? cleanup(true) : pendingCleanup || Promise.resolve()),
    refreshStats,
    token: () => state.generation,
    isCurrent,
    markFrameRendered(token: number): Promise<void> {
      if (isCurrent(token) && state.phase !== 'streaming') {
        state.phase = 'streaming';
        emit();
      }
      return Promise.resolve();
    },
    reportFrameError(token: number, error: unknown): Promise<void> {
      if (!isCurrent(token)) return Promise.resolve();
      state.error = errorText(error);
      return cleanup();
    },
  };
}
