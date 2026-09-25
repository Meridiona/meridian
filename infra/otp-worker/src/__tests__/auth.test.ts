//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { checkBearerAuth } from "../auth";

const PROD_ENV = { OTP_CLIENT_TOKEN: "real-client-token", ENVIRONMENT: "production" };
const STAGING_ENV = {
  OTP_CLIENT_TOKEN: "real-client-token",
  CI_TEST_TOKEN: "real-ci-token",
  ENVIRONMENT: "staging",
};

describe("checkBearerAuth", () => {
  it("accepts the correct client token", () => {
    const result = checkBearerAuth("Bearer real-client-token", PROD_ENV);
    expect(result).toEqual({ ok: true, isCiTestToken: false });
  });

  it("rejects a missing Authorization header", () => {
    expect(checkBearerAuth(null, PROD_ENV).ok).toBe(false);
  });

  it("rejects a header with no Bearer prefix", () => {
    expect(checkBearerAuth("real-client-token", PROD_ENV).ok).toBe(false);
  });

  it("rejects the wrong token", () => {
    expect(checkBearerAuth("Bearer wrong-token", PROD_ENV).ok).toBe(false);
  });

  it("rejects an empty bearer token even against an empty configured secret", () => {
    // Guards the "both sides blank" bypass: an unconfigured OTP_CLIENT_TOKEN
    // must never accept a request just because both are empty strings.
    expect(checkBearerAuth("Bearer ", { OTP_CLIENT_TOKEN: "", ENVIRONMENT: "production" }).ok).toBe(false);
    expect(checkBearerAuth(null, { OTP_CLIENT_TOKEN: "", ENVIRONMENT: "production" }).ok).toBe(false);
  });

  it("never accepts the CI test token on a non-staging environment, even if the secret is present", () => {
    const result = checkBearerAuth("Bearer real-ci-token", {
      OTP_CLIENT_TOKEN: "real-client-token",
      CI_TEST_TOKEN: "real-ci-token",
      ENVIRONMENT: "production",
    });
    expect(result.ok).toBe(false);
  });

  it("accepts the CI test token on staging and flags isCiTestToken", () => {
    const result = checkBearerAuth("Bearer real-ci-token", STAGING_ENV);
    expect(result).toEqual({ ok: true, isCiTestToken: true });
  });

  it("accepts the normal client token on staging without flagging isCiTestToken", () => {
    const result = checkBearerAuth("Bearer real-client-token", STAGING_ENV);
    expect(result).toEqual({ ok: true, isCiTestToken: false });
  });

  it("rejects a wrong token on staging even when a CI token is configured", () => {
    expect(checkBearerAuth("Bearer neither-token", STAGING_ENV).ok).toBe(false);
  });
});
