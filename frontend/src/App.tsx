import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  Conversation,
  ConversationEvent,
  InteractionRequest,
  Project,
  FsEntry,
  apiBase,
  createConversation,
  createProject,
  listConversations,
  listEvents,
  listProjects,
  listPendingInteractions,
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
  format: "markdown" | "pre";
  tone?: "normal" | "reasoning";
  collapsedLines?: number;
};

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

  if (item.format === "pre") {
    return (
      <div className={`bubble bubblePlain ${toneClass}`.trim()}>
        <CollapsiblePre text={item.text} maxLines={item.collapsedLines} />
      </div>
    );
  }

  return (
    <div className={`bubble bubbleMarkdown ${toneClass}`.trim()}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{item.text}</ReactMarkdown>
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
  opts: { showRawCodexEvents: boolean },
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
      } else {
        out.push({ key: `e-${e.id}`, role: "assistant", text: finalText, format: "markdown", tone: "normal" });
      }
      activeStreamIndex = null;
      activeStreamItemId = null;
      continue;
    }

    if (e.event_type === "codex_event") {
      if (opts.showRawCodexEvents) {
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
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [events, setEvents] = useState<ConversationEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pendingInteractions, setPendingInteractions] = useState<InteractionRequest[]>([]);
  const [showRawCodexEvents, setShowRawCodexEvents] = useState(false);

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

  const items = useMemo(() => eventsToChatItems(events, { showRawCodexEvents }), [events, showRawCodexEvents]);
  const activeConversation = useMemo(
    () => conversations.find((c) => c.id === activeConversationId) ?? null,
    [conversations, activeConversationId],
  );
  const runStatus = useMemo(() => deriveRunStatusFromEvents(events), [events]);
  const isConversationRunning = isTurnInProgress(runStatus);

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimerRef = useRef<number | null>(null);
  const messageListRef = useRef<HTMLDivElement | null>(null);
  const lastEventIdRef = useRef<number>(0);

  useEffect(() => {
    Promise.all([listConversations(), listProjects()])
      .then(([list, projectList]) => {
        setConversations(list);
        setProjects(projectList);
        if (list.length > 0) setActiveConversationId((prev) => prev ?? list[0].id);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  useEffect(() => {
    lastEventIdRef.current = events.at(-1)?.id ?? 0;
  }, [events]);

  useEffect(() => {
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
      } catch {
        // ignore; WebSocket may still work
      }

      const url = `${wsBase()}/ws?conversation_id=${encodeURIComponent(conversationId)}`;
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onmessage = (msg) => {
        try {
          const e = JSON.parse(msg.data as string) as ConversationEvent;
          if (e.conversation_id !== conversationId) return;
          mergeEvents([e]);
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
        connectWs().catch(() => {});
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));

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
  }, [activeConversationId]);

  useEffect(() => {
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
      } catch {
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
  }, [activeConversationId]);

  useEffect(() => {
    messageListRef.current?.scrollTo({ top: messageListRef.current.scrollHeight });
  }, [items.length]);

  function conversationProject(c: Conversation): Project | null {
    if (!c.project_id) return null;
    return projectsById.get(c.project_id) ?? null;
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
    try {
      await postUserMessage(activeConversationId, messageText);
      setMessageText("");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
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
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className="layout">
      <aside className="sidebar">
        <div className="sidebarHeader">
          <div className="brand">codex-web</div>
          <div className="muted">API: {apiBase()}</div>
        </div>

        <button className="newConversationRow" type="button" onClick={() => openNewConversationDialog()}>
          New conversation…
        </button>

        <div className="conversationList">
          {conversations.map((c) => (
            <button
              key={c.id}
              className={c.id === activeConversationId ? "conversation active" : "conversation"}
              onClick={() => setActiveConversationId(c.id)}
              type="button"
            >
              <div className="conversationTopRow">
                <div className="conversationTitle">{conversationTitleForList(c, conversationProject(c))}</div>
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
          <div className="chatTitle">
            {activeConversation
              ? conversationTitleForList(activeConversation, conversationProject(activeConversation))
              : "No conversation"}
            {runStatus ? <span className="chatStatus">({runStatus})</span> : null}
          </div>
          <div className="chatActions">
            <label className="toggle">
              <input
                type="checkbox"
                checked={showRawCodexEvents}
                onChange={(e) => setShowRawCodexEvents(e.target.checked)}
              />
              <span>Show raw</span>
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
          <div className="composerInputWrap">
            {isConversationRunning ? (
              <span
                className="spinner spinnerLarge composerSpinner"
                aria-label="Turn in progress"
                title="Turn in progress"
              />
            ) : null}
            <input
              value={messageText}
              onChange={(e) => setMessageText(e.target.value)}
              className={isConversationRunning ? "input composerInput hasSpinner" : "input composerInput"}
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
