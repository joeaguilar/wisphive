import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

describe("frontend environment (itr#118)", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("accepts an omitted VITE_API_URL (itr#118)", async () => {
    const { environment } = await import("./env");

    expect(environment.apiUrl).toBeUndefined();
  });

  it("throws at module load when VITE_API_URL is empty (itr#118)", async () => {
    vi.stubEnv("VITE_API_URL", "");

    await expect(import("./env")).rejects.toThrow(
      "VITE_API_URL must not be empty; omit it to use the default origin.",
    );
  });

  it("throws at module load when VITE_WS_URL is empty (itr#118)", async () => {
    vi.stubEnv("VITE_WS_URL", "");

    await expect(import("./env")).rejects.toThrow(
      "VITE_WS_URL must not be empty; omit it to use the default origin.",
    );
  });
});
