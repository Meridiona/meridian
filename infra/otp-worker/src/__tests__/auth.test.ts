//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { checkBearerAuth } from "../auth";

const ENV = { OTP_CLIENT_TOKEN: "real-client-token" };

describe("checkBearerAuth", () => {
  it("accepts the correct client token", () => {
    expect(checkBearerAuth("Bearer real-client-token", ENV)).toEqual({ ok: true });
  });

  it("rejects a missing Authorization header", () => {
    expect(checkBearerAuth(null, ENV).ok).toBe(false);
  });

  it("rejects a header with no Bearer prefix", () => {
    expect(checkBearerAuth("real-client-token", ENV).ok).toBe(false);
  });

  it("rejects the wrong token", () => {
    expect(checkBearerAuth("Bearer wrong-token", ENV).ok).toBe(false);
  });

  it("rejects an empty bearer token even against an empty configured secret", () => {
    // Guards the "both sides blank" bypass: an unconfigured OTP_CLIENT_TOKEN
    // must never accept a request just because both are empty strings.
    expect(checkBearerAuth("Bearer ", { OTP_CLIENT_TOKEN: "" }).ok).toBe(false);
    expect(checkBearerAuth(null, { OTP_CLIENT_TOKEN: "" }).ok).toBe(false);
  });
});
