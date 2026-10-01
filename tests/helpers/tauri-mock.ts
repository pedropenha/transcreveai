/**
 * Tauri IPC mock for Playwright webview tests (T-009).
 *
 * The app runs against a real Rust backend normally; inside Playwright we serve
 * only the Vite frontend, so `window.__TAURI_INTERNALS__` and friends are
 * replaced with an in-page implementation whose `invoke` is delegated to the
 * Node side via `page.exposeFunction`.
 *
 * Covered surface (matching @tauri-apps/api 2.x internals):
 * - `window.__TAURI_INTERNALS__` — `invoke`, `transformCallback`,
 *   `unregisterCallback`, `runCallback`, `convertFileSrc`, `metadata`.
 * - `window.__TAURI_EVENT_PLUGIN_INTERNALS__` — `unregisterListener`
 *   (registration rides through `invoke("plugin:event|listen")`).
 * - `window.__TAURI_OS_PLUGIN_INTERNALS__` — synchronous platform data
 *   (platform() reads it without IPC). Defaults to a Windows host; pass
 *   `osInternals` overrides to simulate another OS.
 * - `window.isTauri` — truthy, matching a real webview.
 *
 * Backend→frontend events are emitted with {@link emitTauriEvent}, which looks
 * up listeners registered via `listen()` and invokes them with the same
 * `{ event, id, payload }` shape the real IPC delivers.
 */

import type { Page } from "@playwright/test";

/** Tauri command name ("get_app_settings", "plugin:store|get", …) →
 * the raw value `invoke` resolves with (the bindings add the ok/error
 * envelope), or a function of the invoke args. */
export type CommandHandlers = Record<
  string,
  unknown | ((args: Record<string, unknown>) => unknown)
>;

/** A returning-user settings payload: lands straight on the settings UI. */
export const DEFAULT_APP_SETTINGS = {
  onboarding_completed: true,
  app_language: "en",
  debug_mode: false,
  post_process_enabled: false,
  show_tray_icon: true,
};

export const DEFAULT_HANDLERS: CommandHandlers = {
  get_app_settings: DEFAULT_APP_SETTINGS,
  get_windows_microphone_permission_status: {
    supported: false,
    overall_access: "allowed",
    device_access: "allowed",
    app_access: "allowed",
    desktop_app_access: "allowed",
  },
  get_available_microphones: [],
  get_available_output_devices: [],
  get_available_typing_tools: [],
  get_available_accelerators: [],
  get_available_models: [],
  check_custom_sounds: {},
  check_apple_intelligence_available: false,
  is_laptop: false,
  initialize_enigo: null,
  initialize_shortcuts: null,
  get_history_entries: [],
  "plugin:os|locale": "en-US",
  "plugin:os|hostname": "e2e",
};

export interface MockedIpcCall {
  cmd: string;
  args: Record<string, unknown>;
}

export interface TauriMock {
  /** Every `invoke` the page made, in order. */
  calls: MockedIpcCall[];
}

/**
 * Install the mock before `page.goto(...)`. Handlers not in the merged table
 * resolve to `null` — matching what a no-op backend would return for most
 * commands — and are recorded in `mock.calls` for debugging.
 */
export async function installTauriMock(
  page: Page,
  handlers: CommandHandlers = {},
  options: { osInternals?: Record<string, unknown> } = {},
): Promise<TauriMock> {
  const table: CommandHandlers = { ...DEFAULT_HANDLERS, ...handlers };
  const mock: TauriMock = { calls: [] };

  await page.exposeFunction("__e2eInvoke", (cmd: string, args: unknown) => {
    mock.calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
    const handler = table[cmd];
    if (typeof handler === "function") {
      return handler(args as Record<string, unknown>);
    }
    if (cmd in table) {
      return handler;
    }
    return null;
  });

  const osInternals = {
    eol: "\r\n",
    platform: "windows",
    version: "10.0.26200",
    family: "windows",
    os_type: "nt",
    arch: "x86_64",
    exe_extension: "exe",
    ...options.osInternals,
  };

  await page.addInitScript((os) => {
    const w = window as unknown as Record<string, unknown> & {
      __e2eInvoke: (cmd: string, args: unknown) => unknown;
    };

    // Callback registry shared by transformCallback and event delivery.
    const callbacks = new Map<number, (data: unknown) => void>();
    let nextCallbackId = 1;
    // event name -> set of callback ids registered through listen()
    const listeners = new Map<string, Set<number>>();
    let nextEventId = 1;
    const eventIndex = new Map<number, { event: string; cbId: number }>();

    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (event: string, eventId: number) => {
        const entry = eventIndex.get(eventId);
        if (entry) {
          listeners.get(entry.event)?.delete(entry.cbId);
          eventIndex.delete(eventId);
        }
      },
    };

    w.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "hub" },
        currentWebview: { label: "hub", windowLabel: "hub" },
        currentWebviewWindow: { label: "hub" },
        windows: [{ label: "hub" }, { label: "flowbar" }],
        webviews: [{ label: "hub" }, { label: "flowbar" }],
      },
      convertFileSrc: (filePath: string) => filePath,
      transformCallback: (cb: (data: unknown) => void, once = false) => {
        const id = nextCallbackId++;
        callbacks.set(id, (data: unknown) => {
          if (once) callbacks.delete(id);
          cb(data);
        });
        return id;
      },
      unregisterCallback: (id: number) => {
        callbacks.delete(id);
      },
      runCallback: (id: number, data: unknown) => {
        callbacks.get(id)?.(data);
      },
      invoke: (cmd: string, payload?: Record<string, unknown>) => {
        // Event-plugin traffic is resolved locally but still reported to the
        // Node side so tests can observe listener registration in mock.calls.
        if (cmd === "plugin:event|listen") {
          void w.__e2eInvoke(cmd, payload ?? {});
          const event = String(payload?.event ?? "");
          const cbId = Number(payload?.handler ?? 0);
          if (!listeners.has(event)) listeners.set(event, new Set());
          listeners.get(event)?.add(cbId);
          const eventId = nextEventId++;
          eventIndex.set(eventId, { event, cbId });
          return Promise.resolve(eventId);
        }
        if (cmd === "plugin:event|unlisten") {
          void w.__e2eInvoke(cmd, payload ?? {});
          const eventId = Number(payload?.eventId ?? 0);
          const entry = eventIndex.get(eventId);
          if (entry) {
            listeners.get(entry.event)?.delete(entry.cbId);
            eventIndex.delete(eventId);
          }
          return Promise.resolve(null);
        }
        if (cmd === "plugin:event|emit" || cmd === "plugin:event|emit_to") {
          return Promise.resolve(w.__e2eInvoke(cmd, payload ?? {}));
        }
        return Promise.resolve(w.__e2eInvoke(cmd, payload ?? {}));
      },
      plugins: {},
    };

    w.__TAURI_OS_PLUGIN_INTERNALS__ = os;
    w.isTauri = true;

    // Test-side entry point: deliver a backend event to all registered
    // listeners, same envelope the real IPC produces.
    w.__e2eEmit = (event: string, payload: unknown) => {
      for (const cbId of listeners.get(event) ?? []) {
        callbacks.get(cbId)?.({ event, id: 0, payload });
      }
    };
  }, osInternals);

  return mock;
}

/** Emit a backend→frontend event (e.g. "recording-error", "mic-level"). */
export async function emitTauriEvent(
  page: Page,
  event: string,
  payload: unknown,
): Promise<void> {
  await page.evaluate(
    ([ev, data]) => {
      (
        window as unknown as {
          __e2eEmit: (event: string, payload: unknown) => void;
        }
      ).__e2eEmit(ev, data);
    },
    [event, payload],
  );
}
