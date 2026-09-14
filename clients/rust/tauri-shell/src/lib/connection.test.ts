import { test, expect } from "bun:test";
import { validateConnection, createConnection } from "./connection";

type Deferred<T = unknown> = {
  promise: Promise<T>;
  resolve: (value: T | PromiseLike<T>) => void;
  reject: (reason?: unknown) => void;
};

interface NativeCall {
  command: string;
  args?: unknown;
  promise: Promise<unknown>;
  resolve: (value: unknown) => void;
  reject: (reason?: unknown) => void;
}

const deferred = <T = unknown>(): Deferred<T> => {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
};

// Each native operation has an explicit arrival signal, armed before the action.
function fixture({
  releaseInputs = async () => {},
  nativeAvailable = true,
}: {
  releaseInputs?: () => Promise<void> | void;
  nativeAvailable?: boolean;
} = {}) {
  const calls: NativeCall[] = [];
  const arrivals = new Map<string, Deferred<NativeCall>[]>();
  function next(command: string) {
    const signal = deferred<NativeCall>();
    const queue = arrivals.get(command) || [];
    queue.push(signal);
    arrivals.set(command, queue);
    return signal.promise;
  }
  const connection = createConnection({
    nativeAvailable,
    releaseInputs,
    invoke(command: string, args?: unknown) {
      const result = deferred<unknown>();
      const call: NativeCall = { command, args, ...result };
      calls.push(call);
      arrivals.get(command)?.shift()?.resolve(call);
      return result.promise;
    },
  });
  return { connection, calls, next };
}

async function active(f: ReturnType<typeof fixture>) {
  const arrival = f.next("connect");
  const done = f.connection.connect({ host: " example.test ", name: "Example", pin: "00123456" });
  (await arrival).resolve(undefined);
  expect(await done).toBe(true);
  return f.connection.token();
}

test("PIN and address validation matches backend without losing leading zeros", () => {
  expect(validateConnection({ host: "  ", pin: "12" })).toEqual({
    ok: false,
    errors: { host: "required", pin: "invalid-pin" },
  });
  for (const pin of ["1", "123456", "123456789", "１２３４５６７８", "١٢٣٤٥٦٧٨", "1234 678", "abcdefgh"]) {
    expect(validateConnection({ host: "host", pin })).toEqual({ ok: false, errors: { pin: "invalid-pin" } });
  }
  for (const host of ["::1", "odd hostname", "example.test"]) {
    expect(validateConnection({ host: ` ${host} `, pin: " 00123456 " })).toEqual({
      ok: true,
      args: { host, tcpPort: 19730, udpPort: 19731, pin: "00123456" },
    });
    expect(validateConnection({ host, pin: "00123456", tcpPort: 19740, udpPort: 19741 })).toEqual({
      ok: true,
      args: { host, tcpPort: 19740, udpPort: 19741, pin: "00123456" },
    });
    const spaceRes = validateConnection({ host, pin: " " });
    expect(spaceRes.ok ? spaceRes.args.pin : undefined).toBeNull();
    const nullRes = validateConnection({ host, pin: null });
    expect(nullRes.ok ? nullRes.args.pin : undefined).toBeNull();
  }
  for (const badTcp of [0, -1, 65536, 19730.5, "abc", "19730.5"]) {
    expect(
      validateConnection({ host: "example.test", tcpPort: badTcp as unknown as number })
    ).toEqual({ ok: false, errors: { tcpPort: "invalid-port" } });
  }
  for (const badUdp of [0, -1, 65536, 19731.5, "abc", "19731.5"]) {
    expect(
      validateConnection({ host: "example.test", udpPort: badUdp as unknown as number })
    ).toEqual({ ok: false, errors: { udpPort: "invalid-port" } });
  }
});

test("connect passes custom ports through invoke", async () => {
  const f = fixture();
  const arrival = f.next("connect");
  const done = f.connection.connect({
    host: "example.test",
    name: "Example",
    pin: "00123456",
    tcpPort: 19740,
    udpPort: 19741,
  });
  const call = await arrival;
  expect((call.args as Record<string, unknown>).tcpPort).toBe(19740);
  expect((call.args as Record<string, unknown>).udpPort).toBe(19741);
  call.resolve(undefined);
  expect(await done).toBe(true);
});

test("invalid and unavailable connections never invoke; subscriptions and snapshots are isolated", async () => {
  const f = fixture();
  let changes = 0;
  const off = f.connection.subscribe(() => {
    changes++;
  });
  expect(changes).toBe(0);
  expect(await f.connection.connect({ host: "", pin: "bad" })).toBe(false);
  expect(changes).toBe(1);
  f.connection.snapshot().fieldErrors.host = "changed";
  expect(f.connection.snapshot().fieldErrors.host).toBe("required");
  off();
  await f.connection.connect({ host: "" });
  expect(changes).toBe(1);
  expect(f.calls.length).toBe(0);
  const absent = fixture({ nativeAvailable: false });
  expect(await absent.connection.connect({ host: "host" })).toBe(false);
  await absent.connection.disconnect();
  await absent.connection.refreshStats();
  expect(absent.connection.snapshot().phase).toBe("unavailable");
  expect(absent.calls.length).toBe(0);
});

test("exact IPC, waiting-video then rendered frame; no PIN in state and busy blocks connect", async () => {
  const f = fixture();
  const token = await active(f);
  expect(f.calls[0].args).toEqual({ host: "example.test", tcpPort: 19730, udpPort: 19731, pin: "00123456" });
  expect(f.connection.snapshot().phase).toBe("waiting-video");
  expect(JSON.stringify(f.connection.snapshot()).includes("00123456")).toBe(false);
  const copy = f.connection.snapshot();
  copy.host!.name = "mutated";
  expect(f.connection.snapshot().host!.name).toBe("Example");
  expect(await f.connection.connect({ host: "other" })).toBe(false);
  expect(f.connection.isCurrent(token)).toBe(true);
  await f.connection.markFrameRendered(token - 1);
  expect(f.connection.snapshot().phase).toBe("waiting-video");
  await f.connection.markFrameRendered(token);
  expect(f.connection.snapshot().phase).toBe("streaming");
});

for (const outcome of ["resolve", "reject"] as const) {
  test(`cancel pending connect (${outcome}) invalidates immediately and serializes one final cleanup`, async () => {
    const release = deferred<void>(),
      released = deferred<void>();
    const f = fixture({
      releaseInputs: () => {
        released.resolve();
        return release.promise;
      },
    });
    const arrival = f.next("connect");
    const connecting = f.connection.connect({ host: "A" });
    const native = await arrival;
    const before = f.connection.token();
    const teardown = f.next("disconnect");
    const canceled = f.connection.cancel();
    expect(f.connection.token()).not.toBe(before);
    expect(f.connection.snapshot().phase).toBe("disconnecting");
    expect(f.connection.disconnect()).toBe(canceled);
    await released.promise;
    expect(await f.connection.connect({ host: "B" })).toBe(false);
    expect(f.calls.map((c) => c.command)).toEqual(["connect"]);
    release.resolve();
    native[outcome](outcome === "reject" ? new Error("late rejection") : undefined);
    const final = await teardown;
    expect(f.connection.isCurrent(before)).toBe(false);
    expect(f.connection.snapshot().phase).toBe("disconnecting");
    expect(await f.connection.connect({ host: "B" })).toBe(false);
    final.resolve(undefined);
    await canceled;
    expect(await connecting).toBe(true);
    expect(f.connection.snapshot().phase).toBe("idle");
    expect(f.connection.snapshot().busy).toBe(false);
    expect(f.connection.snapshot().error).toBe(null);
    expect(f.calls.map((c) => c.command)).toEqual(["connect", "disconnect"]);
    const fresh = await active(f);
    expect(fresh).toBeGreaterThan(before);
  });
}

test("pending connect settles before input release: teardown still waits for release", async () => {
  const release = deferred<void>(),
    entered = deferred<void>();
  const f = fixture({
    releaseInputs: () => {
      entered.resolve();
      return release.promise;
    },
  });
  const arrival = f.next("connect");
  const connecting = f.connection.connect({ host: "host" });
  const native = await arrival;
  const canceled = f.connection.cancel();
  await entered.promise;
  native.resolve(undefined);
  await connecting;
  expect(f.calls.map((c) => c.command)).toEqual(["connect"]);
  const teardown = f.next("disconnect");
  release.resolve();
  (await teardown).resolve(undefined);
  await canceled;
});

test("synchronous connecting subscription can cancel without losing native cleanup ownership", async () => {
  const f = fixture();
  const arrival = f.next("connect"),
    teardown = f.next("disconnect");
  let canceled: Promise<void> | undefined, duplicate: Promise<void> | undefined;
  const off = f.connection.subscribe((state) => {
    if (state.phase === "connecting") canceled = f.connection.cancel();
    if (state.phase === "disconnecting") duplicate = f.connection.disconnect();
  });
  const connecting = f.connection.connect({ host: "host" });
  expect(f.connection.snapshot().phase).toBe("disconnecting");
  expect(canceled).toBe(duplicate);
  expect(await f.connection.connect({ host: "blocked" })).toBe(false);
  (await arrival).resolve(undefined);
  (await teardown).resolve(undefined);
  await canceled;
  await connecting;
  off();
  expect(f.connection.snapshot().phase).toBe("idle");
  expect(f.calls.map((c) => c.command)).toEqual(["connect", "disconnect"]);
});

test("completed input release cannot disconnect an unpublished pending connect", async () => {
  const release = deferred<void>(),
    entered = deferred<void>();
  const f = fixture({
    releaseInputs: () => {
      entered.resolve();
      return release.promise;
    },
  });
  const arrival = f.next("connect");
  const connecting = f.connection.connect({ host: "host" });
  const native = await arrival;
  const teardown = f.next("disconnect");
  const canceled = f.connection.cancel();
  await entered.promise;
  release.resolve();
  // Await the exact release settlement, after cleanup's already-registered await.
  await release.promise;
  expect(f.calls.map((c) => c.command)).toEqual(["connect"]);
  expect(f.connection.snapshot().busy).toBe(true);
  native.resolve(undefined);
  (await teardown).resolve(undefined);
  await canceled;
  await connecting;
  expect(f.connection.snapshot().busy).toBe(false);
});

test("connect rejection cleans up, retaining original error and retryable cleanup failure", async () => {
  const f = fixture();
  const arrival = f.next("connect"),
    teardown = f.next("disconnect");
  const connecting = f.connection.connect({ host: "host" });
  (await arrival).reject(new Error("<unsafe> pairing denied"));
  (await teardown).reject(new Error("cleanup failed"));
  expect(await connecting).toBe(true);
  let state = f.connection.snapshot();
  expect(state.error).toBe("<unsafe> pairing denied");
  expect(state.cleanupError).toBe("cleanup failed");
  expect(state.phase).toBe("error");
  // A failed teardown surfaces cleanupError but must not latch the client:
  // busy/host are released on both branches so reconnect stays possible.
  expect(state.busy).toBe(false);
  expect(state.host).toBe(null);
  const retried = f.next("disconnect");
  const retry = f.connection.retryCleanup();
  expect(f.connection.disconnect()).toBe(retry);
  (await retried).resolve(undefined);
  await retry;
  state = f.connection.snapshot();
  expect(state.busy).toBe(false);
  expect(state.host).toBe(null);
  expect(state.cleanupError).toBe(null);
  expect(state.error).toBe("<unsafe> pairing denied");
  expect(f.calls.map((c) => c.command)).toEqual(["connect", "disconnect", "disconnect"]);
});

test("a failed cleanup never wedges the client: reconnect is accepted without a retry", async () => {
  const f = fixture();
  const arrival = f.next("connect"),
    teardown = f.next("disconnect");
  const connecting = f.connection.connect({ host: "host" });
  (await arrival).reject(new Error("pairing denied"));
  (await teardown).reject(new Error("cleanup failed"));
  expect(await connecting).toBe(true);
  expect(f.connection.snapshot().cleanupError).toBe("cleanup failed");
  expect(f.connection.snapshot().busy).toBe(false);
  const fresh = await active(f);
  expect(f.connection.snapshot().phase).toBe("waiting-video");
  expect(f.connection.snapshot().cleanupError).toBe(null);
  expect(f.connection.isCurrent(fresh)).toBe(true);
  expect(f.calls.map((c) => c.command)).toEqual(["connect", "disconnect", "connect"]);
});

test("release failure is visible, does not prevent teardown, and is retryable without blocking reconnect", async () => {
  let releases = 0;
  const f = fixture({
    releaseInputs: async () => {
      releases++;
      throw new Error("release failed");
    },
  });
  await active(f);
  const arrival = f.next("disconnect");
  const done = f.connection.disconnect();
  (await arrival).resolve(undefined);
  await done;
  expect(f.connection.snapshot().cleanupError).toBe("release failed");
  expect(f.connection.snapshot().busy).toBe(false);
  const retryArrival = f.next("disconnect");
  const retry = f.connection.retryCleanup();
  (await retryArrival).resolve(undefined);
  await retry;
  expect(releases).toBe(1);
  expect(f.connection.snapshot().busy).toBe(false);
  expect(f.connection.snapshot().cleanupError).toBe(null);
});

test("stats deduplicate, clear unavailable values, and failure does not disconnect", async () => {
  const f = fixture();
  await active(f);
  const arrival = f.next("stats");
  const pending = f.connection.refreshStats();
  expect(f.connection.refreshStats()).toBe(pending);
  (await arrival).resolve({ connected: true, latency_p50_ms: 4, latency_p99_ms: 8 });
  await pending;
  f.connection.snapshot().stats!.latency_p50_ms = 99;
  expect(f.connection.snapshot().stats!.latency_p50_ms).toBe(4);
  const next = f.next("stats");
  const nulls = f.connection.refreshStats();
  (await next).resolve({ connected: true, latency_p50_ms: null, latency_p99_ms: null });
  await nulls;
  expect(f.connection.snapshot().stats!.latency_p50_ms).toBe(null);
  const failed = f.next("stats");
  const rejected = f.connection.refreshStats();
  (await failed).reject(new Error("stats unavailable"));
  await rejected;
  expect(f.connection.snapshot().stats).toBe(null);
  expect(f.connection.snapshot().statsStatus).toBe("error");
  expect(f.connection.snapshot().statsError).toBe("stats unavailable");
  expect(f.connection.snapshot().phase).toBe("waiting-video");
});

for (const outcome of ["resolve", "reject"] as const) {
  test(`stale stats ${outcome} and stale frame errors cannot alter new generation`, async () => {
    const f = fixture();
    const oldToken = await active(f);
    const arrival = f.next("stats");
    const oldStats = f.connection.refreshStats();
    const oldNative = await arrival;
    const teardown = f.next("disconnect");
    const done = f.connection.disconnect();
    (await teardown).resolve(undefined);
    await done;
    const token = await active(f);
    const freshArrival = f.next("stats");
    const fresh = f.connection.refreshStats();
    const freshNative = await freshArrival;
    oldNative[outcome](outcome === "reject" ? new Error("old failure") : { connected: false });
    await oldStats;
    expect(f.connection.refreshStats()).toBe(fresh);
    await f.connection.reportFrameError(oldToken, new Error("stale draw"));
    await f.connection.markFrameRendered(oldToken);
    expect(f.connection.snapshot().phase).toBe("waiting-video");
    freshNative.resolve({ connected: true, frames_decoded: 12 });
    await fresh;
    expect(f.connection.snapshot().stats!.frames_decoded).toBe(12);
    expect(f.connection.isCurrent(token)).toBe(true);
  });
}

test("remote-ended stats and current frame failures use the cleanup path", async () => {
  for (const cause of ["stats", "frame"]) {
    const f = fixture();
    const token = await active(f);
    const teardown = f.next("disconnect");
    let done: Promise<void> | undefined;
    if (cause === "stats") {
      const arrival = f.next("stats");
      done = f.connection.refreshStats();
      (await arrival).resolve({ connected: false });
    } else {
      done = f.connection.reportFrameError(token, new Error("renderer failed"));
    }
    (await teardown).resolve(undefined);
    await done;
    expect(f.connection.snapshot().phase).toBe("error");
    expect(typeof f.connection.snapshot().error).toBe("string");
    expect(f.connection.snapshot().busy).toBe(false);
    expect(f.connection.isCurrent(token)).toBe(false);
    expect(f.connection.snapshot().stats).toBe(null);
  }
});

test("validateConnection accepts pairingId and passes through invoke", async () => {
  const parsed = validateConnection({ host: "example.test", pairingId: "explicit-pairing-id" });
  expect(parsed.ok).toBe(true);
  if (parsed.ok) {
    expect(parsed.args.pairingId).toBe("explicit-pairing-id");
  }

  const f = fixture();
  const arrival = f.next("connect");
  const done = f.connection.connect({
    host: "example.test",
    name: "Example",
    pairingId: "explicit-pairing-id",
  });
  const call = await arrival;
  expect((call.args as Record<string, unknown>).pairingId).toBe("explicit-pairing-id");
  expect((call.args as Record<string, unknown>).pin).toBe(null);
  call.resolve(undefined);
  expect(await done).toBe(true);
});
