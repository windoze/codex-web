import { describe, expect, it } from "vitest";
import { getSpeechRecognitionConstructor, isSpeechRecognitionSupported } from "./speech";

describe("speech helpers", () => {
  it("returns null when no window-like object is provided", () => {
    expect(getSpeechRecognitionConstructor(null)).toBeNull();
    expect(isSpeechRecognitionSupported(null)).toBe(false);
  });

  it("detects SpeechRecognition", () => {
    function FakeSpeechRecognition() {}
    const win = { SpeechRecognition: FakeSpeechRecognition };
    expect(isSpeechRecognitionSupported(win)).toBe(true);
    expect(getSpeechRecognitionConstructor(win)).toBe(FakeSpeechRecognition);
  });

  it("falls back to webkitSpeechRecognition", () => {
    function FakeWebkit() {}
    const win = { webkitSpeechRecognition: FakeWebkit };
    expect(isSpeechRecognitionSupported(win)).toBe(true);
    expect(getSpeechRecognitionConstructor(win)).toBe(FakeWebkit);
  });
});

