import { describe, expect, it } from "vitest";
import type { Conversation, ConversationEvent, ConversationListItem, Project } from "./lib/api";
import {
  bubblePreviewText,
  bubbleStartsExpanded,
  conversationTitleForList,
  deriveRunStatusFromEvents,
  deriveTokenUsageFromEvents,
  eventsToChatItems,
  isTurnInProgress,
  pathBasename,
  updateConversationListRunStatus,
} from "./App";

function e(id: number, event_type: string, payload: unknown, ts_ms = 0): ConversationEvent {
  return {
    id,
    conversation_id: "00000000-0000-0000-0000-000000000000",
    ts_ms,
    event_type,
    payload,
  };
}

describe("ui helpers", () => {
  it("uses project basename when conversation title is default", () => {
    const project: Project = {
      id: "00000000-0000-0000-0000-000000000001",
      name: "my-project",
      root_path: "/Users/alice/work/my-project",
      created_at_ms: 0,
      updated_at_ms: 0,
    };
    const conversation: Conversation = {
      id: "00000000-0000-0000-0000-000000000002",
      project_id: project.id,
      title: "New conversation",
      archived_at_ms: null,
      created_at_ms: 0,
      updated_at_ms: 0,
    };
    expect(conversationTitleForList(conversation, project)).toBe("my-project");
  });

  it("prefers explicit conversation title", () => {
    const project: Project = {
      id: "00000000-0000-0000-0000-000000000001",
      name: "my-project",
      root_path: "/Users/alice/work/my-project",
      created_at_ms: 0,
      updated_at_ms: 0,
    };
    const conversation: Conversation = {
      id: "00000000-0000-0000-0000-000000000002",
      project_id: project.id,
      title: "Bugfixes",
      archived_at_ms: null,
      created_at_ms: 0,
      updated_at_ms: 0,
    };
    expect(conversationTitleForList(conversation, project)).toBe("Bugfixes");
  });

  it("extracts a basename for both unix and windows-ish paths", () => {
    expect(pathBasename("/a/b/c/")).toBe("c");
    expect(pathBasename("C:\\Users\\alice\\repo")).toBe("repo");
  });

  it("shows user-visible codex fields even when raw is off", () => {
    const events = [
      e(1, "codex_event", {
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn_0",
        item: { type: "AgentMessage", id: "item_0", content: [{ type: "Text", text: "hello" }] },
      }),
    ];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items).toHaveLength(1);
    expect(items[0].role).toBe("assistant");
    expect(items[0].text).toBe("hello");
  });

  it("extracts nested item.text for legacy item.completed events", () => {
    const events = [
      e(1, "codex_event", {
        type: "item.completed",
        item: { id: "item_0", type: "reasoning", text: "**Finding markdown files**\n\nhello" },
      }),
    ];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items).toHaveLength(1);
    expect(items[0].role).toBe("assistant");
    expect(items[0].text).toContain("**Finding markdown files**");
    expect(items[0].tone).toBe("reasoning");
  });

  it("renders legacy command_execution items as collapsed preformatted text", () => {
    const events = [
      e(1, "codex_event", {
        type: "item.completed",
        item: { id: "item_0", type: "command_execution", text: "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9" },
      }),
    ];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items).toHaveLength(1);
    expect(items[0].format).toBe("pre");
    expect(items[0].collapsedLines).toBe(8);
  });

  it("renders exec_command_end aggregated_output as collapsed preformatted text", () => {
    const events = [
      e(1, "codex_event", {
        type: "exec_command_end",
        command: ["rg", "--files"],
        exit_code: 0,
        aggregated_output: "a\nb\nc\nd\ne\nf\ng\nh\ni\nj",
        formatted_output: "",
        stdout: "",
        stderr: "",
      }),
    ];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items).toHaveLength(1);
    expect(items[0].format).toBe("pre");
    expect(items[0].collapsedLines).toBe(8);
    expect(items[0].text).toContain("rg --files");
  });

  it("shows raw JSON when raw toggle is on", () => {
    const events = [e(1, "codex_event", { type: "error", message: "boom" })];
    const items = eventsToChatItems(events, { showRawMessages: true });
    expect(items[0].text).toContain("codex_event:");
    expect(items[0].text).toContain("boom");
  });

  it("streams agent message deltas into a single assistant bubble", () => {
    const events = [
      e(1, "codex_event", { type: "agent_message_content_delta", item_id: "item_1", delta: "hel" }),
      e(2, "codex_event", { type: "agent_message_content_delta", item_id: "item_1", delta: "lo" }),
    ];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items).toHaveLength(1);
    expect(items[0].role).toBe("assistant");
    expect(items[0].text).toBe("hello");
    expect(items[0].kind).toBe("agent_message");
  });

  it("tags agent_message events for styling", () => {
    const events = [e(1, "agent_message", { text: "hello" })];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("agent_message");
  });

  it("renders run_status running/completed as Start/Stop splitters when raw is off", () => {
    const events = [
      e(1, "run_status", { status: "running" }, 0),
      e(2, "agent_message", { text: "hello" }, 1_000),
      e(3, "run_status", { status: "completed" }, 2_000),
    ];
    const items = eventsToChatItems(events, { showRawMessages: false });
    expect(items[0].format).toBe("splitter");
    expect(items[0].text).toMatch(/^Start: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
    expect(items.at(-1)?.format).toBe("splitter");
    expect(items.at(-1)?.text).toMatch(/^Stop: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  });

  it("renders run_status as raw pre text when raw is on", () => {
    const events = [e(1, "run_status", { status: "running" }, 0)];
    const items = eventsToChatItems(events, { showRawMessages: true });
    expect(items).toHaveLength(1);
    expect(items[0].format).toBe("pre");
    expect(items[0].text).toContain("run_status:");
    expect(items[0].text).toContain("running");
  });

  it("derives the latest run_status from the event stream", () => {
    const events = [
      e(1, "run_status", { status: "running" }),
      e(2, "agent_message", { text: "hello" }),
      e(3, "run_status", { status: "completed" }),
    ];
    expect(deriveRunStatusFromEvents(events)).toBe("completed");
  });

  it("derives token usage from turn.completed events", () => {
    const events = [
      e(1, "codex_event", {
        type: "turn.completed",
        usage: { cached_input_tokens: 120, input_tokens: 30, output_tokens: 50 },
      }),
    ];
    expect(deriveTokenUsageFromEvents(events)).toEqual({
      cached_input_tokens: 120,
      input_tokens: 30,
      output_tokens: 50,
    });
  });

  it("treats queued/running/waiting_for_interaction as in-progress", () => {
    expect(isTurnInProgress(null)).toBe(false);
    expect(isTurnInProgress("completed")).toBe(false);
    expect(isTurnInProgress("queued")).toBe(true);
    expect(isTurnInProgress("running")).toBe(true);
    expect(isTurnInProgress("waiting_for_interaction")).toBe(true);
  });

  it("collapses message preview to the first line", () => {
    expect(bubblePreviewText("hello")).toBe("hello");
    expect(bubblePreviewText("hello\nworld")).toBe("hello …");
    expect(bubblePreviewText("hello\r\nworld")).toBe("hello …");
    expect(bubblePreviewText("\nworld")).toBe("…");
  });

  it("expands user bubbles and agent_message bubbles by default", () => {
    expect(bubbleStartsExpanded({ role: "user", kind: undefined })).toBe(true);
    expect(bubbleStartsExpanded({ role: "assistant", kind: "agent_message" })).toBe(true);
    expect(bubbleStartsExpanded({ role: "assistant", kind: undefined })).toBe(false);
  });

  it("updates list run_status so spinners can show even when not selected", () => {
    const a: ConversationListItem = {
      id: "00000000-0000-0000-0000-000000000010",
      project_id: null,
      title: "A",
      archived_at_ms: null,
      created_at_ms: 0,
      updated_at_ms: 1,
      run_status: "idle",
    };
    const b: ConversationListItem = {
      id: "00000000-0000-0000-0000-000000000011",
      project_id: null,
      title: "B",
      archived_at_ms: null,
      created_at_ms: 0,
      updated_at_ms: 2,
      run_status: "idle",
    };

    const next = updateConversationListRunStatus([a, b], a.id, "running", 123);
    expect(next[0].run_status).toBe("running");
    expect(next[0].updated_at_ms).toBe(123);
    expect(next[1]).toEqual(b);
  });
});
