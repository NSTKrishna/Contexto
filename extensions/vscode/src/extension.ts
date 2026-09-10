/**
 * extension.ts — Contexto VS Code Extension Entry Point.
 *
 * Captures editor events (file open, save, selection, focus) and sends them
 * to the ctxd daemon via the REST API. Manages the sidebar webview and
 * status bar indicator.
 *
 * Activation: onStartupFinished (captures events from first interaction).
 * Deactivation: stops the ctxd client and clears all listeners.
 */

import * as vscode from "vscode";
import * as path from "path";
import {
  CtxdClient,
  IngestEventRequest,
  DaemonState,
  isLoopbackUrl,
} from "./ctxdClient";
import { ContextoSidebarProvider } from "./sidebarProvider";

// =============================================================================
// Module State
// =============================================================================

let client: CtxdClient;
let sidebarProvider: ContextoSidebarProvider;
let statusBarItem: vscode.StatusBarItem;
let isCapturing = true;
let selectionDebounceTimer: NodeJS.Timeout | null = null;

// =============================================================================
// Activation
// =============================================================================

export async function activate(
  context: vscode.ExtensionContext
): Promise<void> {
  const config = vscode.workspace.getConfiguration("contexto");

  // ── 1. Initialize ctxd client ─────────────────────────────────────────────
  let daemonUrl = config.get<string>("daemonUrl") || "http://127.0.0.1:8942";
  if (!isLoopbackUrl(daemonUrl)) {
    vscode.window.showErrorMessage(
      `Contexto: Insecure daemonUrl "${daemonUrl}" rejected. Only loopback addresses (127.0.0.1, localhost, [::1]) are permitted to prevent credential exfiltration. Falling back to default.`
    );
    daemonUrl = "http://127.0.0.1:8942";
  }

  client = new CtxdClient({
    baseUrl: daemonUrl,
    authTokenPath: config.get<string>("authTokenPath") || undefined,
    timeoutMs: 5000,
  });

  client.onStateChange = (state: DaemonState) => {
    updateStatusBar(state);
    sidebarProvider?.updateStatus(null, state);
  };

  client.onEvent = (event) => {
    sidebarProvider?.pushEvent(event);
  };

  const healthCheckMs =
    config.get<number>("healthCheckIntervalMs") || 30000;
  await client.start(healthCheckMs);

  // Connect SSE stream for live events
  client.startEventStream();

  // ── 2. Status bar ─────────────────────────────────────────────────────────
  statusBarItem = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    100
  );
  statusBarItem.command = "contexto.showStatus";
  updateStatusBar(client.state);
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // ── 3. Sidebar webview provider ───────────────────────────────────────────
  sidebarProvider = new ContextoSidebarProvider(
    context.extensionUri,
    client
  );
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(
      ContextoSidebarProvider.viewType,
      sidebarProvider
    )
  );

  // ── 4. Editor event listeners ─────────────────────────────────────────────
  const selectionDebounceMs =
    config.get<number>("selectionDebounceMs") || 2000;

  // File opened
  if (config.get<boolean>("captureOnOpen") !== false) {
    context.subscriptions.push(
      vscode.workspace.onDidOpenTextDocument((doc) => {
        if (!isCapturing || shouldIgnoreDocument(doc)) return;
        sendEvent({
          source: "editor",
          label: `Opened: ${basename(doc.uri)}`,
          content: getDocumentSnippet(doc, 20),
          ...getWorkspaceContext(doc.uri),
        });
      })
    );
  }

  // File saved
  if (config.get<boolean>("captureOnSave") !== false) {
    context.subscriptions.push(
      vscode.workspace.onDidSaveTextDocument((doc) => {
        if (!isCapturing || shouldIgnoreDocument(doc)) return;
        sendEvent({
          source: "editor",
          label: `Saved: ${basename(doc.uri)}`,
          content: getDocumentSnippet(doc, 30),
          ...getWorkspaceContext(doc.uri),
        });
      })
    );
  }

  // Selection / cursor changes (debounced)
  if (config.get<boolean>("captureSelections") !== false) {
    context.subscriptions.push(
      vscode.window.onDidChangeTextEditorSelection((e) => {
        if (!isCapturing || shouldIgnoreDocument(e.textEditor.document))
          return;

        if (selectionDebounceTimer) {
          clearTimeout(selectionDebounceTimer);
        }

        selectionDebounceTimer = setTimeout(() => {
          const doc = e.textEditor.document;
          const selection = e.selections[0];
          const content = selection.isEmpty
            ? getCursorContext(doc, selection.active)
            : doc.getText(selection).substring(0, 2000);

          sendEvent({
            source: "editor",
            label: `Editing: ${basename(doc.uri)} L${selection.active.line + 1}`,
            content,
            ...getWorkspaceContext(doc.uri),
          });
        }, selectionDebounceMs);
      })
    );
  }

  // Editor focus change
  if (config.get<boolean>("captureEditorFocus") !== false) {
    context.subscriptions.push(
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        if (!isCapturing || !editor || shouldIgnoreDocument(editor.document))
          return;
        sendEvent({
          source: "editor",
          label: `Focused: ${basename(editor.document.uri)}`,
          content: `Language: ${editor.document.languageId}, Lines: ${editor.document.lineCount}`,
          ...getWorkspaceContext(editor.document.uri),
        });
      })
    );
  }

  // ── 5. Commands ───────────────────────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand("contexto.toggleCapture", () => {
      isCapturing = !isCapturing;
      updateStatusBar(client.state);
      vscode.window.showInformationMessage(
        `Contexto: Capture ${isCapturing ? "resumed" : "paused"}.`
      );
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("contexto.search", async () => {
      const query = await vscode.window.showInputBox({
        prompt: "Search developer context (BM25 full-text)",
        placeHolder: "e.g. ring buffer, cargo test, auth middleware...",
      });

      if (!query) return;

      try {
        const events = await client.search(query, 20);
        if (events.length === 0) {
          vscode.window.showInformationMessage(
            `Contexto: No results for "${query}".`
          );
          return;
        }

        // Show results in a quick pick
        const items = events.map((e) => ({
          label: `[${e.source.toUpperCase()}] ${e.label}`,
          description: formatRelativeTime(e.timestamp),
          detail: e.content.substring(0, 200),
        }));

        await vscode.window.showQuickPick(items, {
          title: `Contexto: Results for "${query}" (${events.length})`,
          matchOnDescription: true,
          matchOnDetail: true,
        });
      } catch (err) {
        vscode.window.showErrorMessage(
          `Contexto: Search failed — ${err}`
        );
      }
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("contexto.remember", async () => {
      const note = await vscode.window.showInputBox({
        prompt: "Save a note to Contexto",
        placeHolder: "e.g. Decided to use LanceDB for vector search...",
      });

      if (!note) return;

      try {
        await client.remember({ note });
        vscode.window.showInformationMessage("Contexto: Note saved.");
      } catch (err) {
        vscode.window.showErrorMessage(
          `Contexto: Failed to save note — ${err}`
        );
      }
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("contexto.showStatus", async () => {
      try {
        const status = await client.getStatus();
        const uptime = formatDuration(status.uptime_seconds);
        const taskInfo = status.active_task
          ? `\nActive Task: ${status.active_task.name} (${status.active_task.event_count} events)`
          : "\nNo active task";

        vscode.window.showInformationMessage(
          `Contexto Daemon v${status.version}\n` +
            `Uptime: ${uptime}\n` +
            `Events: ${status.event_count}\n` +
            `DB: ${status.db_path}` +
            taskInfo,
          { modal: true }
        );
      } catch (err) {
        vscode.window.showErrorMessage(
          `Contexto: Daemon not reachable — ${err}`
        );
      }
    })
  );

  // ── 6. Configuration change listener ──────────────────────────────────────
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("contexto")) {
        // Reload configuration would require restart
        vscode.window.showInformationMessage(
          "Contexto: Configuration changed. Reload window to apply."
        );
      }
    })
  );
}

// =============================================================================
// Deactivation
// =============================================================================

export function deactivate(): void {
  if (selectionDebounceTimer) {
    clearTimeout(selectionDebounceTimer);
  }
  client?.stop();
}

// =============================================================================
// Helpers
// =============================================================================

/** Send an event to the ctxd daemon (fire-and-forget). */
function sendEvent(event: IngestEventRequest): void {
  client.ingestEvent(event);
}

/** Get the file basename from a URI. */
function basename(uri: vscode.Uri): string {
  return path.basename(uri.fsPath);
}

/** Check if a document should be ignored (output panels, untitled, settings). */
function shouldIgnoreDocument(doc: vscode.TextDocument): boolean {
  if (doc.uri.scheme !== "file") return true;
  if (doc.uri.fsPath.includes(".git/")) return true;
  if (doc.uri.fsPath.includes("node_modules/")) return true;
  if (doc.uri.fsPath.endsWith(".vsix")) return true;
  return false;
}

/** Get the first N lines of a document as a content snippet. */
function getDocumentSnippet(
  doc: vscode.TextDocument,
  maxLines: number
): string {
  const lineCount = Math.min(doc.lineCount, maxLines);
  const lines: string[] = [];
  for (let i = 0; i < lineCount; i++) {
    lines.push(doc.lineAt(i).text);
  }
  return lines.join("\n");
}

/** Get a few lines around the cursor position for context. */
function getCursorContext(
  doc: vscode.TextDocument,
  pos: vscode.Position
): string {
  const startLine = Math.max(0, pos.line - 3);
  const endLine = Math.min(doc.lineCount - 1, pos.line + 3);
  const lines: string[] = [];
  for (let i = startLine; i <= endLine; i++) {
    const prefix = i === pos.line ? "» " : "  ";
    lines.push(`${prefix}${doc.lineAt(i).text}`);
  }
  return lines.join("\n");
}

/** Extract workspace-relative cwd and git_repo from a URI. */
function getWorkspaceContext(
  uri: vscode.Uri
): { cwd?: string; git_repo?: string } {
  const result: { cwd?: string; git_repo?: string } = {};

  const folder = vscode.workspace.getWorkspaceFolder(uri);
  if (folder) {
    result.cwd = folder.uri.fsPath;
    result.git_repo = path.basename(folder.uri.fsPath);
  } else {
    result.cwd = path.dirname(uri.fsPath);
  }

  return result;
}

/** Format a UTC ISO timestamp into a relative time string. */
function formatRelativeTime(isoTimestamp: string): string {
  const now = Date.now();
  const then = new Date(isoTimestamp).getTime();
  const diffSec = Math.floor((now - then) / 1000);

  if (diffSec < 60) return `${diffSec}s ago`;
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
  return `${Math.floor(diffSec / 86400)}d ago`;
}

/** Format seconds into a human-readable duration. */
function formatDuration(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

// =============================================================================
// Status Bar
// =============================================================================

function updateStatusBar(state: DaemonState): void {
  if (!statusBarItem) return;

  if (!isCapturing) {
    statusBarItem.text = "$(debug-pause) Contexto";
    statusBarItem.tooltip = "Contexto: Capture paused (click to toggle)";
    statusBarItem.backgroundColor = new vscode.ThemeColor(
      "statusBarItem.warningBackground"
    );
    return;
  }

  switch (state) {
    case "connected":
      statusBarItem.text = "$(pulse) Contexto";
      statusBarItem.tooltip = "Contexto: Connected to ctxd (click for status)";
      statusBarItem.backgroundColor = undefined;
      break;
    case "connecting":
      statusBarItem.text = "$(sync~spin) Contexto";
      statusBarItem.tooltip = "Contexto: Connecting to ctxd...";
      statusBarItem.backgroundColor = undefined;
      break;
    case "disconnected":
      statusBarItem.text = "$(error) Contexto";
      statusBarItem.tooltip =
        "Contexto: Daemon not reachable (click for status)";
      statusBarItem.backgroundColor = new vscode.ThemeColor(
        "statusBarItem.errorBackground"
      );
      break;
  }
}
