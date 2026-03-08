export type Uuid = string;

export type Project = {
  id: Uuid;
  name: string;
  root_path: string;
  created_at_ms: number;
  updated_at_ms: number;
};

export type Conversation = {
  id: Uuid;
  project_id: Uuid | null;
  title: string;
  created_at_ms: number;
  updated_at_ms: number;
  archived_at_ms: number | null;
};

export type ConversationEvent = {
  id: number;
  conversation_id: Uuid;
  ts_ms: number;
  event_type: string;
  payload: unknown;
};

export type InteractionRequest = {
  id: Uuid;
  conversation_id: Uuid;
  kind: string;
  status: "pending" | "resolved";
  payload: unknown;
  created_at_ms: number;
  timeout_ms: number;
  default_action: string;
  resolved_at_ms: number | null;
  resolved_by: string | null;
  response: unknown | null;
};

const DEFAULT_API_BASE = "http://127.0.0.1:8787";

export function apiBase(): string {
  const envBase = (import.meta as unknown as { env?: Record<string, unknown> }).env?.VITE_API_BASE;
  return (typeof envBase === "string" ? envBase : undefined) ?? DEFAULT_API_BASE;
}

export function wsBase(): string {
  const base = apiBase();
  if (base.startsWith("https://")) return base.replace("https://", "wss://");
  if (base.startsWith("http://")) return base.replace("http://", "ws://");
  return base;
}

async function jsonFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${apiBase()}${path}`, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init?.headers ?? {}),
    },
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`HTTP ${res.status} ${res.statusText}: ${text}`);
  }
  return (await res.json()) as T;
}

export function listConversations(): Promise<Conversation[]> {
  return jsonFetch<Conversation[]>("/api/conversations");
}

export function createProject(rootPath: string, name?: string): Promise<Project> {
  return jsonFetch<Project>("/api/projects", {
    method: "POST",
    body: JSON.stringify({ root_path: rootPath, name }),
  });
}

export function createConversation(projectId: Uuid | null, title?: string): Promise<Conversation> {
  return jsonFetch<Conversation>("/api/conversations", {
    method: "POST",
    body: JSON.stringify({ project_id: projectId, title }),
  });
}

export function updateConversation(
  conversationId: Uuid,
  patch: { title?: string; archived?: boolean },
): Promise<Conversation> {
  return jsonFetch<Conversation>(`/api/conversations/${conversationId}`, {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
}

export function listEvents(conversationId: Uuid, after = 0): Promise<ConversationEvent[]> {
  const params = new URLSearchParams({ after: String(after), limit: "5000" });
  return jsonFetch<ConversationEvent[]>(
    `/api/conversations/${conversationId}/events?${params.toString()}`,
  );
}

export function postUserMessage(conversationId: Uuid, text: string): Promise<ConversationEvent> {
  return jsonFetch<ConversationEvent>(`/api/conversations/${conversationId}/messages`, {
    method: "POST",
    body: JSON.stringify({ text }),
  });
}

export function listPendingInteractions(conversationId: Uuid): Promise<InteractionRequest[]> {
  return jsonFetch<InteractionRequest[]>(`/api/conversations/${conversationId}/interactions`);
}

export function respondInteraction(
  interactionId: Uuid,
  action: string,
  text?: string,
): Promise<{ status: string }> {
  return jsonFetch<{ status: string }>(`/api/interactions/${interactionId}/respond`, {
    method: "POST",
    body: JSON.stringify({ action, text }),
  });
}
