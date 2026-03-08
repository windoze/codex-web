import { describe, expect, it } from "vitest";
import type { ConversationEvent } from "./lib/api";
import { deriveRunStatusFromEvents, eventsToChatItems, isTurnInProgress } from "./App";

function e(id: number, event_type: string, payload: unknown): ConversationEvent {
  return {
    id,
    conversation_id: "00000000-0000-0000-0000-000000000000",
    ts_ms: 0,
    event_type,
    payload,
  };
}

describe("ui helpers", () => {
  it("shows user-visible codex fields even when raw is off", () => {
    const events = [
      e(1, "codex_event", {
        type: "item_completed",
        thread_id: "t",
        turn_id: "turn_0",
        item: { type: "AgentMessage", id: "item_0", content: [{ type: "Text", text: "hello" }] },
      }),
    ];
    const items = eventsToChatItems(events, { showRawCodexEvents: false });
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
    const items = eventsToChatItems(events, { showRawCodexEvents: false });
    expect(items).toHaveLength(1);
    expect(items[0].role).toBe("assistant");
    expect(items[0].text).toContain("**Finding markdown files**");
  });

  it("shows raw JSON when raw toggle is on", () => {
    const events = [e(1, "codex_event", { type: "error", message: "boom" })];
    const items = eventsToChatItems(events, { showRawCodexEvents: true });
    expect(items[0].text).toContain("codex_event:");
    expect(items[0].text).toContain("boom");
  });

  it("streams agent message deltas into a single assistant bubble", () => {
    const events = [
      e(1, "codex_event", { type: "agent_message_content_delta", item_id: "item_1", delta: "hel" }),
      e(2, "codex_event", { type: "agent_message_content_delta", item_id: "item_1", delta: "lo" }),
    ];
    const items = eventsToChatItems(events, { showRawCodexEvents: false });
    expect(items).toHaveLength(1);
    expect(items[0].role).toBe("assistant");
    expect(items[0].text).toBe("hello");
  });

  it("derives the latest run_status from the event stream", () => {
    const events = [
      e(1, "run_status", { status: "running" }),
      e(2, "agent_message", { text: "hello" }),
      e(3, "run_status", { status: "completed" }),
    ];
    expect(deriveRunStatusFromEvents(events)).toBe("completed");
  });

  it("treats queued/running/waiting_for_interaction as in-progress", () => {
    expect(isTurnInProgress(null)).toBe(false);
    expect(isTurnInProgress("completed")).toBe(false);
    expect(isTurnInProgress("queued")).toBe(true);
    expect(isTurnInProgress("running")).toBe(true);
    expect(isTurnInProgress("waiting_for_interaction")).toBe(true);
  });
});
