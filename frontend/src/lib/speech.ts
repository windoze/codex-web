export type SpeechRecognitionLike = {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  onresult: ((event: unknown) => void) | null;
  onend: (() => void) | null;
  onerror: ((event: unknown) => void) | null;
  start: () => void;
  stop: () => void;
  abort: () => void;
};

export type SpeechRecognitionConstructorLike = new () => SpeechRecognitionLike;

export function getSpeechRecognitionConstructor(win: unknown): SpeechRecognitionConstructorLike | null {
  if (!win || typeof win !== "object") return null;
  const w = win as Record<string, unknown>;
  const ctor = (w.SpeechRecognition ?? w.webkitSpeechRecognition) as unknown;
  if (typeof ctor !== "function") return null;
  return ctor as SpeechRecognitionConstructorLike;
}

export function isSpeechRecognitionSupported(win: unknown): boolean {
  return getSpeechRecognitionConstructor(win) != null;
}

export function speechErrorMessage(event: unknown): string {
  if (!event || typeof event !== "object") return "Voice input error.";
  const obj = event as Record<string, unknown>;
  const err = obj.error;
  if (typeof err === "string" && err.trim()) return err;
  return "Voice input error.";
}

