import { describe, expect, it } from "vitest";
import type { ConversationEvent } from "./lib/api";
import {
  deriveRunStatusFromEvents,
  filterEventsForDisplay,
  isRawCodexEvent,
  isTurnInProgress,
} from "./App";

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
  it("classifies codex_event as raw", () => {
    expect(isRawCodexEvent(e(1, "codex_event", {}))).toBe(true);
    expect(isRawCodexEvent(e(2, "agent_message", {}))).toBe(false);
  });

  it("filters raw codex events when toggle is off", () => {
    const events = [e(1, "user_message", { text: "hi" }), e(2, "codex_event", { type: "item_completed" })];
    expect(filterEventsForDisplay(events, { showRawCodexEvents: false }).map((x) => x.id)).toEqual([1]);
    expect(filterEventsForDisplay(events, { showRawCodexEvents: true }).map((x) => x.id)).toEqual([1, 2]);
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

