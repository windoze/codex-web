import { describe, expect, it } from "vitest";
import { HttpError, apiBase, isUnauthorizedError, wsBase } from "./api";

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
    expect(err).toBeInstanceOf(HttpError);
    expect(err.status).toBe(401);
    expect(err.name).toBe("HttpError");
    expect(err.message).toContain("HTTP 401");
  });
});

describe("isUnauthorizedError", () => {
  it("matches HttpError 401", () => {
    const err = new HttpError(401, "Unauthorized", "no token");
    expect(isUnauthorizedError(err)).toBe(true);
  });

  it("matches plain status objects (structural fallback)", () => {
    expect(isUnauthorizedError({ status: 401 })).toBe(true);
  });

  it("does not match other errors", () => {
    expect(isUnauthorizedError(new Error("boom"))).toBe(false);
    expect(isUnauthorizedError({ status: 500 })).toBe(false);
    expect(isUnauthorizedError(null)).toBe(false);
  });
});
