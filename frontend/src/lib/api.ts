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

export type ConversationListItem = Conversation & {
  run_status: string;
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

export type FsEntry = {
  name: string;
  path: string;
  kind: "dir" | "file" | "symlink" | "other";
};

export type FsListResponse = {
  path: string;
  parent: string | null;
  entries: FsEntry[];
};

export class HttpError extends Error {
  status: number;
  statusText: string;
  bodyText: string;

  constructor(status: number, statusText: string, bodyText: string) {
    super(`HTTP ${status} ${statusText}: ${bodyText}`);
    this.name = "HttpError";
    this.status = status;
    this.statusText = statusText;
    this.bodyText = bodyText;
    // Ensure instanceof works reliably across browsers/transpilation targets.
    Object.setPrototypeOf(this, HttpError.prototype);
  }
}

export function httpStatusFromUnknown(err: unknown): number | null {
  if (!err || typeof err !== "object") return null;
  const status = (err as { status?: unknown }).status;
  return typeof status === "number" ? status : null;
}

export function isUnauthorizedError(err: unknown): boolean {
  return httpStatusFromUnknown(err) === 401;
}

const DEFAULT_API_BASE = "http://127.0.0.1:8787";
const AUTH_TOKEN_STORAGE_KEY = "codex_web_auth_token";

export function apiBase(): string {
  const envBase = (import.meta as unknown as { env?: Record<string, unknown> }).env?.VITE_API_BASE;
  if (typeof envBase === "string" && envBase.trim()) return envBase;

  // In production the UI is often served by the daemon. Default to the current origin so mobile
  // devices work without rebuilding with VITE_API_BASE. In dev (Vite), fall back to localhost.
  if (typeof window !== "undefined") {
    const origin = window.location?.origin;
    const host = window.location?.hostname ?? "";
    const port = window.location?.port ?? "";

    const isLocalHost = host === "localhost" || host === "127.0.0.1";
    const isViteDevPort = port === "5173" || port === "5174";

    if (isLocalHost && isViteDevPort) return DEFAULT_API_BASE;
    if (typeof origin === "string" && origin.trim() && origin !== "null") return origin;
  }

  return DEFAULT_API_BASE;
}

export function wsBase(): string {
  const base = apiBase();
  if (base.startsWith("https://")) return base.replace("https://", "wss://");
  if (base.startsWith("http://")) return base.replace("http://", "ws://");
  return base;
}

function safeLocalStorage(): Storage | null {
  try {
    if (typeof window === "undefined") return null;
    return window.localStorage;
  } catch {
    return null;
  }
}

export function getAuthToken(): string | null {
  const storage = safeLocalStorage();
  if (!storage) return null;
  const token = storage.getItem(AUTH_TOKEN_STORAGE_KEY);
  const trimmed = token?.trim();
  return trimmed ? trimmed : null;
}

export function setAuthToken(token: string): void {
  const storage = safeLocalStorage();
  if (!storage) return;
  storage.setItem(AUTH_TOKEN_STORAGE_KEY, token);
}

export function clearAuthToken(): void {
  const storage = safeLocalStorage();
  if (!storage) return;
  storage.removeItem(AUTH_TOKEN_STORAGE_KEY);
}

async function jsonFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getAuthToken();
  const res = await fetch(`${apiBase()}${path}`, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(init?.headers ?? {}),
    },
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new HttpError(res.status, res.statusText, text);
  }
  return (await res.json()) as T;
}

export function listConversations(): Promise<ConversationListItem[]> {
  return jsonFetch<ConversationListItem[]>("/api/conversations");
}

export function listProjects(): Promise<Project[]> {
  return jsonFetch<Project[]>("/api/projects");
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

export function fsHome(): Promise<{ path: string }> {
  return jsonFetch<{ path: string }>("/api/fs/home");
}

export function fsList(path: string): Promise<FsListResponse> {
  const params = new URLSearchParams({ path });
  return jsonFetch<FsListResponse>(`/api/fs/list?${params.toString()}`);
}
