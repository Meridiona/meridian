//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it, vi } from "vitest";
import {
  buildAccountEventBody,
  buildAccountEventSubject,
  resolveAccountEvent,
  sendAccountEventEmail,
  type Fetcher,
} from "../resend";

const ENV = {
  RESEND_API_KEY: "re_fake_key",
  NOTIFY_FROM: "Meridian Sign-ins <notify@meridiona.com>",
  NOTIFY_EMAIL: "adithya@meridiona.com",
};

/** Decode the JSON body a `sendAccountEventEmail` call posted. */
function postedBody(fetcher: ReturnType<typeof vi.fn>) {
  const [, init] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
  return JSON.parse(String(init.body)) as {
    from: string;
    to: string[];
    subject: string;
    text: string;
  };
}

describe("resolveAccountEvent", () => {
  it("is a sign-up when there is no previous email", () => {
    expect(resolveAccountEvent("new@example.com", null)).toEqual({
      kind: "signed_up",
      email: "new@example.com",
    });
  });

  it("is an email change when the previous email differs", () => {
    expect(resolveAccountEvent("new@example.com", "old@example.com")).toEqual({
      kind: "email_changed",
      from: "old@example.com",
      to: "new@example.com",
    });
  });

  it("is null (nothing to notify) when the previous email is the SAME as the new one", () => {
    // "Change email" re-entering the address already on file must not send a
    // notification claiming something changed.
    expect(resolveAccountEvent("same@example.com", "same@example.com")).toBeNull();
  });
});

describe("buildAccountEventSubject", () => {
  /** The marketing site has sent `New sign-up: <email>` since June; both
   *  sources must thread together in the same inbox. */
  it("matches the website's existing sign-up convention exactly", () => {
    expect(buildAccountEventSubject({ kind: "signed_up", email: "a@b.com" })).toBe("New sign-up: a@b.com");
  });

  it("gives an email change its own prefix carrying both addresses", () => {
    expect(
      buildAccountEventSubject({ kind: "email_changed", from: "old@b.com", to: "new@b.com" }),
    ).toBe("Email changed: old@b.com -> new@b.com");
  });
});

describe("buildAccountEventBody", () => {
  it("puts the address on line 1 and names the source, mirroring the web format", () => {
    const lines = buildAccountEventBody({ kind: "signed_up", email: "a@b.com" }).trim().split("\n");
    expect(lines[0]).toBe("a@b.com");
    expect(lines[1]).toContain("desktop app");
    expect(lines[2]).toBe("First time signing in.");
  });

  it("leads an email change with the NEW address and names the old one", () => {
    const lines = buildAccountEventBody({
      kind: "email_changed",
      from: "old@b.com",
      to: "new@b.com",
    })
      .trim()
      .split("\n");
    expect(lines[0]).toBe("new@b.com");
    expect(lines[2]).toBe("Changed from old@b.com.");
  });

  it("carries no links", () => {
    expect(buildAccountEventBody({ kind: "signed_up", email: "a@b.com" })).not.toMatch(/https?:\/\//);
  });
});

describe("sendAccountEventEmail", () => {
  it("posts to Resend with bearer auth and returns true on 2xx", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({ id: "x" }), { status: 200 }));
    const result = await sendAccountEventEmail(
      { kind: "signed_up", email: "a@b.com" },
      ENV,
      fetcher as unknown as Fetcher,
    );
    expect(result).toBe(true);
    const [url, init] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("https://api.resend.com/emails");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer re_fake_key");
  });

  it("sends to NOTIFY_EMAIL from NOTIFY_FROM, never to the account's own address", async () => {
    const fetcher = vi.fn(async () => new Response("{}", { status: 200 }));
    await sendAccountEventEmail(
      { kind: "signed_up", email: "auser@example.com" },
      ENV,
      fetcher as unknown as Fetcher,
    );
    const body = postedBody(fetcher);
    expect(body.to).toEqual(["adithya@meridiona.com"]);
    expect(body.from).toBe("Meridian Sign-ins <notify@meridiona.com>");
    // The signing-up user is the SUBJECT of the mail, never a recipient of it.
    expect(body.to).not.toContain("auser@example.com");
  });

  it("sends text only - no HTML part, matching the website's notification", async () => {
    const fetcher = vi.fn(async () => new Response("{}", { status: 200 }));
    await sendAccountEventEmail(
      { kind: "signed_up", email: "a@b.com" },
      ENV,
      fetcher as unknown as Fetcher,
    );
    expect(postedBody(fetcher)).not.toHaveProperty("html");
  });

  it("returns false (never throws) on a non-2xx response", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({ message: "nope" }), { status: 422 }));
    const result = await sendAccountEventEmail(
      { kind: "signed_up", email: "a@b.com" },
      ENV,
      fetcher as unknown as Fetcher,
    );
    expect(result).toBe(false);
  });

  it("returns false (never throws) when the fetch itself rejects", async () => {
    const fetcher = vi.fn(async () => {
      throw new Error("network down");
    });
    const result = await sendAccountEventEmail(
      { kind: "signed_up", email: "a@b.com" },
      ENV,
      fetcher as unknown as Fetcher,
    );
    expect(result).toBe(false);
  });

  /** A Worker deployed without the secret must degrade to "no notification",
   *  not throw inside the `ctx.waitUntil` where nothing would surface it. */
  it("returns false without attempting a request when the API key is unset", async () => {
    const fetcher = vi.fn(async () => new Response("{}", { status: 200 }));
    const result = await sendAccountEventEmail(
      { kind: "signed_up", email: "a@b.com" },
      { ...ENV, RESEND_API_KEY: "" },
      fetcher as unknown as Fetcher,
    );
    expect(result).toBe(false);
    expect(fetcher).not.toHaveBeenCalled();
  });
});
