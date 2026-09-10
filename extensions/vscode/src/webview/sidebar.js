/**
 * sidebar.js — Contexto VS Code Sidebar Webview Logic
 *
 * Runs inside the VS Code webview sandbox. Communicates with the extension
 * host via vscode.postMessage() / window.addEventListener('message').
 *
 * Features:
 * - Live event feed rendering with source badges and relative timestamps
 * - Search with debounced input
 * - Keyboard navigation (j/k, Enter, Esc, /)
 * - Card expand/collapse
 * - Automatic timestamp refresh
 */

// =============================================================================
// VS Code API
// =============================================================================

// @ts-ignore — acquireVsCodeApi is injected by the VS Code webview runtime
const vscode = acquireVsCodeApi();

// =============================================================================
// State
// =============================================================================

let events = [];
let selectedIndex = -1;
let searchMode = false;

// =============================================================================
// DOM References
// =============================================================================

const statusDot = document.getElementById('status-dot');
const statusLabel = document.getElementById('status-label');
const statusMeta = document.getElementById('status-meta');
const searchInput = document.getElementById('search-input');
const contextFeed = document.getElementById('context-feed');
const feedLoading = document.getElementById('feed-loading');
const emptyState = document.getElementById('empty-state');
const activeTask = document.getElementById('active-task');
const taskName = document.getElementById('task-name');
const taskDuration = document.getElementById('task-duration');

// =============================================================================
// Message Handler (from extension host)
// =============================================================================

window.addEventListener('message', (event) => {
  const message = event.data;

  switch (message.type) {
    case 'contextLoaded':
      events = message.events || [];
      renderFeed();
      break;

    case 'newEvent':
      if (message.event && !searchMode) {
        events.unshift(message.event);
        // Keep feed manageable
        if (events.length > 200) {
          events = events.slice(0, 200);
        }
        prependEventCard(message.event, true);
        updateEmptyState();
      }
      break;

    case 'searchResults':
      events = message.events || [];
      searchMode = true;
      renderFeed();
      if (message.error) {
        showFeedMessage('Search failed: ' + message.error);
      }
      break;

    case 'statusUpdate':
      updateStatusBar(message.status, message.state);
      break;
  }
});

// =============================================================================
// Initialization
// =============================================================================

// Tell extension host we're ready
vscode.postMessage({ type: 'ready' });

// =============================================================================
// Rendering
// =============================================================================

function renderFeed() {
  if (feedLoading) {
    feedLoading.style.display = 'none';
  }

  // Clear existing cards
  const existingCards = contextFeed.querySelectorAll('.event-card');
  existingCards.forEach((card) => card.remove());

  if (events.length === 0) {
    updateEmptyState();
    return;
  }

  emptyState.style.display = 'none';

  // Render cards
  const fragment = document.createDocumentFragment();
  events.forEach((event, index) => {
    fragment.appendChild(createEventCard(event, index, false));
  });
  contextFeed.appendChild(fragment);

  selectedIndex = -1;
}

function prependEventCard(event, animate) {
  if (feedLoading) {
    feedLoading.style.display = 'none';
  }
  emptyState.style.display = 'none';

  const card = createEventCard(event, 0, animate);

  // Insert after the loading div (or at the top)
  const firstCard = contextFeed.querySelector('.event-card');
  if (firstCard) {
    contextFeed.insertBefore(card, firstCard);
  } else {
    contextFeed.appendChild(card);
  }

  // Update indices on existing cards
  const cards = contextFeed.querySelectorAll('.event-card');
  cards.forEach((c, i) => {
    c.dataset.index = String(i);
  });
}

function createEventCard(event, index, animate) {
  const card = document.createElement('div');
  card.className = 'event-card' + (animate ? ' new-event' : '');
  card.dataset.index = String(index);
  card.tabIndex = 0;

  const sourceClass = getSourceClass(event.source);
  const sourceLabel = getSourceLabel(event.source);
  const relTime = formatRelativeTime(event.timestamp);
  const redactedHtml = event.was_redacted
    ? '<span class="redacted-badge">[REDACTED]</span>'
    : '';

  card.innerHTML = `
    <div class="card-header">
      <span class="source-badge ${sourceClass}">${sourceLabel}</span>
      ${redactedHtml}
      <span class="card-timestamp" title="${event.timestamp}">${relTime}</span>
    </div>
    <div class="card-label">${escapeHtml(event.label)}</div>
    <div class="card-content">${escapeHtml(event.content || '')}</div>
  `;

  // Click to expand/collapse
  card.addEventListener('click', () => {
    const content = card.querySelector('.card-content');
    if (content) {
      content.classList.toggle('expanded');
    }
    selectCard(parseInt(card.dataset.index || '0', 10));
  });

  return card;
}

function updateEmptyState() {
  if (events.length === 0) {
    emptyState.style.display = 'flex';
  } else {
    emptyState.style.display = 'none';
  }
}

function showFeedMessage(msg) {
  const el = document.createElement('div');
  el.className = 'feed-loading';
  el.innerHTML = `<span class="loading-text">${escapeHtml(msg)}</span>`;
  const cards = contextFeed.querySelectorAll('.event-card, .feed-loading');
  cards.forEach((c) => c.remove());
  contextFeed.appendChild(el);
}

// =============================================================================
// Status Bar
// =============================================================================

function updateStatusBar(status, state) {
  // Update dot
  statusDot.className = 'status-dot ' + (state || 'disconnected');

  // Update label
  if (state === 'connected') {
    statusLabel.textContent = 'Recording';
  } else if (state === 'connecting') {
    statusLabel.textContent = 'Connecting...';
  } else {
    statusLabel.textContent = 'Offline';
  }

  // Update meta
  if (status) {
    const eventCount = status.event_count || 0;
    const version = status.version || '';
    statusMeta.textContent = `v${version} | ${eventCount} events`;

    // Active task
    if (status.active_task) {
      activeTask.style.display = 'flex';
      taskName.textContent = status.active_task.name;
      taskDuration.textContent = formatDurationShort(
        status.active_task.started_at
      );
    } else {
      activeTask.style.display = 'none';
    }
  } else {
    statusMeta.textContent = '';
    activeTask.style.display = 'none';
  }
}

// =============================================================================
// Search
// =============================================================================

let searchDebounce = null;

searchInput.addEventListener('input', () => {
  const query = searchInput.value.trim();

  if (searchDebounce) {
    clearTimeout(searchDebounce);
  }

  if (!query) {
    // Clear search — reload context
    searchMode = false;
    vscode.postMessage({ type: 'loadContext', limit: 30 });
    return;
  }

  searchDebounce = setTimeout(() => {
    vscode.postMessage({ type: 'search', query, limit: 20 });
  }, 300);
});

searchInput.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    searchInput.value = '';
    searchInput.blur();
    searchMode = false;
    vscode.postMessage({ type: 'loadContext', limit: 30 });
  }
});

// =============================================================================
// Keyboard Navigation (DESIGN.md §4.1)
// =============================================================================

document.addEventListener('keydown', (e) => {
  // '/' to focus search
  if (e.key === '/' && document.activeElement !== searchInput) {
    e.preventDefault();
    searchInput.focus();
    return;
  }

  // Escape to blur search
  if (e.key === 'Escape' && document.activeElement === searchInput) {
    searchInput.blur();
    return;
  }

  // j/k or arrow keys for card navigation
  const cards = contextFeed.querySelectorAll('.event-card');
  if (cards.length === 0) return;

  if (e.key === 'j' || e.key === 'ArrowDown') {
    if (document.activeElement === searchInput) return;
    e.preventDefault();
    selectCard(Math.min(selectedIndex + 1, cards.length - 1));
  } else if (e.key === 'k' || e.key === 'ArrowUp') {
    if (document.activeElement === searchInput) return;
    e.preventDefault();
    selectCard(Math.max(selectedIndex - 1, 0));
  } else if (e.key === 'Enter') {
    if (document.activeElement === searchInput) return;
    if (selectedIndex >= 0 && selectedIndex < cards.length) {
      const content = cards[selectedIndex].querySelector('.card-content');
      if (content) {
        content.classList.toggle('expanded');
      }
    }
  }
});

function selectCard(index) {
  const cards = contextFeed.querySelectorAll('.event-card');

  // Deselect previous
  if (selectedIndex >= 0 && selectedIndex < cards.length) {
    cards[selectedIndex].classList.remove('selected');
  }

  selectedIndex = index;

  if (index >= 0 && index < cards.length) {
    cards[index].classList.add('selected');
    cards[index].scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }
}

// =============================================================================
// Timestamp Refresh
// =============================================================================

// Update relative timestamps every 30 seconds
setInterval(() => {
  const timestamps = document.querySelectorAll('.card-timestamp');
  timestamps.forEach((el) => {
    const iso = el.getAttribute('title');
    if (iso) {
      el.textContent = formatRelativeTime(iso);
    }
  });
}, 30000);

// =============================================================================
// Utilities
// =============================================================================

function getSourceClass(source) {
  const s = (source || '').toLowerCase();
  if (s === 'terminal' || s === 'term') return 'terminal';
  if (s === 'git') return 'git';
  if (s === 'editor' || s === 'ide') return 'editor';
  if (s === 'file_system' || s === 'fs') return 'fs';
  if (s === 'manual' || s === 'note') return 'note';
  if (s === 'mcp') return 'mcp';
  return 'terminal';
}

function getSourceLabel(source) {
  const s = (source || '').toLowerCase();
  if (s === 'terminal' || s === 'term') return 'TERM';
  if (s === 'git') return 'GIT';
  if (s === 'editor' || s === 'ide') return 'IDE';
  if (s === 'file_system' || s === 'fs') return 'FS';
  if (s === 'manual' || s === 'note') return 'NOTE';
  if (s === 'mcp') return 'MCP';
  return source ? source.toUpperCase() : '???';
}

function formatRelativeTime(isoTimestamp) {
  if (!isoTimestamp) return '';
  const now = Date.now();
  const then = new Date(isoTimestamp).getTime();
  if (isNaN(then)) return '';
  const diffSec = Math.floor((now - then) / 1000);

  if (diffSec < 5) return 'just now';
  if (diffSec < 60) return diffSec + 's ago';
  if (diffSec < 3600) return Math.floor(diffSec / 60) + 'm ago';
  if (diffSec < 86400) return Math.floor(diffSec / 3600) + 'h ago';
  return Math.floor(diffSec / 86400) + 'd ago';
}

function formatDurationShort(startedAt) {
  if (!startedAt) return '';
  const now = Date.now();
  const then = new Date(startedAt).getTime();
  if (isNaN(then)) return '';
  const diffSec = Math.floor((now - then) / 1000);
  const h = Math.floor(diffSec / 3600);
  const m = Math.floor((diffSec % 3600) / 60);
  if (h > 0) return h + 'h ' + m + 'm';
  return m + 'm';
}

function escapeHtml(str) {
  if (!str) return '';
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}
