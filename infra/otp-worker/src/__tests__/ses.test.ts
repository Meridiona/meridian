//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it, vi } from "vitest";
import { buildOtpEmailBody, extractSesErrorCode, sendOtpEmail, type AwsFetcher } from "../ses";

const ENV = {
  AWS_ACCESS_KEY_ID: "AKIA_FAKE",
  AWS_SECRET_ACCESS_KEY: "fake-secret",
  AWS_REGION: "us-east-1",
  FROM_ADDRESS: "otp@auth.meridiona.com",
  FROM_NAME: "Meridian",
};

describe("buildOtpEmailBody", () => {
  it("includes the code and the TTL, and no links", () => {
    const body = buildOtpEmailBody("123456", 10);
    expect(body).toContain("123456");
    expect(body).toContain("10 minutes");
    expect(body).not.toMatch(/https?:\/\//);
  });
});

describe("extractSesErrorCode", () => {
  it("extracts an AWS-shaped Error.Code", () => {
    expect(extractSesErrorCode(JSON.stringify({ Error: { Code: "MessageRejected" } }))).toBe("MessageRejected");
  });

  it("falls back to a top-level message field", () => {
    expect(extractSesErrorCode(JSON.stringify({ message: "Some SES error" }))).toBe("Some SES error");
  });

  it("never echoes an arbitrary raw body back — unparseable text yields a fixed placeholder", () => {
    // This is the security-relevant case: SES sandbox errors echo the
    // destination email address in the raw text. This must not leak through.
    const result = extractSesErrorCode("plain text mentioning victim@example.com is not verified");
    expect(result).toBe("unparseable_ses_error_response");
    expect(result).not.toContain("victim@example.com");
  });

  it("returns a fixed placeholder for a JSON body with neither known shape", () => {
    expect(extractSesErrorCode(JSON.stringify({ somethingElse: true }))).toBe("unknown_ses_error_shape");
  });
});

describe("sendOtpEmail", () => {
  it("returns true on a 2xx response and posts to the region-specific SES endpoint", async () => {
    const fakeClient: AwsFetcher = { fetch: vi.fn(async () => new Response("{}", { status: 200 })) };
    const result = await sendOtpEmail("user@example.com", "123456", 10, ENV, fakeClient);
    expect(result).toBe(true);
    expect(fakeClient.fetch).toHaveBeenCalledWith(
      "https://email.us-east-1.amazonaws.com/",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("returns false on a non-2xx response without throwing", async () => {
    const fakeClient: AwsFetcher = {
      fetch: vi.fn(async () => new Response(JSON.stringify({ Error: { Code: "Throttling" } }), { status: 429 })),
    };
    const result = await sendOtpEmail("user@example.com", "123456", 10, ENV, fakeClient);
    expect(result).toBe(false);
  });

  it("returns false (never throws) when the underlying fetch rejects", async () => {
    const fakeClient: AwsFetcher = {
      fetch: vi.fn(async () => {
        throw new Error("network down");
      }),
    };
    const result = await sendOtpEmail("user@example.com", "123456", 10, ENV, fakeClient);
    expect(result).toBe(false);
  });

  it("never includes the code in the visible request URL (it belongs in the signed body only)", async () => {
    const fakeClient: AwsFetcher = { fetch: vi.fn(async () => new Response("{}", { status: 200 })) };
    await sendOtpEmail("user@example.com", "999999", 10, ENV, fakeClient);
    const [url] = (fakeClient.fetch as ReturnType<typeof vi.fn>).mock.calls[0] as [string, RequestInit];
    expect(url).not.toContain("999999");
  });
});
