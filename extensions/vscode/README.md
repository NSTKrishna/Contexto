# Contexto — VS Code Extension

> Universal Developer Context Manager — automatically captures editor events and provides a live context feed powered by the `ctxd` daemon.

## Prerequisites

- **`ctxd` daemon running** on `http://127.0.0.1:8942` (see [root README](../../README.md))
- VS Code 1.85.0+

## Setup

1. Start the daemon:
   ```bash
   cargo run -p ctxd --bin ctxd
   ```

2. Open the extension in VS Code:
   ```bash
   code extensions/vscode
   ```

3. Press `F5` to launch the Extension Development Host.

4. Open the **Contexto** sidebar panel from the activity bar.

## Features

### Automatic Event Capture

The extension captures editor events and sends them to `ctxd`:

| Event | Trigger | Source |
|---|---|---|
| File opened | `onDidOpenTextDocument` | `editor` |
| File saved | `onDidSaveTextDocument` | `editor` |
| Selection changed | `onDidChangeTextEditorSelection` (2s debounce) | `editor` |
| Editor focused | `onDidChangeActiveTextEditor` | `editor` |

### Sidebar Panel

A Linear/Raycast Dark themed sidebar with:

- **Status indicator** — Green dot when connected, red when daemon is down
- **Search bar** — BM25 full-text search across all context events
- **Live event feed** — Real-time events via SSE with source badges `[GIT]`, `[IDE]`, `[TERM]`, `[NOTE]`, `[MCP]`
- **Active task** — Current task name and duration
- **Keyboard navigation** — `j`/`k` to move, `Enter` to expand, `Esc` to collapse, `/` to search

### Commands

| Command | Description |
|---|---|
| `Contexto: Toggle Capture` | Pause/resume event capture |
| `Contexto: Search Context` | Open a search input for BM25 full-text search |
| `Contexto: Remember Note` | Save a manual annotation to Contexto |
| `Contexto: Show Daemon Status` | Show daemon version, uptime, event count |

### Configuration

| Setting | Default | Description |
|---|---|---|
| `contexto.selectionDebounceMs` | `2000` | Debounce interval for selection events |
| `contexto.daemonUrl` | `http://127.0.0.1:8942` | ctxd daemon URL |
| `contexto.authTokenPath` | `~/.ctx/auth_token` | Auth token file path |
| `contexto.captureOnOpen` | `true` | Capture file open events |
| `contexto.captureOnSave` | `true` | Capture file save events |
| `contexto.captureSelections` | `true` | Capture selection/cursor events |
| `contexto.captureEditorFocus` | `true` | Capture editor focus events |
| `contexto.healthCheckIntervalMs` | `30000` | Daemon health check interval |

## Security

- Auth token is read from `~/.ctx/auth_token` (mode 0600) — never stored in extension settings
- All requests go to `127.0.0.1` only — never exposed to network
- Webview uses strict Content Security Policy
- File contents are truncated to 8KB per event

## Development

```bash
cd extensions/vscode
npm install
npm run compile    # Build TypeScript
npm run watch      # Watch mode for development
```

Press `F5` in VS Code to launch the Extension Development Host.
