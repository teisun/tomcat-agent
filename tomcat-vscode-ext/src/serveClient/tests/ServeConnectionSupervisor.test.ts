import { describe, expect, it, vi } from "vitest";

import type { InitializeResult } from "../initialize";
import {
  DEFAULT_HANDSHAKE_TIMEOUT_MS,
  ServeConnectionSupervisor,
  type ServeConnectionState,
} from "../ServeConnectionSupervisor";
import {
  DEFAULT_REQUEST_TIMEOUT_MS,
  type TomcatMessengerExit,
} from "../TomcatMessenger";

const READY_RESULT: InitializeResult = {
  attachmentRoot: null,
  capabilities: ["prompt", "ask_question"],
  protocolVersion: 2,
  serverVersion: "0.1.40",
  sessionId: "s1",
};

function deferred<T>() {
  let reject!: (error: Error) => void;
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function exit(
  overrides: Partial<TomcatMessengerExit> = {},
): TomcatMessengerExit {
  return {
    code: 1,
    signal: null,
    stderr: "",
    ...overrides,
  };
}

function createMessenger() {
  let running = false;
  let recentStderr = "";
  let exitListener: ((event: TomcatMessengerExit) => void) | undefined;
  const messenger = {
    dispose: vi.fn(() => {
      running = false;
    }),
    get isRunning() {
      return running;
    },
    get recentStderr() {
      return recentStderr;
    },
    onExit: vi.fn((listener: (event: TomcatMessengerExit) => void) => {
      exitListener = listener;
      return { dispose: vi.fn() };
    }),
    restart: vi.fn(() => {
      running = true;
      recentStderr = "";
    }),
    start: vi.fn(() => {
      running = true;
      recentStderr = "";
    }),
    stop: vi.fn(() => {
      running = false;
    }),
  };
  return {
    emitExit(event: TomcatMessengerExit) {
      running = false;
      recentStderr = event.stderr;
      exitListener?.(event);
    },
    messenger,
  };
}

async function retryOnce(): Promise<void> {
  await vi.advanceTimersByTimeAsync(300);
}

describe("ServeConnectionSupervisor", () => {
  it("auto-retries a transient startup failure and resolves initialize on a later attempt", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      const initialize = vi
        .fn<() => Promise<InitializeResult>>()
        .mockImplementationOnce(async () => {
          runtime.emitExit(exit({ stderr: "temporary startup failure" }));
          throw new Error("initialize timed out");
        })
        .mockResolvedValueOnce(READY_RESULT);
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });

      const ready = supervisor.whenReady();
      await retryOnce();

      await expect(ready).resolves.toEqual(READY_RESULT);
      expect(initialize).toHaveBeenCalledTimes(2);
      expect(initialize).toHaveBeenCalledWith(DEFAULT_HANDSHAKE_TIMEOUT_MS);
      expect(supervisor.currentState).toMatchObject({ phase: "ready", status: "ready" });
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps whenReady pending across transient startup retries", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      const secondResult = deferred<InitializeResult>();
      const initialize = vi
        .fn<() => Promise<InitializeResult>>()
        .mockImplementationOnce(async () => {
          runtime.emitExit(exit({ stderr: "temporary startup failure" }));
          throw new Error("temporary startup failure");
        })
        .mockReturnValueOnce(secondResult.promise);
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });
      const settled = vi.fn();
      const ready = supervisor.whenReady().then(settled);

      await retryOnce();
      await Promise.resolve();
      expect(settled).not.toHaveBeenCalled();

      secondResult.resolve(READY_RESULT);
      await ready;
      expect(settled).toHaveBeenCalledWith(READY_RESULT);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not double-connect when serve exits during startup", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      const initialize = vi.fn<() => Promise<InitializeResult>>();
      runtime.messenger.start.mockImplementationOnce(() => {
        runtime.emitExit(exit({ stderr: "early startup exit" }));
      });
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });

      void supervisor.whenReady();
      expect(runtime.messenger.start).toHaveBeenCalledTimes(1);
      expect(initialize).not.toHaveBeenCalled();

      await retryOnce();
      expect(runtime.messenger.start).toHaveBeenCalledTimes(2);
      expect(initialize).toHaveBeenCalledTimes(1);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("auto-reconnects after serve exits unexpectedly once ready", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      const initialize = vi.fn<() => Promise<InitializeResult>>().mockResolvedValue(READY_RESULT);
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });
      await supervisor.whenReady();

      runtime.emitExit(exit({ stderr: "lost child" }));
      expect(supervisor.currentState).toMatchObject({
        phase: "backoff",
        status: "reconnecting",
      });
      await retryOnce();

      expect(initialize).toHaveBeenCalledTimes(2);
      expect(supervisor.currentState).toMatchObject({ phase: "ready", status: "ready" });
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("gives up after the bounded retry budget and preserves the last error", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      let failureNumber = 0;
      const initialize = vi.fn<() => Promise<InitializeResult>>().mockImplementation(async () => {
        failureNumber += 1;
        runtime.emitExit(exit({ stderr: `transient failure ${failureNumber}` }));
        throw new Error("startup timeout");
      });
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });

      const ready = supervisor.whenReady();
      const rejected = expect(ready).rejects.toThrow("tomcat serve exited");
      for (const delay of [300, 800, 2_000, 4_000]) {
        await vi.advanceTimersByTimeAsync(delay);
      }

      await rejected;
      expect(supervisor.currentState).toMatchObject({
        attempt: 5,
        failure: { kind: "retry_exhausted", stderr: "transient failure 5" },
        phase: "fatal",
        status: "failed",
      });
      expect(runtime.messenger.stop).toHaveBeenCalledTimes(0);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops retrying after matching fast stderr failures and surfaces that stderr", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      runtime.messenger.start.mockImplementation(() => {
        runtime.emitExit(exit({ stderr: "missing API key TOMCAT_TEST_KEY" }));
      });
      const supervisor = new ServeConnectionSupervisor({
        initialize: vi.fn<() => Promise<InitializeResult>>(),
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });

      const ready = supervisor.whenReady();
      const rejected = expect(ready).rejects.toThrow("tomcat serve exited");
      await retryOnce();

      await rejected;
      expect(supervisor.currentState).toMatchObject({
        attempt: 2,
        failure: {
          kind: "deterministic_startup",
          stderr: "missing API key TOMCAT_TEST_KEY",
        },
        phase: "fatal",
      });
      expect(runtime.messenger.start).toHaveBeenCalledTimes(2);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("classifies ENOENT as executable-missing without retrying", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      runtime.messenger.start.mockImplementationOnce(() => {
        runtime.emitExit(exit({ error: new Error("spawn tomcat ENOENT") }));
      });
      const supervisor = new ServeConnectionSupervisor({
        initialize: vi.fn<() => Promise<InitializeResult>>(),
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
      });

      await expect(supervisor.whenReady()).rejects.toThrow("spawn tomcat ENOENT");
      expect(supervisor.currentState).toMatchObject({
        failure: { kind: "executable_missing" },
        phase: "fatal",
      });
      expect(runtime.messenger.start).toHaveBeenCalledTimes(1);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("allows manual reconnect after the retry budget is exhausted", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      let failureNumber = 0;
      const initialize = vi.fn<() => Promise<InitializeResult>>().mockImplementation(async () => {
        if (failureNumber < 5) {
          failureNumber += 1;
          runtime.emitExit(exit({ stderr: `transient failure ${failureNumber}` }));
          throw new Error(`failure ${failureNumber}`);
        }
        return READY_RESULT;
      });
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });

      const first = supervisor.whenReady();
      const rejected = expect(first).rejects.toThrow("tomcat serve exited");
      for (const delay of [300, 800, 2_000, 4_000]) {
        await vi.advanceTimersByTimeAsync(delay);
      }
      await rejected;

      await expect(supervisor.reconnect()).resolves.toEqual(READY_RESULT);
      expect(initialize).toHaveBeenCalledTimes(6);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not restart a live serve process after the initialize handshake times out", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      const initialize = vi
        .fn<() => Promise<InitializeResult>>()
        .mockRejectedValue(
          new Error("Timed out waiting for control response init-test"),
        );
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
      });

      await expect(supervisor.whenReady()).rejects.toThrow(
        "Timed out waiting for control response init-test",
      );
      expect(initialize).toHaveBeenCalledTimes(1);
      expect(runtime.messenger.restart).not.toHaveBeenCalled();
      expect(runtime.messenger.start).toHaveBeenCalledTimes(1);
      expect(runtime.messenger.stop).toHaveBeenCalledTimes(1);
      expect(supervisor.currentState).toMatchObject({
        attempt: 1,
        failure: { kind: "handshake_timeout" },
        phase: "fatal",
        status: "failed",
      });
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the handshake timeout at least as large as a regular request timeout", () => {
    expect(DEFAULT_HANDSHAKE_TIMEOUT_MS).toBeGreaterThanOrEqual(
      DEFAULT_REQUEST_TIMEOUT_MS,
    );
  });

  it("keeps polling setup without mistaking its expected repeated stderr for a fatal error", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      runtime.messenger.start.mockImplementation(() => {
        runtime.emitExit(exit({ stderr: "fake serve requires tomcat init first" }));
      });
      const supervisor = new ServeConnectionSupervisor({
        initialize: vi.fn<() => Promise<InitializeResult>>(),
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        setupMaxAttempts: 3,
      });

      const ready = supervisor.reconnect("setup");
      const rejected = expect(ready).rejects.toThrow("tomcat serve exited");
      await vi.advanceTimersByTimeAsync(5_000);

      expect(supervisor.currentState).toMatchObject({
        attempt: 2,
        phase: "backoff",
        status: "connecting",
      });
      await vi.advanceTimersByTimeAsync(5_000);
      await rejected;
      expect(supervisor.currentState).toMatchObject({
        failure: { kind: "retry_exhausted" },
        phase: "fatal",
      });
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("reports every transition to interested UI adapters", async () => {
    vi.useFakeTimers();
    try {
      const runtime = createMessenger();
      const initialize = vi
        .fn<() => Promise<InitializeResult>>()
        .mockImplementationOnce(async () => {
          runtime.emitExit(exit({ stderr: "temporary startup failure" }));
          throw new Error("retry");
        })
        .mockResolvedValueOnce(READY_RESULT);
      const supervisor = new ServeConnectionSupervisor({
        initialize,
        isExecutableAvailable: () => true,
        messenger: runtime.messenger as never,
        random: () => 0.5,
      });
      const transitions: ServeConnectionState["phase"][] = [];
      supervisor.onStateChange((state) => transitions.push(state.phase));

      const ready = supervisor.whenReady();
      await retryOnce();
      await ready;

      expect(transitions).toEqual([
        "starting",
        "handshaking",
        "backoff",
        "starting",
        "handshaking",
        "ready",
      ]);
      supervisor.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});
