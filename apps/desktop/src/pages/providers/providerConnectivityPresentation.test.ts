import { describe, expect, it } from "vitest";
import type { ProviderConnectivityIssueCode } from "../../types";
import type { Translate } from "../../i18n";
import {
  providerConnectivityTranslationKey,
  providerOnboardingFallbackMessage,
} from "./providerConnectivityPresentation";

describe("provider connectivity presentation", () => {
  it("maps every backend issue code to stable localized copy", () => {
    const codes: ProviderConnectivityIssueCode[] = [
      "invalid_base_url",
      "local_proxy_url",
      "credential_required",
      "credential_endpoint_mismatch",
      "credential_unavailable",
      "timeout",
      "network",
      "unauthorized",
      "forbidden",
      "not_found",
      "rate_limited",
      "upstream_server",
      "redirect_rejected",
      "response_too_large",
      "invalid_response",
      "empty_model_catalog",
      "request_failed",
      "provider_unavailable",
      "http_error",
    ];

    expect(new Set(codes.map(providerConnectivityTranslationKey)).size).toBe(codes.length);
    expect(providerConnectivityTranslationKey("unauthorized"))
      .toBe("providers.connectivity.issue.unauthorized");
    expect(providerConnectivityTranslationKey("redirect_rejected"))
      .toBe("providers.connectivity.issue.redirectRejected");
  });

  it("uses localized stable copy for unexpected loader failures", () => {
    const t: Translate = (key) => key;
    expect(providerOnboardingFallbackMessage(t)).toBe(
      "providers.connectivity.issue.requestFailed providers.connectivity.manualFallback",
    );
  });
});
