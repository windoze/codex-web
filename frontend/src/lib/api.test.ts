import { describe, expect, it } from "vitest";
import { apiBase, wsBase } from "./api";

describe("apiBase / wsBase", () => {
  it("defaults to local daemon addresses", () => {
    expect(apiBase()).toMatch(/^http:\/\//);
    expect(wsBase()).toMatch(/^ws:\/\//);
  });
});

