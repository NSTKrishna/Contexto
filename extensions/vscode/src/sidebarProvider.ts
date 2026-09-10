/**
 * sidebarProvider.ts — WebviewViewProvider for the Contexto sidebar panel.
 *
 * Renders a Linear/Raycast Dark themed webview using DESIGN.md tokens.
 * Communicates with the extension host via postMessage for:
 * - Receiving live SSE events
 * - Triggering search/remember commands
 * - Displaying daemon status updates
 */

import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { CtxdClient, ContextEvent, StatusResponse } from "./ctxdClient";

export class ContextoSidebarProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "contexto.sidebarView";

  private _view?: vscode.WebviewView;
  private _client: CtxdClient;
  private _extensionUri: vscode.Uri;

  constructor(extensionUri: vscode.Uri, client: CtxdClient) {
    this._extensionUri = extensionUri;
    this._client = client;
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this._view = webviewView;

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this._extensionUri],
    };

    webviewView.webview.html = this._getHtmlContent(webviewView.webview);

    // Handle messages from the webview
    webviewView.webview.onDidReceiveMessage(async (message) => {
      switch (message.type) {
        case "search": {
          try {
            const events = await this._client.search(
              message.query,
              message.limit || 20
            );
            this._postMessage({ type: "searchResults", events });
          } catch (err) {
            this._postMessage({
              type: "searchResults",
              events: [],
              error: String(err),
            });
          }
          break;
        }
        case "remember": {
          try {
            await this._client.remember({ note: message.note });
            vscode.window.showInformationMessage(
              "Contexto: Note saved successfully."
            );
          } catch (err) {
            vscode.window.showErrorMessage(
              `Contexto: Failed to save note — ${err}`
            );
          }
          break;
        }
        case "loadContext": {
          try {
            const events = await this._client.getContext({
              limit: message.limit || 30,
            });
            this._postMessage({ type: "contextLoaded", events });
          } catch (err) {
            this._postMessage({
              type: "contextLoaded",
              events: [],
              error: String(err),
            });
          }
          break;
        }
        case "ready": {
          // Webview initialized — send initial data
          await this._sendInitialData();
          break;
        }
      }
    });

    // Re-send state when the view becomes visible
    webviewView.onDidChangeVisibility(() => {
      if (webviewView.visible) {
        this._sendInitialData();
      }
    });
  }

  /** Push a new SSE event to the webview. */
  public pushEvent(event: ContextEvent): void {
    this._postMessage({ type: "newEvent", event });
  }

  /** Update the daemon status display in the webview. */
  public updateStatus(
    status: StatusResponse | null,
    state: string
  ): void {
    this._postMessage({ type: "statusUpdate", status, state });
  }

  private async _sendInitialData(): Promise<void> {
    try {
      const status = await this._client.getStatus();
      this._postMessage({
        type: "statusUpdate",
        status,
        state: this._client.state,
      });

      const events = await this._client.getContext({ limit: 30 });
      this._postMessage({ type: "contextLoaded", events });
    } catch {
      this._postMessage({
        type: "statusUpdate",
        status: null,
        state: "disconnected",
      });
      this._postMessage({
        type: "contextLoaded",
        events: [],
        error: "Daemon not reachable",
      });
    }
  }

  private _postMessage(message: unknown): void {
    this._view?.webview.postMessage(message);
  }

  // ---------------------------------------------------------------------------
  // HTML Generation
  // ---------------------------------------------------------------------------

  private _getHtmlContent(webview: vscode.Webview): string {
    const nonce = getNonce();

    // Read external files if they exist, otherwise use inline
    const cssPath = path.join(
      this._extensionUri.fsPath,
      "src",
      "webview",
      "sidebar.css"
    );
    const jsPath = path.join(
      this._extensionUri.fsPath,
      "src",
      "webview",
      "sidebar.js"
    );

    const htmlPath = path.join(
      this._extensionUri.fsPath,
      "src",
      "webview",
      "sidebar.html"
    );

    let cssContent = "";
    let jsContent = "";
    let htmlContent = "";

    try {
      cssContent = fs.readFileSync(cssPath, "utf-8");
    } catch {
      cssContent = "/* sidebar.css not found */";
    }

    try {
      jsContent = fs.readFileSync(jsPath, "utf-8");
    } catch {
      jsContent = "/* sidebar.js not found */";
    }

    try {
      htmlContent = fs.readFileSync(htmlPath, "utf-8");
      return htmlContent
        .replace(/\{\{nonce\}\}/g, nonce)
        .replace(/\{\{cspSource\}\}/g, webview.cspSource)
        .replace("{{style}}", cssContent)
        .replace("{{script}}", jsContent);
    } catch {
      // Fallback to inline template
      return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta http-equiv="Content-Security-Policy"
    content="default-src 'none';
             style-src 'nonce-${nonce}';
             script-src 'nonce-${nonce}';
             font-src ${webview.cspSource};">
  <style nonce="${nonce}">
${cssContent}
  </style>
  <title>Contexto</title>
</head>
<body>
  <!-- Status Bar -->
  <div id="status-bar" class="status-bar">
    <div class="status-dot" id="status-dot"></div>
    <span class="status-label" id="status-label">Connecting...</span>
    <span class="status-meta" id="status-meta"></span>
  </div>

  <!-- Search Bar -->
  <div class="search-container">
    <input
      type="text"
      id="search-input"
      class="search-input"
      placeholder="Search context..."
      autocomplete="off"
      spellcheck="false"
    />
    <span class="search-hint">⌘K</span>
  </div>

  <!-- Active Task -->
  <div id="active-task" class="active-task" style="display: none;">
    <span class="task-badge">TASK</span>
    <span class="task-name" id="task-name"></span>
    <span class="task-duration" id="task-duration"></span>
  </div>

  <!-- Context Feed -->
  <div id="context-feed" class="context-feed">
    <div class="feed-loading" id="feed-loading">
      <span class="loading-text">Loading context...</span>
    </div>
  </div>

  <!-- Empty State -->
  <div id="empty-state" class="empty-state" style="display: none;">
    <div class="empty-icon">◉</div>
    <div class="empty-title">No events yet</div>
    <div class="empty-desc">Start editing files to capture context.</div>
  </div>

  <script nonce="${nonce}">
${jsContent}
  </script>
</body>
</html>`;
    }
  }
}

// =============================================================================
// Utilities
// =============================================================================

function getNonce(): string {
  let text = "";
  const possible =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}
