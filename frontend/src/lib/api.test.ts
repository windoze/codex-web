import { describe, expect, it } from "vitest";
import { HttpError, apiBase, wsBase } from "./api";

describe("apiBase / wsBase", () => {
  it("defaults to local daemon addresses", () => {
    expect(apiBase()).toMatch(/^http:\/\//);
    expect(wsBase()).toMatch(/^ws:\/\//);
  });
});

describe("HttpError", () => {
  it("exposes http status information", () => {
    const err = new HttpError(401, "Unauthorized", "no token");
    expect(err).toBeInstanceOf(Error);
    expect(err.status).toBe(401);
    expect(err.message).toContain("HTTP 401");
  });
});
