/**
 * ctxdClient.ts — TypeScript HTTP client for the ctxd daemon REST API.
 *
 * Zero external dependencies — uses Node.js built-in `http` module.
 * Handles:
 * - Auth token management (reads from ~/.ctx/auth_token, auto-refreshes on 401)
 * - Request timeout (5s default, prevents UI freeze)
 * - Connection health polling
 * - SSE stream relay for live event feed
 */

import * as http from "http";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";

// =============================================================================
// Types (mirrors ctx-core Rust types)
// =============================================================================

export interface ContextEvent {
  id: string;
  timestamp: string;
  source: string;
  label: string;
  content: string;
  metadata?: Record<string, unknown>;
  was_redacted: boolean;
  cwd?: string;
  git_repo?: string;
  task_id?: string;
}

export interface IngestEventRequest {
  source: string;
  label: string;
  content: string;
  metadata?: Record<string, unknown>;
  cwd?: string;
  git_repo?: string;
  task_id?: string;
  was_redacted?: boolean;
}

export interface RememberRequest {
  note: string;
  label?: string;
  cwd?: string;
  git_repo?: string;
}

export interface StatusResponse {
  version: string;
  uptime_seconds: number;
  event_count: number;
  active_task?: {
    id: string;
    name: string;
    status: string;
    event_count: number;
    started_at: string;
  };
  db_path: string;
  listen_addr: string;
}

export type DaemonState = "connected" | "disconnected" | "connecting";

export interface ClientOptions {
  baseUrl: string;
  authTokenPath: string;
  timeoutMs: number;
  healthCheckIntervalMs: number;
}

// =============================================================================
// Security Validation
// =============================================================================

/**
 * Validates that a URL points strictly to a loopback address.
 * Blocks any attempt to exfiltrate local credentials to remote hosts.
 */
export function isLoopbackUrl(urlStr: string): boolean {
  try {
    const url = new URL(urlStr);
    const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, "");
    return (
      host === "127.0.0.1" ||
      host === "localhost" ||
      host === "::1" ||
      host === "0.0.0.0"
    );
  } catch {
    return false;
  }
}

// =============================================================================
// CtxdClient
// =============================================================================

export class CtxdClient {
  private _baseUrl: string;
  private _authTokenPath: string;
  private _timeoutMs: number;
  private _token: string | null = null;
  private _state: DaemonState = "disconnected";
  private _healthCheckTimer: NodeJS.Timeout | null = null;
  private _sseRequest: http.ClientRequest | null = null;
  private _sseReconnectTimer: NodeJS.Timeout | null = null;

  /** Callbacks for state changes and incoming SSE events */
  public onStateChange: ((state: DaemonState) => void) | null = null;
  public onEvent: ((event: ContextEvent) => void) | null = null;

  constructor(options: Partial<ClientOptions> = {}) {
    const rawUrl = options.baseUrl || "http://127.0.0.1:8942";
    if (!isLoopbackUrl(rawUrl)) {
      throw new Error(
        `SecurityError: ctxd daemon URL must resolve to a loopback address (127.0.0.1, localhost, [::1]) to prevent credential exfiltration. Got: ${rawUrl}`
      );
    }
    this._baseUrl = rawUrl;
    this._authTokenPath =
      options.authTokenPath || path.join(os.homedir(), ".ctx", "auth_token");
    this._timeoutMs = options.timeoutMs || 5000;
    this._loadToken();
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  /** Start the client: load token, verify connection, begin health checks. */
  async start(healthCheckIntervalMs: number = 30000): Promise<void> {
    this._setState("connecting");
    this._loadToken();

    try {
      await this.getStatus();
      this._setState("connected");
    } catch {
      this._setState("disconnected");
    }

    // Periodic health check
    this._healthCheckTimer = setInterval(async () => {
      try {
        await this.getStatus();
        if (this._state !== "connected") {
          this._setState("connected");
        }
      } catch {
        if (this._state !== "disconnected") {
          this._setState("disconnected");
        }
      }
    }, healthCheckIntervalMs);
  }

  /** Stop the client: clear timers and close SSE connection. */
  stop(): void {
    if (this._healthCheckTimer) {
      clearInterval(this._healthCheckTimer);
      this._healthCheckTimer = null;
    }
    this.stopEventStream();
    this._setState("disconnected");
  }

  get state(): DaemonState {
    return this._state;
  }

  // ---------------------------------------------------------------------------
  // Auth Token
  // ---------------------------------------------------------------------------

  private _loadToken(): void {
    try {
      this._token = fs.readFileSync(this._authTokenPath, "utf-8").trim();
    } catch {
      this._token = null;
    }
  }

  /** Force re-read the token (called after a 401 response). */
  private _refreshToken(): void {
    this._loadToken();
  }

  // ---------------------------------------------------------------------------
  // HTTP Helpers
  // ---------------------------------------------------------------------------

  private _request<T>(
    method: string,
    urlPath: string,
    body?: unknown,
    retried: boolean = false
  ): Promise<T> {
    return new Promise((resolve, reject) => {
      const url = new URL(urlPath, this._baseUrl);
      const headers: http.OutgoingHttpHeaders = {
        "Content-Type": "application/json",
      };

      if (this._token) {
        headers["Authorization"] = `Bearer ${this._token}`;
      }

      const options: http.RequestOptions = {
        method,
        hostname: url.hostname,
        port: url.port,
        path: url.pathname + url.search,
        timeout: this._timeoutMs,
        headers,
      };

      const req = http.request(options, (res) => {
        let data = "";
        res.on("data", (chunk: Buffer) => {
          data += chunk.toString();
        });
        res.on("end", () => {
          if (res.statusCode === 401) {
            if (!retried) {
              // Token may have rotated — re-read and retry once
              this._refreshToken();
              this._request<T>(method, urlPath, body, true)
                .then(resolve)
                .catch(reject);
              return;
            }
            reject(
              new Error("Unauthorized (401) — token authentication failed")
            );
            return;
          }
          if (
            res.statusCode &&
            res.statusCode >= 200 &&
            res.statusCode < 300
          ) {
            if (data.length === 0) {
              resolve(undefined as T);
            } else {
              try {
                resolve(JSON.parse(data) as T);
              } catch {
                resolve(data as unknown as T);
              }
            }
          } else {
            reject(
              new Error(`HTTP ${res.statusCode}: ${data.substring(0, 200)}`)
            );
          }
        });
      });

      req.on("error", (err) => reject(err));
      req.on("timeout", () => {
        req.destroy();
        reject(new Error("Request timed out"));
      });

      if (body !== undefined) {
        req.write(JSON.stringify(body));
      }
      req.end();
    });
  }

  // ---------------------------------------------------------------------------
  // REST API Methods
  // ---------------------------------------------------------------------------

  /** POST /events — Ingest a context event (returns 202). */
  async ingestEvent(event: IngestEventRequest): Promise<void> {
    try {
      await this._request<void>("POST", "/events", event);
      if (this._state !== "connected") {
        this._setState("connected");
      }
    } catch (err) {
      // Silently drop on failure — event capture should never block the editor
      if (this._state === "connected") {
        this._setState("disconnected");
      }
    }
  }

  /** GET /context — Fetch recent events with optional filters. */
  async getContext(params?: {
    source?: string;
    limit?: number;
    git_repo?: string;
  }): Promise<ContextEvent[]> {
    const query = new URLSearchParams();
    if (params?.source) query.set("source", params.source);
    if (params?.limit) query.set("limit", String(params.limit));
    if (params?.git_repo) query.set("git_repo", params.git_repo);
    const qs = query.toString();
    return this._request<ContextEvent[]>(
      "GET",
      `/context${qs ? "?" + qs : ""}`
    );
  }

  /** GET /search?q=... — Full-text BM25 search. */
  async search(query: string, limit: number = 20): Promise<ContextEvent[]> {
    const qs = new URLSearchParams({
      q: query,
      limit: String(limit),
    }).toString();
    return this._request<ContextEvent[]>("GET", `/search?${qs}`);
  }

  /** POST /remember — Save a manual annotation. */
  async remember(req: RememberRequest): Promise<void> {
    return this._request<void>("POST", "/remember", req);
  }

  /** GET /status — Daemon health and runtime stats. */
  async getStatus(): Promise<StatusResponse> {
    return this._request<StatusResponse>("GET", "/status");
  }

  // ---------------------------------------------------------------------------
  // SSE Event Stream
  // ---------------------------------------------------------------------------

  /** Connect to GET /events/stream (SSE) and relay events via onEvent callback. */
  startEventStream(): void {
    this.stopEventStream();

    const url = new URL("/events/stream", this._baseUrl);
    const sseHeaders: http.OutgoingHttpHeaders = {
      Accept: "text/event-stream",
      "Cache-Control": "no-cache",
    };

    if (this._token) {
      sseHeaders["Authorization"] = `Bearer ${this._token}`;
    }

    const options: http.RequestOptions = {
      method: "GET",
      hostname: url.hostname,
      port: url.port,
      path: url.pathname,
      headers: sseHeaders,
    };

    this._sseRequest = http.request(options, (res) => {
      if (res.statusCode !== 200) {
        if (res.statusCode === 401) {
          this._refreshToken();
        }
        res.resume(); // drain response
        this._scheduleStreamReconnect();
        return;
      }

      let buffer = "";

      res.on("data", (chunk: Buffer) => {
        buffer += chunk.toString();
        const lines = buffer.split("\n");
        buffer = lines.pop() || "";

        for (const line of lines) {
          if (line.startsWith("data:")) {
            const data = line.slice(5).trim();
            if (data && data !== "keep-alive") {
              try {
                const event = JSON.parse(data) as ContextEvent;
                this.onEvent?.(event);
              } catch {
                // Ignore malformed SSE data
              }
            }
          }
        }
      });

      res.on("end", () => {
        this._scheduleStreamReconnect();
      });
    });

    this._sseRequest.on("error", () => {
      this._scheduleStreamReconnect();
    });

    this._sseRequest.end();
  }

  /** Schedule SSE reconnection with delay. */
  private _scheduleStreamReconnect(delayMs: number = 5000): void {
    this.stopEventStream();
    if (this._sseReconnectTimer) {
      clearTimeout(this._sseReconnectTimer);
    }
    this._sseReconnectTimer = setTimeout(() => {
      this._sseReconnectTimer = null;
      if (this._state !== "disconnected") {
        this.startEventStream();
      }
    }, delayMs);
  }

  /** Close the SSE connection. */
  stopEventStream(): void {
    if (this._sseReconnectTimer) {
      clearTimeout(this._sseReconnectTimer);
      this._sseReconnectTimer = null;
    }
    if (this._sseRequest) {
      this._sseRequest.destroy();
      this._sseRequest = null;
    }
  }

  // ---------------------------------------------------------------------------
  // Internal
  // ---------------------------------------------------------------------------

  private _setState(state: DaemonState): void {
    if (this._state !== state) {
      this._state = state;
      this.onStateChange?.(state);

      // Auto-heal event stream when daemon transitions back to connected
      if (state === "connected" && !this._sseRequest) {
        this.startEventStream();
      }
    }
  }
}
