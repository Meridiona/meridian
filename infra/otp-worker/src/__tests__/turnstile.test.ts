//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it, vi } from "vitest";
import { verifyTurnstileToken } from "../turnstile";

function fakeFetch(response: Partial<{ ok: boolean; status: number; json: () => unknown }>): typeof fetch {
  return vi.fn(async () => ({
    ok: response.ok ?? true,
    status: response.status ?? 200,
    json: response.json ?? (async () => ({ success: true })),
  })) as unknown as typeof fetch;
}

describe("verifyTurnstileToken", () => {
  it("skips verification (returns true) when no secret is configured — gate 2", async () => {
    const fetchImpl = fakeFetch({});
    const result = await verifyTurnstileToken("some-token", undefined, "1.2.3.4", fetchImpl);
    expect(result).toBe(true);
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("skips verification when the secret is an empty string", async () => {
    const fetchImpl = fakeFetch({});
    const result = await verifyTurnstileToken("some-token", "", "1.2.3.4", fetchImpl);
    expect(result).toBe(true);
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("returns true when siteverify reports success", async () => {
    const fetchImpl = fakeFetch({ json: async () => ({ success: true }) });
    const result = await verifyTurnstileToken("good-token", "secret", "1.2.3.4", fetchImpl);
    expect(result).toBe(true);
  });

  it("fails closed when siteverify reports failure — a present-but-invalid token is rejected", async () => {
    const fetchImpl = fakeFetch({ json: async () => ({ success: false, "error-codes": ["invalid-input-response"] }) });
    const result = await verifyTurnstileToken("bad-token", "secret", "1.2.3.4", fetchImpl);
    expect(result).toBe(false);
  });

  it("fails closed on a non-2xx siteverify response", async () => {
    const fetchImpl = fakeFetch({ ok: false, status: 500 });
    const result = await verifyTurnstileToken("token", "secret", "1.2.3.4", fetchImpl);
    expect(result).toBe(false);
  });

  it("fails closed when the fetch itself throws (network error)", async () => {
    const throwingFetch = vi.fn(async () => {
      throw new Error("network down");
    }) as unknown as typeof fetch;
    const result = await verifyTurnstileToken("token", "secret", "1.2.3.4", throwingFetch);
    expect(result).toBe(false);
  });
});
