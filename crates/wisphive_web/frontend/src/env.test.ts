import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

describe("frontend environment (itr#118)", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("throws at module load when VITE_WS_URL is empty (itr#118)", async () => {
    vi.stubEnv("VITE_WS_URL", "");

    await expect(import("./env")).rejects.toThrow(
      "VITE_WS_URL must not be empty; omit it to use the default origin.",
    );
  });
});
