import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  Conversation,
  ConversationListItem,
  ConversationEvent,
  HttpError,
  InteractionRequest,
  Project,
  FsEntry,
  apiBase,
  clearAuthToken,
  createConversation,
  createProject,
  getAuthToken,
  listConversations,
  listEvents,
  listProjects,
  listPendingInteractions,
  setAuthToken,
  fsHome,
  fsList,
  postUserMessage,
  respondInteraction,
  updateConversation,
  wsBase,
} from "./lib/api";

export type ChatItem = {
  key: string;
  role: "user" | "assistant" | "event";
  text: string;
  format: "markdown" | "pre" | "splitter";
  tone?: "normal" | "reasoning";
  kind?: "agent_message";
  collapsedLines?: number;
};

export function bubblePreviewText(text: string): string {
  const idx = text.search(/\r?\n/);
  const firstLine = (idx === -1 ? text : text.slice(0, idx)).trimEnd();
  const hasMore = idx !== -1;
  if (!hasMore) return firstLine;
  if (!firstLine) return "…";
  return `${firstLine} …`;
}

export function bubbleStartsExpanded(item: Pick<ChatItem, "kind" | "role">): boolean {
  // User messages should remain visible by default (they are the prompt/context).
  if (item.role === "user") return true;
  return item.kind === "agent_message";
}

export function deriveRunStatusFromEvents(events: ConversationEvent[]): string | null {
  let status: string | null = null;
  for (const e of events) {
    if (e.event_type !== "run_status") continue;
    const raw = (e.payload as { status?: unknown } | null)?.status;
    if (typeof raw === "string") status = raw;
  }
  return status;
}

export function isTurnInProgress(runStatus: string | null): boolean {
  return runStatus === "queued" || runStatus === "running" || runStatus === "waiting_for_interaction";
}

export function updateConversationListRunStatus(
  conversations: ConversationListItem[],
  conversationId: string,
  status: string,
  updatedAtMs?: number,
): ConversationListItem[] {
  const nextUpdatedAt = typeof updatedAtMs === "number" ? updatedAtMs : null;
  let changed = false;

  const next = conversations.map((c) => {
    if (c.id !== conversationId) return c;

    const mergedUpdatedAt =
      nextUpdatedAt != null ? Math.max(c.updated_at_ms, nextUpdatedAt) : c.updated_at_ms;

    if (c.run_status === status && mergedUpdatedAt === c.updated_at_ms) {
      return c;
    }

    changed = true;
    return {
      ...c,
      run_status: status,
      updated_at_ms: mergedUpdatedAt,
    };
  });

  return changed ? next : conversations;
}

export type TokenUsage = {
  cached_input_tokens: number;
  input_tokens: number;
  output_tokens: number;
};

export function deriveTokenUsageFromEvents(events: ConversationEvent[]): TokenUsage | null {
  let usage: TokenUsage | null = null;
  for (const e of events) {
    if (e.event_type !== "codex_event") continue;
    const payload = e.payload as Record<string, unknown> | null;
    const typ = payload?.type;
    if (typ !== "turn.completed" && typ !== "turn_completed") continue;
    const u = payload?.usage as Record<string, unknown> | null;
    if (!u || typeof u !== "object") continue;
    const cached = u.cached_input_tokens;
    const input = u.input_tokens;
    const output = u.output_tokens;
    if (typeof cached !== "number" || typeof input !== "number" || typeof output !== "number") continue;
    usage = { cached_input_tokens: cached, input_tokens: input, output_tokens: output };
  }
  return usage;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function formatLocalTimestamp(ms: number): string {
  // Local time is more readable for a UI (matches the conversation list’s local timestamp display).
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(
    d.getMinutes(),
  )}:${pad2(d.getSeconds())}`;
}

export function pathBasename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? path;
}

export function formatUpdatedAt(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) {
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  return d.toLocaleDateString([], { month: "short", day: "2-digit" });
}

export function conversationTitleForList(c: Conversation, project: Project | null): string {
  const title = c.title?.trim();
  if (title && title !== "New conversation") return title;
  if (project) return pathBasename(project.root_path);
  return c.title;
}

function extractText(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const maybeText = (payload as { text?: unknown }).text;
  return typeof maybeText === "string" ? maybeText : null;
}

function asStringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  const out: string[] = [];
  for (const part of value) {
    if (typeof part !== "string") return null;
    out.push(part);
  }
  return out;
}

function extractExecCommandEndPre(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const obj = payload as Record<string, unknown>;
  if (obj.type !== "exec_command_end") return null;

  const cmdParts = asStringArray(obj.command);
  const cmd = cmdParts ? cmdParts.join(" ") : null;
  const exitCode = typeof obj.exit_code === "number" ? obj.exit_code : null;

  const aggregated = typeof obj.aggregated_output === "string" ? obj.aggregated_output : "";
  const formatted = typeof obj.formatted_output === "string" ? obj.formatted_output : "";
  const stdout = typeof obj.stdout === "string" ? obj.stdout : "";
  const stderr = typeof obj.stderr === "string" ? obj.stderr : "";

  const output = aggregated || formatted || [stdout, stderr].filter(Boolean).join("\n");
  const header =
    cmd && exitCode != null ? `$ ${cmd} (exit ${exitCode})` : cmd ? `$ ${cmd}` : exitCode != null ? `(exit ${exitCode})` : "";

  if (!header && !output) return null;
  return [header, output].filter(Boolean).join("\n");
}

function extractUserVisibleText(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const obj = payload as Record<string, unknown>;
  if (typeof obj.text === "string") return obj.text;
  if (obj.item && typeof obj.item === "object") {
    const item = obj.item as Record<string, unknown>;
    if (typeof item.text === "string") return item.text;
  }
  if (typeof obj.message === "string") return obj.message;
  if (typeof obj.delta === "string") return obj.delta;
  if (typeof obj.error === "string") return obj.error;
  if (Array.isArray(obj.summary_text) && obj.summary_text.every((x) => typeof x === "string")) {
    return (obj.summary_text as string[]).join("");
  }
  return null;
}

function extractCodexTextItem(payload: unknown): { text: string; itemType: string | null } | null {
  if (!payload || typeof payload !== "object") return null;
  const obj = payload as Record<string, unknown>;

  if (obj.type !== "item_completed" && obj.type !== "item.completed") return null;

  const item = obj.item;
  if (!item || typeof item !== "object") return null;
  const itemObj = item as Record<string, unknown>;

  // Legacy shape: item has a top-level `text`.
  if (typeof itemObj.text === "string") {
    return { text: itemObj.text, itemType: typeof itemObj.type === "string" ? itemObj.type : null };
  }

  if (itemObj.type !== "AgentMessage") return null;

  const content = itemObj.content;
  if (!Array.isArray(content)) return null;

  let out = "";
  for (const part of content) {
    if (typeof part === "string") {
      out += part;
      continue;
    }
    if (!part || typeof part !== "object") continue;
    const p = part as Record<string, unknown>;
    if (typeof p.text === "string") {
      out += p.text;
    }
  }

  if (!out) return null;
  return { text: out, itemType: "agent_message" };
}

function CollapsiblePre({ text, maxLines }: { text: string; maxLines?: number }) {
  const [expanded, setExpanded] = useState(false);
  const limit = typeof maxLines === "number" && maxLines > 0 ? maxLines : null;
  const lines = text.split(/\r?\n/);
  const shouldCollapse = limit != null && lines.length > limit;
  const remaining = shouldCollapse && limit != null ? lines.length - limit : 0;

  const displayText =
    shouldCollapse && !expanded && limit != null
      ? `${lines.slice(0, limit).join("\n")}\n… (${remaining} more lines)`
      : text;

  return (
    <div className="preBlock">
      <pre className="preText">{displayText}</pre>
      {shouldCollapse ? (
        <button className="linkButton" type="button" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "Show less" : "Show more"}
        </button>
      ) : null}
    </div>
  );
}

function Bubble({ item }: { item: ChatItem }) {
  const toneClass = item.tone === "reasoning" ? "bubbleReasoning" : "";
  const kindClass = item.kind === "agent_message" ? "bubbleAgentMessage" : "";
  const [expanded, setExpanded] = useState(() => bubbleStartsExpanded(item));

  if (item.format === "splitter") {
    return (
      <div className="splitter">
        <div className="splitterLine" />
        <div className="splitterText">{item.text}</div>
        <div className="splitterLine" />
      </div>
    );
  }

  const bubbleClass = `bubble ${item.format === "pre" ? "bubblePlain" : "bubbleMarkdown"} ${toneClass} ${kindClass}`.trim();
  const toggle = (
    <button
      className="bubbleToggle"
      type="button"
      onClick={() => setExpanded((v) => !v)}
      aria-label={expanded ? "Collapse message" : "Expand message"}
      title={expanded ? "Collapse message" : "Expand message"}
    >
      {expanded ? "▾" : "▸"}
    </button>
  );

  if (!expanded) {
    return (
      <div className={bubbleClass}>
        <div className="bubbleBody">
          {toggle}
          <div className="bubblePreview">{bubblePreviewText(item.text)}</div>
        </div>
      </div>
    );
  }

  if (item.format === "pre") {
    return (
      <div className={bubbleClass}>
        <div className="bubbleBody">
          {toggle}
          <div className="bubbleContent">
            <CollapsiblePre text={item.text} maxLines={item.collapsedLines} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className={bubbleClass}>
      <div className="bubbleBody">
        {toggle}
        <div className="bubbleContent">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{item.text}</ReactMarkdown>
        </div>
      </div>
    </div>
  );
}

function eventToChatItem(e: ConversationEvent): ChatItem {
  const text = extractText(e.payload);
  if (e.event_type === "user_message") {
    return { key: `e-${e.id}`, role: "user", text: text ?? JSON.stringify(e.payload), format: "markdown" };
  }
  if (e.event_type === "agent_message") {
    return {
      key: `e-${e.id}`,
      role: "assistant",
      text: text ?? JSON.stringify(e.payload),
      format: "markdown",
      tone: "normal",
      kind: "agent_message",
    };
  }
  return {
    key: `e-${e.id}`,
    role: "event",
    text: `${e.event_type}: ${JSON.stringify(e.payload)}`,
    format: "pre",
  };
}

export function eventsToChatItems(
  events: ConversationEvent[],
  opts: { showRawMessages: boolean },
): ChatItem[] {
  const out: ChatItem[] = [];
  let activeStreamIndex: number | null = null;
  let activeStreamItemId: string | null = null;

  for (let i = 0; i < events.length; i++) {
    const e = events[i];

    if (e.event_type === "user_message") {
      out.push(eventToChatItem(e));
      // Starting a new user message usually means any previous streaming assistant output is "done".
      activeStreamIndex = null;
      activeStreamItemId = null;
      continue;
    }

    if (e.event_type === "agent_message") {
      const finalText = extractText(e.payload) ?? JSON.stringify(e.payload);
      if (activeStreamIndex != null) {
        out[activeStreamIndex].text = finalText;
        out[activeStreamIndex].kind = "agent_message";
      } else {
        out.push({
          key: `e-${e.id}`,
          role: "assistant",
          text: finalText,
          format: "markdown",
          tone: "normal",
          kind: "agent_message",
        });
      }
      activeStreamIndex = null;
      activeStreamItemId = null;
      continue;
    }

    if (e.event_type === "run_status") {
      const status = (e.payload as { status?: unknown } | null)?.status;
      const statusStr = typeof status === "string" ? status : null;

      if (opts.showRawMessages) {
        out.push({
          key: `e-${e.id}`,
          role: "event",
          text: `run_status: ${JSON.stringify(e.payload)}`,
          format: "pre",
        });
        continue;
      }

      if (statusStr === "running") {
        out.push({
          key: `e-${e.id}`,
          role: "event",
          text: `Start: ${formatLocalTimestamp(e.ts_ms)}`,
          format: "splitter",
        });
      } else if (statusStr === "completed") {
        out.push({
          key: `e-${e.id}`,
          role: "event",
          text: `Stop: ${formatLocalTimestamp(e.ts_ms)}`,
          format: "splitter",
        });
      } else if (statusStr === "failed" || statusStr === "aborted") {
        out.push({
          key: `e-${e.id}`,
          role: "event",
          text: `Stop (${statusStr}): ${formatLocalTimestamp(e.ts_ms)}`,
          format: "splitter",
        });
      }
      continue;
    }

    if (e.event_type === "codex_event") {
      if (opts.showRawMessages) {
        out.push({
          key: `e-${e.id}`,
          role: "event",
          text: `codex_event: ${JSON.stringify(e.payload)}`,
          format: "pre",
        });
        continue;
      }

      // Prefer user-visible fields over raw JSON when the raw toggle is off.
      const payload = e.payload as Record<string, unknown> | null;
      const codexType = payload?.type;

      const execEnd = extractExecCommandEndPre(e.payload);
      if (execEnd) {
        out.push({
          key: `e-${e.id}`,
          role: "assistant",
          text: execEnd,
          format: "pre",
          collapsedLines: 8,
        });
        continue;
      }

      if (codexType === "agent_message_content_delta") {
        const delta = payload?.delta;
        const itemId = payload?.item_id;
        if (typeof delta === "string") {
          const id = typeof itemId === "string" ? itemId : null;
          if (activeStreamIndex == null || (id != null && id !== activeStreamItemId)) {
            out.push({
              key: `stream-${id ?? e.id}`,
              role: "assistant",
              text: "",
              format: "markdown",
              tone: "normal",
              kind: "agent_message",
            });
            activeStreamIndex = out.length - 1;
            activeStreamItemId = id;
          }
          out[activeStreamIndex].text += delta;
          continue;
        }
      }

      // Fallback: if the backend didn't derive an `agent_message`, show the message text from
      // `item_completed` agent messages.
      const maybeItemText = extractCodexTextItem(e.payload);
      if (maybeItemText) {
        const next = events[i + 1];

        const itemType = maybeItemText.itemType ?? "";
        if (itemType === "reasoning") {
          out.push({
            key: `e-${e.id}`,
            role: "assistant",
            text: maybeItemText.text,
            format: "markdown",
            tone: "reasoning",
          });
          continue;
        }

        if (itemType === "command_execution" || itemType === "commandExecution") {
          out.push({
            key: `e-${e.id}`,
            role: "assistant",
            text: maybeItemText.text,
            format: "pre",
            collapsedLines: 8,
          });
          continue;
        }

        const isAgentMessage = itemType === "agent_message" || itemType === "AgentMessage";
        if (isAgentMessage && next?.event_type === "agent_message") {
          continue;
        }

        out.push({
          key: `e-${e.id}`,
          role: "assistant",
          text: maybeItemText.text,
          format: "markdown",
          tone: "normal",
          kind: "agent_message",
        });
        continue;
      }

      const visible = extractUserVisibleText(e.payload);
      if (visible) {
        out.push({ key: `e-${e.id}`, role: "event", text: visible, format: "pre" });
      }
      continue;
    }

    // Default rendering for internal events.
    const visible = extractUserVisibleText(e.payload);
    out.push({
      key: `e-${e.id}`,
      role: "event",
      text: visible ? `${e.event_type}: ${visible}` : `${e.event_type}: ${JSON.stringify(e.payload)}`,
      format: "pre",
    });
  }

  return out;
}

export default function App() {
  const [authToken, setAuthTokenState] = useState<string>(() => getAuthToken() ?? "");
  const [authStatus, setAuthStatus] = useState<"checking" | "ok" | "needs_login">("checking");
  const [loginTokenDraft, setLoginTokenDraft] = useState<string>(() => getAuthToken() ?? "");
  const [loginBusy, setLoginBusy] = useState(false);
  const [loginError, setLoginError] = useState<string | null>(null);

  const [conversations, setConversations] = useState<ConversationListItem[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [events, setEvents] = useState<ConversationEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pendingInteractions, setPendingInteractions] = useState<InteractionRequest[]>([]);
  const [showRawMessages, setShowRawMessages] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);

  const projectsById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects]);

  const [newConversationOpen, setNewConversationOpen] = useState(false);
  const [newConversationTitle, setNewConversationTitle] = useState("");
  const [newConversationCreating, setNewConversationCreating] = useState(false);

  const [pickerPath, setPickerPath] = useState<string | null>(null);
  const [pickerParent, setPickerParent] = useState<string | null>(null);
  const [pickerEntries, setPickerEntries] = useState<FsEntry[]>([]);
  const [pickerError, setPickerError] = useState<string | null>(null);
  const [pickerLoading, setPickerLoading] = useState(false);
  const [homePath, setHomePath] = useState<string | null>(null);

  const [messageText, setMessageText] = useState("");
  const [isSending, setIsSending] = useState(false);

  const items = useMemo(() => eventsToChatItems(events, { showRawMessages }), [events, showRawMessages]);
  const activeConversation = useMemo(
    () => conversations.find((c) => c.id === activeConversationId) ?? null,
    [conversations, activeConversationId],
  );
  const runStatus = useMemo(() => deriveRunStatusFromEvents(events), [events]);
  const isConversationRunning = isTurnInProgress(runStatus);
  const tokenUsage = useMemo(() => deriveTokenUsageFromEvents(events), [events]);

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimerRef = useRef<number | null>(null);
  const messageListRef = useRef<HTMLDivElement | null>(null);
  const lastEventIdRef = useRef<number>(0);

  function enterLogin(opts: { hadToken: boolean; message: string }) {
    // Clear any persisted token to prevent background polling loops.
    clearAuthToken();
    setAuthTokenState("");
    setAuthStatus("needs_login");
    setLoginError(opts.hadToken ? `Auth token rejected. ${opts.message}` : opts.message);

    // Clear app state so the login screen is all the user sees.
    setConversations([]);
    setProjects([]);
    setActiveConversationId(null);
    setEvents([]);
    setPendingInteractions([]);
  }

  async function bootstrap() {
    setError(null);
    setLoginError(null);
    setAuthStatus("checking");

    const currentToken = getAuthToken() ?? "";
    const hadToken = Boolean(currentToken.trim());

    try {
      const [list, projectList] = await Promise.all([listConversations(), listProjects()]);
      setAuthTokenState(currentToken);
      setConversations(list);
      setProjects(projectList);
      setActiveConversationId((prev) => {
        if (!prev) return list[0]?.id ?? null;
        if (list.some((c) => c.id === prev)) return prev;
        return list[0]?.id ?? null;
      });
      setAuthStatus("ok");
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken, message: "This server requires an auth token." });
        return;
      }
      setError(err instanceof Error ? err.message : String(err));
      setAuthStatus("ok");
    }
  }

  useEffect(() => {
    bootstrap().catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  useEffect(() => {
    if (authStatus !== "ok") return;
    let cancelled = false;

    async function refreshConversations() {
      try {
        const list = await listConversations();
        if (cancelled) return;
        setConversations(list);
        setActiveConversationId((prev) => {
          if (!prev) return list[0]?.id ?? null;
          if (list.some((c) => c.id === prev)) return prev;
          return list[0]?.id ?? null;
        });
      } catch (err: unknown) {
        if (err instanceof HttpError && err.status === 401) {
          enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        }
      }
    }

    // Keep the left-pane run indicators reasonably fresh even when the active conversation changes.
    refreshConversations().catch(() => {});
    const timer = window.setInterval(() => {
      refreshConversations().catch(() => {});
    }, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [authStatus]);

  useEffect(() => {
    lastEventIdRef.current = events.at(-1)?.id ?? 0;
  }, [events]);

  useEffect(() => {
    if (authStatus !== "ok") return;
    if (!activeConversationId) return;

    const conversationId = activeConversationId;
    setError(null);
    setEvents([]);

    let cancelled = false;

    function mergeEvents(missed: ConversationEvent[]) {
      setEvents((prev) => {
        const seen = new Set(prev.map((p) => p.id));
        const merged = [...prev];
        for (const e of missed) {
          if (!seen.has(e.id)) merged.push(e);
        }
        return merged;
      });
    }

    async function connectWs() {
      if (cancelled) return;

      // Close any previous connection / timer.
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
      if (reconnectTimerRef.current) {
        window.clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }

      // Catch up first (covers reconnect gaps).
      try {
        const missed = await listEvents(conversationId, lastEventIdRef.current);
        if (cancelled) return;
        mergeEvents(missed);
      } catch (err: unknown) {
        if (err instanceof HttpError && err.status === 401) {
          enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
          return;
        }
        // ignore; WebSocket may still work
      }

      const params = new URLSearchParams({ conversation_id: conversationId });
      const token = authToken.trim();
      if (token) params.set("token", token);
      const url = `${wsBase()}/ws?${params.toString()}`;
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onmessage = (msg) => {
        try {
          const e = JSON.parse(msg.data as string) as ConversationEvent;
          if (e.conversation_id !== conversationId) return;
          mergeEvents([e]);
          if (e.event_type === "run_status") {
            const status = (e.payload as { status?: unknown } | null)?.status;
            if (typeof status === "string") {
              setConversations((prev) =>
                updateConversationListRunStatus(prev, conversationId, status, e.ts_ms),
              );
            }
          }
        } catch {
          // ignore
        }
      };

      ws.onclose = () => {
        if (cancelled) return;
        reconnectTimerRef.current = window.setTimeout(() => {
          connectWs().catch(() => {});
        }, 1000);
      };
    }

    listEvents(conversationId, 0)
      .then((initialEvents) => {
        if (cancelled) return;
        setEvents(initialEvents);
        const initialRunStatus = deriveRunStatusFromEvents(initialEvents);
        if (initialRunStatus) {
          const updatedAt = initialEvents.at(-1)?.ts_ms;
          setConversations((prev) =>
            updateConversationListRunStatus(prev, conversationId, initialRunStatus, updatedAt),
          );
        }
        connectWs().catch(() => {});
      })
      .catch((err: unknown) => {
        if (err instanceof HttpError && err.status === 401) {
          enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
          return;
        }
        setError(err instanceof Error ? err.message : String(err));
      });

    return () => {
      cancelled = true;
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
      if (reconnectTimerRef.current) {
        window.clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }
    };
  }, [activeConversationId, authToken, authStatus]);

  useEffect(() => {
    if (authStatus !== "ok") {
      setPendingInteractions([]);
      return;
    }
    if (!activeConversationId) {
      setPendingInteractions([]);
      return;
    }

    const conversationId = activeConversationId;
    let cancelled = false;

    async function refresh() {
      try {
        const pending = await listPendingInteractions(conversationId);
        if (!cancelled) setPendingInteractions(pending);
      } catch (err: unknown) {
        if (err instanceof HttpError && err.status === 401) {
          enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
          return;
        }
        // ignore
      }
    }

    refresh().catch(() => {});
    const timer = window.setInterval(() => {
      refresh().catch(() => {});
    }, 1500);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeConversationId, authStatus]);

  useEffect(() => {
    messageListRef.current?.scrollTo({ top: messageListRef.current.scrollHeight });
  }, [items.length]);

  function conversationProject(c: Conversation): Project | null {
    if (!c.project_id) return null;
    return projectsById.get(c.project_id) ?? null;
  }

  function onSelectConversation(conversationId: string) {
    setActiveConversationId(conversationId);
    setSidebarOpen(false);
  }

  async function loadPickerPath(path: string) {
    setPickerError(null);
    setPickerLoading(true);
    try {
      const res = await fsList(path);
      setPickerPath(res.path);
      setPickerParent(res.parent);
      setPickerEntries(res.entries);
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setPickerError(err instanceof Error ? err.message : String(err));
    } finally {
      setPickerLoading(false);
    }
  }

  async function openNewConversationDialog() {
    setNewConversationOpen(true);
    setPickerError(null);
    setNewConversationTitle("");
    setPickerEntries([]);
    setPickerParent(null);
    setPickerPath(null);
    try {
      const home = await fsHome();
      setHomePath(home.path);
      await loadPickerPath(home.path);
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setPickerError(err instanceof Error ? err.message : String(err));
    }
  }

  function closeNewConversationDialog() {
    if (newConversationCreating) return;
    setNewConversationOpen(false);
  }

  async function onConfirmNewConversation() {
    if (!pickerPath) return;
    setPickerError(null);
    setNewConversationCreating(true);
    try {
      const title = newConversationTitle.trim();
      const project = await createProject(pickerPath);
      const conversation = await createConversation(project.id, title || undefined);

      const [nextConversations, nextProjects] = await Promise.all([listConversations(), listProjects()]);
      setConversations(nextConversations);
      setProjects(nextProjects);

      setActiveConversationId(conversation.id);
      setMessageText("");
      setNewConversationOpen(false);
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setPickerError(err instanceof Error ? err.message : String(err));
    } finally {
      setNewConversationCreating(false);
    }
  }

  async function onSendMessage(e: FormEvent) {
    e.preventDefault();
    if (!activeConversationId) return;
    if (!messageText.trim()) return;
    if (isConversationRunning) return;
    setError(null);
    setIsSending(true);
    const previousRunStatus = activeConversation?.run_status ?? null;
    setConversations((prev) => updateConversationListRunStatus(prev, activeConversationId, "running"));
    try {
      await postUserMessage(activeConversationId, messageText);
      setMessageText("");
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setError(err instanceof Error ? err.message : String(err));
      try {
        const list = await listConversations();
        setConversations(list);
      } catch {
        if (previousRunStatus) {
          setConversations((prev) =>
            updateConversationListRunStatus(prev, activeConversationId, previousRunStatus),
          );
        }
      }
    } finally {
      setIsSending(false);
    }
  }

  async function onRespond(interactionId: string, action: string) {
    setError(null);
    try {
      await respondInteraction(interactionId, action);
      if (activeConversationId) {
        const pending = await listPendingInteractions(activeConversationId);
        setPendingInteractions(pending);
      }
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function onRenameConversation() {
    if (!activeConversation) return;
    const nextTitle = window.prompt("New conversation title", activeConversation.title);
    if (!nextTitle) return;
    const trimmed = nextTitle.trim();
    if (!trimmed) return;
    try {
      await updateConversation(activeConversation.id, { title: trimmed });
      const list = await listConversations();
      setConversations(list);
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function onArchiveConversation() {
    if (!activeConversation) return;
    const ok = window.confirm(`Archive "${activeConversation.title}"?`);
    if (!ok) return;
    try {
      await updateConversation(activeConversation.id, { archived: true });
      const list = await listConversations();
      setConversations(list);
      setActiveConversationId(list[0]?.id ?? null);
      setEvents([]);
    } catch (err: unknown) {
      if (err instanceof HttpError && err.status === 401) {
        enterLogin({ hadToken: Boolean((getAuthToken() ?? "").trim()), message: "Please log in again." });
        return;
      }
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function onLogin(e: FormEvent) {
    e.preventDefault();
    const trimmed = loginTokenDraft.trim();
    if (!trimmed) {
      setLoginError("Auth token is required.");
      return;
    }
    setLoginBusy(true);
    setLoginError(null);
    setError(null);
    setAuthToken(trimmed);
    setAuthTokenState(trimmed);
    try {
      await bootstrap();
    } finally {
      setLoginBusy(false);
    }
  }

  function onClearLoginToken() {
    clearAuthToken();
    setAuthTokenState("");
    setLoginTokenDraft("");
    setLoginError(null);
    setError(null);
  }

  async function onLogout() {
    onClearLoginToken();
    await bootstrap();
  }

  if (authStatus === "checking") {
    return (
      <div className="loginPage">
        <div className="loginCard">
          <div className="brand">codex-web</div>
          <div className="muted">API: {apiBase()}</div>
          <div className="loginStatus">
            <span className="spinner" aria-label="Connecting" title="Connecting" /> Connecting…
          </div>
        </div>
      </div>
    );
  }

  if (authStatus === "needs_login") {
    return (
      <div className="loginPage">
        <div className="loginCard">
          <div className="brand">codex-web</div>
          <div className="muted">API: {apiBase()}</div>
          <div className="loginTitle">Login</div>
          {loginError ? <div className="loginError">{loginError}</div> : null}
          <form className="loginForm" onSubmit={onLogin}>
            <input
              className="input"
              type="password"
              value={loginTokenDraft}
              onChange={(e) => setLoginTokenDraft(e.target.value)}
              placeholder="Auth token"
              spellCheck={false}
              autoCapitalize="none"
              autoComplete="off"
              autoCorrect="off"
              disabled={loginBusy}
            />
            <div className="loginActions">
              <button className="button" type="submit" disabled={loginBusy}>
                {loginBusy ? "Connecting…" : "Connect"}
              </button>
              <button className="button" type="button" onClick={onClearLoginToken} disabled={loginBusy}>
                Clear
              </button>
            </div>
          </form>
        </div>
      </div>
    );
  }

  return (
    <div className={sidebarOpen ? "layout sidebarOpen" : "layout"}>
      {sidebarOpen ? (
        <button
          className="sidebarBackdrop"
          type="button"
          aria-label="Close sidebar"
          title="Close sidebar"
          onClick={() => setSidebarOpen(false)}
        />
      ) : null}
      <aside className="sidebar">
        <div className="sidebarHeader">
          <div className="brand">codex-web</div>
          <div className="muted">API: {apiBase()}</div>
          <button
            className="button iconButton sidebarCloseButton"
            type="button"
            onClick={() => setSidebarOpen(false)}
            aria-label="Close sidebar"
            title="Close sidebar"
          >
            <svg viewBox="0 0 24 24" className="icon" aria-hidden="true">
              <path
                d="M18.3 5.7a1 1 0 0 0-1.4 0L12 10.6 7.1 5.7a1 1 0 1 0-1.4 1.4l4.9 4.9-4.9 4.9a1 1 0 1 0 1.4 1.4l4.9-4.9 4.9 4.9a1 1 0 0 0 1.4-1.4l-4.9-4.9 4.9-4.9a1 1 0 0 0 0-1.4z"
                fill="currentColor"
              />
            </svg>
          </button>
        </div>

        <button className="newConversationRow" type="button" onClick={() => openNewConversationDialog()}>
          New conversation…
        </button>

        <div className="conversationList">
          {conversations.map((c) => (
            <button
              key={c.id}
              className={c.id === activeConversationId ? "conversation active" : "conversation"}
              onClick={() => onSelectConversation(c.id)}
              type="button"
            >
              <div className="conversationTopRow">
                <div className="conversationTitleWrap">
                  {isTurnInProgress(c.run_status) || (c.id === activeConversationId && isConversationRunning) ? (
                    <span className="spinner conversationSpinner" aria-label="Turn in progress" title="Turn in progress" />
                  ) : null}
                  <div className="conversationTitle">{conversationTitleForList(c, conversationProject(c))}</div>
                </div>
                <div className="conversationTime">{formatUpdatedAt(c.updated_at_ms)}</div>
              </div>
            </button>
          ))}
          {conversations.length === 0 ? (
            <div className="muted" style={{ padding: 12 }}>
              Click “New conversation…” to start from a project directory.
            </div>
          ) : null}
        </div>
      </aside>

      <main className="chat">
        <div className="chatHeader">
          <div className="chatHeaderLeft">
            <button
              className="button iconButton menuButton"
              type="button"
              onClick={() => setSidebarOpen(true)}
              aria-label="Open conversations"
              title="Open conversations"
            >
              <svg viewBox="0 0 24 24" className="icon" aria-hidden="true">
                <path
                  d="M4 6h16v2H4V6zm0 5h16v2H4v-2zm0 5h16v2H4v-2z"
                  fill="currentColor"
                />
              </svg>
            </button>
            <div className="chatTitle">
              {activeConversation
                ? conversationTitleForList(activeConversation, conversationProject(activeConversation))
                : "No conversation"}
              {tokenUsage ? (
                <span className="tokenUsage">
                  ({tokenUsage.cached_input_tokens} cached tokens, {tokenUsage.input_tokens} input tokens,{" "}
                  {tokenUsage.output_tokens} output tokens)
                </span>
              ) : null}
              {runStatus ? <span className="chatStatus">({runStatus})</span> : null}
            </div>
          </div>
          <div className="chatActions">
            <label className="toggle">
              <input
                type="checkbox"
                checked={showRawMessages}
                onChange={(e) => setShowRawMessages(e.target.checked)}
              />
              <span>Show raw messages</span>
            </label>
            {activeConversation ? (
              <>
                <button className="button" type="button" onClick={onRenameConversation}>
                  Rename
                </button>
                <button className="button" type="button" onClick={onArchiveConversation}>
                  Archive
                </button>
                <a
                  className="button"
                  href={`${apiBase()}/api/conversations/${activeConversation.id}/export?format=md`}
                  target="_blank"
                  rel="noreferrer"
                >
                  Export
                </a>
              </>
            ) : null}
            {authToken ? (
              <button
                className="button iconButton"
                type="button"
                onClick={onLogout}
                aria-label="Log out"
                title="Log out"
              >
                <svg viewBox="0 0 24 24" className="icon" aria-hidden="true">
                  <path
                    d="M10 17l1.4-1.4L8.8 13H20v-2H8.8l2.6-2.6L10 7l-7 7 7 7z"
                    fill="currentColor"
                  />
                  <path
                    d="M4 4h8v2H6v12h6v2H4V4z"
                    fill="currentColor"
                    opacity="0.85"
                  />
                </svg>
              </button>
            ) : null}
          </div>
        </div>

        {error ? <div className="error">{error}</div> : null}

        {pendingInteractions.length > 0 ? (
          <div className="interactions">
            <div className="interactionsTitle">Input required</div>
            {pendingInteractions.map((r) => (
              <div key={r.id} className="interactionCard">
                <div className="interactionKind">{r.kind}</div>
                <div className="interactionBody">
                  <pre className="interactionPayload">{JSON.stringify(r.payload, null, 2)}</pre>
                </div>
                <div className="interactionActions">
                  <button className="button" type="button" onClick={() => onRespond(r.id, "accept")}>
                    Accept
                  </button>
                  <button className="button" type="button" onClick={() => onRespond(r.id, "decline")}>
                    Decline
                  </button>
                </div>
              </div>
            ))}
          </div>
        ) : null}

        <div className="messages" ref={messageListRef}>
          {activeConversationId ? null : (
            <div className="muted" style={{ padding: 12 }}>
              No conversation selected.
            </div>
          )}
          {items.map((m) => (
            <div key={m.key} className={`message ${m.role}`}>
              <Bubble item={m} />
            </div>
          ))}
        </div>

        <form className="composer" onSubmit={onSendMessage}>
          <div className="composerField">
            {isConversationRunning ? (
              <span className="spinner spinnerLarge composerSpinner" aria-label="Turn in progress" title="Turn in progress" />
            ) : null}
            <input
              value={messageText}
              onChange={(e) => setMessageText(e.target.value)}
              className="composerInput"
              placeholder={activeConversationId ? "Send a message…" : "Create/select a conversation first"}
              disabled={!activeConversationId || isSending || isConversationRunning}
            />
          </div>
          <button
            className="button"
            type="submit"
            disabled={!activeConversationId || isSending || isConversationRunning}
          >
            Send
          </button>
        </form>
      </main>

      {newConversationOpen ? (
        <div className="modalBackdrop" role="dialog" aria-modal="true">
          <div className="modal">
            <div className="modalHeader">
              <div className="modalTitle">New conversation</div>
              <button
                className="button"
                type="button"
                onClick={() => closeNewConversationDialog()}
                disabled={newConversationCreating}
              >
                Close
              </button>
            </div>

            <div className="modalBody">
              {pickerError ? <div className="modalError">{pickerError}</div> : null}

              <label className="field">
                <div className="label">Conversation title (optional)</div>
                <input
                  className="input"
                  value={newConversationTitle}
                  onChange={(e) => setNewConversationTitle(e.target.value)}
                  placeholder="Optional"
                  disabled={newConversationCreating}
                />
              </label>

              <div className="field">
                <div className="label">Project directory</div>
                <div className="pickerHeader">
                  <button
                    className="button buttonSmall"
                    type="button"
                    onClick={() => {
                      if (homePath) loadPickerPath(homePath).catch(() => {});
                    }}
                    disabled={!homePath || pickerLoading || newConversationCreating}
                  >
                    Home
                  </button>
                  <button
                    className="button buttonSmall"
                    type="button"
                    onClick={() => {
                      if (pickerParent) loadPickerPath(pickerParent).catch(() => {});
                    }}
                    disabled={!pickerParent || pickerLoading || newConversationCreating}
                  >
                    Up
                  </button>
                  <div className="pickerPath">{pickerPath ?? "Loading…"}</div>
                </div>

                <div className="pickerList">
                  {pickerLoading ? <div className="muted">Loading…</div> : null}
                  {pickerEntries.map((entry) => {
                    const isOpenable = entry.kind === "dir" || entry.kind === "symlink";
                    return (
                      <button
                        key={entry.path}
                        type="button"
                        className={isOpenable ? "pickerEntry" : "pickerEntry pickerEntryDisabled"}
                        onClick={() => {
                          if (isOpenable) loadPickerPath(entry.path).catch(() => {});
                        }}
                        disabled={!isOpenable || pickerLoading || newConversationCreating}
                        title={entry.path}
                      >
                        <span className="pickerEntryName">{entry.name}</span>
                        <span className="pickerEntryKind">{entry.kind}</span>
                      </button>
                    );
                  })}
                </div>
              </div>
            </div>

            <div className="modalFooter">
              <div className="muted">{pickerPath ? `Selected: ${pickerPath}` : ""}</div>
              <button
                className="button"
                type="button"
                onClick={() => onConfirmNewConversation().catch(() => {})}
                disabled={!pickerPath || pickerLoading || newConversationCreating}
              >
                {newConversationCreating ? "Creating…" : "Create"}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
