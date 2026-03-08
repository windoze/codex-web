import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  Conversation,
  ConversationEvent,
  InteractionRequest,
  apiBase,
  createConversation,
  createProject,
  listConversations,
  listEvents,
  listPendingInteractions,
  postUserMessage,
  respondInteraction,
  updateConversation,
  wsBase,
} from "./lib/api";

type ChatItem = {
  key: string;
  role: "user" | "assistant" | "event";
  text: string;
};

export function isRawCodexEvent(e: ConversationEvent): boolean {
  return e.event_type === "codex_event";
}

export function filterEventsForDisplay(
  events: ConversationEvent[],
  opts: { showRawCodexEvents: boolean },
): ConversationEvent[] {
  if (opts.showRawCodexEvents) return events;
  return events.filter((e) => !isRawCodexEvent(e));
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

function extractText(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const maybeText = (payload as { text?: unknown }).text;
  return typeof maybeText === "string" ? maybeText : null;
}

function eventToChatItem(e: ConversationEvent): ChatItem {
  const text = extractText(e.payload);
  if (e.event_type === "user_message") {
    return { key: `e-${e.id}`, role: "user", text: text ?? JSON.stringify(e.payload) };
  }
  if (e.event_type === "agent_message") {
    return { key: `e-${e.id}`, role: "assistant", text: text ?? JSON.stringify(e.payload) };
  }
  return { key: `e-${e.id}`, role: "event", text: `${e.event_type}: ${JSON.stringify(e.payload)}` };
}

export default function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [events, setEvents] = useState<ConversationEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pendingInteractions, setPendingInteractions] = useState<InteractionRequest[]>([]);
  const [showRawCodexEvents, setShowRawCodexEvents] = useState(false);

  const [newProjectPath, setNewProjectPath] = useState("");
  const [newConversationTitle, setNewConversationTitle] = useState("");

  const [messageText, setMessageText] = useState("");
  const [isSending, setIsSending] = useState(false);

  const displayEvents = useMemo(
    () => filterEventsForDisplay(events, { showRawCodexEvents }),
    [events, showRawCodexEvents],
  );
  const items = useMemo(() => displayEvents.map(eventToChatItem), [displayEvents]);
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
    listConversations()
      .then((list) => {
        setConversations(list);
        if (list.length > 0) setActiveConversationId((prev) => prev ?? list[0].id);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  useEffect(() => {
    lastEventIdRef.current = events.at(-1)?.id ?? 0;
  }, [events]);

  useEffect(() => {
    if (!activeConversationId) return;

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
        const missed = await listEvents(activeConversationId, lastEventIdRef.current);
        if (cancelled) return;
        mergeEvents(missed);
      } catch {
        // ignore; WebSocket may still work
      }

      const url = `${wsBase()}/ws?conversation_id=${encodeURIComponent(activeConversationId)}`;
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onmessage = (msg) => {
        try {
          const e = JSON.parse(msg.data as string) as ConversationEvent;
          if (e.conversation_id !== activeConversationId) return;
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

    listEvents(activeConversationId, 0)
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

    let cancelled = false;

    async function refresh() {
      try {
        const pending = await listPendingInteractions(activeConversationId);
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

  async function onCreateConversation(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      if (!newProjectPath.trim()) {
        throw new Error("Project directory is required");
      }
      const project = await createProject(newProjectPath.trim());
      const conversation = await createConversation(project.id, newConversationTitle.trim() || undefined);
      setNewConversationTitle("");
      setMessageText("");
      const list = await listConversations();
      setConversations(list);
      setActiveConversationId(conversation.id);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
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

        <form className="newConversation" onSubmit={onCreateConversation}>
          <label>
            <div className="label">Project directory</div>
            <input
              value={newProjectPath}
              onChange={(e) => setNewProjectPath(e.target.value)}
              placeholder="/path/to/project"
              className="input"
            />
          </label>
          <label>
            <div className="label">Conversation title</div>
            <input
              value={newConversationTitle}
              onChange={(e) => setNewConversationTitle(e.target.value)}
              placeholder="Optional"
              className="input"
            />
          </label>
          <button className="button" type="submit">
            New conversation
          </button>
        </form>

        <div className="sectionTitle">Conversations</div>
        <div className="conversationList">
          {conversations.map((c) => (
            <button
              key={c.id}
              className={c.id === activeConversationId ? "conversation active" : "conversation"}
              onClick={() => setActiveConversationId(c.id)}
              type="button"
            >
              <div className="conversationTitle">{c.title}</div>
              <div className="conversationMeta">{c.id.slice(0, 8)}</div>
            </button>
          ))}
          {conversations.length === 0 ? (
            <div className="muted" style={{ padding: 12 }}>
              Create a conversation from a project directory to get started.
            </div>
          ) : null}
        </div>
      </aside>

      <main className="chat">
        <div className="chatHeader">
          <div className="chatTitle">
            {activeConversation ? activeConversation.title : "No conversation"}
            {isConversationRunning ? <span className="spinner" title="Turn in progress" /> : null}
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
              <div className="bubble">{m.text}</div>
            </div>
          ))}
        </div>

        <form className="composer" onSubmit={onSendMessage}>
          <input
            value={messageText}
            onChange={(e) => setMessageText(e.target.value)}
            className="input"
            placeholder={activeConversationId ? "Send a message…" : "Create/select a conversation first"}
            disabled={!activeConversationId || isSending || isConversationRunning}
          />
          <button
            className="button"
            type="submit"
            disabled={!activeConversationId || isSending || isConversationRunning}
          >
            Send
          </button>
        </form>
      </main>
    </div>
  );
}
