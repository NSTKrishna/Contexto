/**
 * ctxdClient.test.ts — Unit tests for the ctxd TypeScript HTTP client.
 *
 * Uses Node.js built-in test runner (node:test) and assert (node:assert).
 * Zero external testing framework dependencies.
 */

import { test, describe, before, after } from "node:test";
import * as assert from "node:assert";
import * as http from "node:http";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { CtxdClient, isLoopbackUrl } from "../ctxdClient";

describe("Loopback URL Security Validation", () => {
  test("allows loopback hosts", () => {
    assert.strictEqual(isLoopbackUrl("http://127.0.0.1:8942"), true);
    assert.strictEqual(isLoopbackUrl("http://localhost:8942"), true);
    assert.strictEqual(isLoopbackUrl("http://[::1]:8942"), true);
    assert.strictEqual(isLoopbackUrl("http://0.0.0.0:8942"), true);
  });

  test("rejects remote and external hosts", () => {
    assert.strictEqual(isLoopbackUrl("http://attacker.com"), false);
    assert.strictEqual(isLoopbackUrl("https://evil.net:8942"), false);
    assert.strictEqual(isLoopbackUrl("http://192.168.1.100:8942"), false);
    assert.strictEqual(isLoopbackUrl("http://10.0.0.1:8942"), false);
    assert.strictEqual(isLoopbackUrl("invalid-url"), false);
  });

  test("CtxdClient constructor blocks non-loopback URLs", () => {
    assert.throws(
      () => {
        new CtxdClient({ baseUrl: "http://malicious-site.com" });
      },
      {
        message: /SecurityError/,
      }
    );
  });
});

describe("CtxdClient HTTP Requests & Auth", () => {
  let server: http.Server;
  let serverPort: number;
  let tokenFilePath: string;
  let requestCount = 0;

  before(async () => {
    // Create temporary token file
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "ctxd-test-"));
    tokenFilePath = path.join(tmpDir, "auth_token");
    fs.writeFileSync(tokenFilePath, "initial-secret-token", "utf-8");

    // Mock HTTP server
    server = http.createServer((req, res) => {
      requestCount++;
      const authHeader = req.headers["authorization"];

      if (req.url === "/status") {
        if (authHeader === "Bearer valid-token") {
          res.writeHead(200, { "Content-Type": "application/json" });
          res.end(
            JSON.stringify({
              version: "0.1.0",
              uptime_seconds: 42,
              event_count: 10,
              db_path: "/tmp/ctx.db",
              listen_addr: "127.0.0.1:8942",
            })
          );
        } else if (authHeader === "Bearer initial-secret-token") {
          // Simulate expired token on first request -> rotate token file
          fs.writeFileSync(tokenFilePath, "valid-token", "utf-8");
          res.writeHead(401, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ error: "Unauthorized" }));
        } else {
          res.writeHead(401, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ error: "Unauthorized" }));
        }
      } else if (req.url === "/events/stream") {
        if (authHeader !== "Bearer valid-token") {
          res.writeHead(401);
          res.end();
          return;
        }
        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        });
        res.write('data: {"id":"1","timestamp":"2026-09-10T12:00:00Z","source":"editor","label":"test","content":"hello","was_redacted":false}\n\n');
        // keep open briefly then end
        setTimeout(() => res.end(), 100);
      } else {
        res.writeHead(404);
        res.end();
      }
    });

    await new Promise<void>((resolve) => {
      server.listen(0, "127.0.0.1", () => {
        const addr = server.address() as { port: number };
        serverPort = addr.port;
        resolve();
      });
    });
  });

  after(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    try {
      fs.unlinkSync(tokenFilePath);
    } catch {}
  });

  test("auto-refreshes token and retries request on 401", async () => {
    requestCount = 0;
    const client = new CtxdClient({
      baseUrl: `http://127.0.0.1:${serverPort}`,
      authTokenPath: tokenFilePath,
      timeoutMs: 2000,
    });

    const status = await client.getStatus();
    assert.strictEqual(status.version, "0.1.0");
    assert.strictEqual(status.event_count, 10);
    // Request count should be 2: initial 401 + auto-retry 200
    assert.strictEqual(requestCount, 2);
  });

  test("state management and SSE stream handling", async () => {
    const client = new CtxdClient({
      baseUrl: `http://127.0.0.1:${serverPort}`,
      authTokenPath: tokenFilePath,
      timeoutMs: 2000,
    });

    let receivedEvent = false;
    client.onEvent = (evt) => {
      if (evt.label === "test") {
        receivedEvent = true;
      }
    };

    client.startEventStream();
    // Wait briefly for SSE event
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.strictEqual(receivedEvent, true);
    client.stopEventStream();
  });

  test("rejects when 401 persists after token refresh", async () => {
    // Overwrite token file with permanently invalid token
    const invalidTokenPath = path.join(path.dirname(tokenFilePath), "bad_token");
    fs.writeFileSync(invalidTokenPath, "invalid-token", "utf-8");

    const client = new CtxdClient({
      baseUrl: `http://127.0.0.1:${serverPort}`,
      authTokenPath: invalidTokenPath,
      timeoutMs: 2000,
    });

    await assert.rejects(
      async () => {
        await client.getStatus();
      },
      {
        message: /Unauthorized \(401\)/,
      }
    );

    try {
      fs.unlinkSync(invalidTokenPath);
    } catch {}
  });
});

