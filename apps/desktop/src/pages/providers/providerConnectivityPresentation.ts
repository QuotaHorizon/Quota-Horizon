import type { Translate, TranslationKey } from "../../i18n";
import type {
  ProviderConnectivityIssue,
  ProviderConnectivityIssueCode,
} from "../../types";

const ISSUE_TRANSLATIONS: Record<ProviderConnectivityIssueCode, TranslationKey> = {
  invalid_base_url: "providers.connectivity.issue.invalidBaseUrl",
  local_proxy_url: "providers.connectivity.issue.localProxyUrl",
  credential_required: "providers.connectivity.issue.credentialRequired",
  credential_endpoint_mismatch: "providers.connectivity.issue.credentialEndpointMismatch",
  credential_unavailable: "providers.connectivity.issue.credentialUnavailable",
  timeout: "providers.connectivity.issue.timeout",
  network: "providers.connectivity.issue.network",
  unauthorized: "providers.connectivity.issue.unauthorized",
  forbidden: "providers.connectivity.issue.forbidden",
  not_found: "providers.connectivity.issue.notFound",
  rate_limited: "providers.connectivity.issue.rateLimited",
  upstream_server: "providers.connectivity.issue.upstreamServer",
  redirect_rejected: "providers.connectivity.issue.redirectRejected",
  response_too_large: "providers.connectivity.issue.responseTooLarge",
  invalid_response: "providers.connectivity.issue.invalidResponse",
  empty_model_catalog: "providers.connectivity.issue.emptyModelCatalog",
  request_failed: "providers.connectivity.issue.requestFailed",
  provider_unavailable: "providers.connectivity.issue.providerUnavailable",
  http_error: "providers.connectivity.issue.httpError",
};

export const PROVIDER_REQUEST_FAILED_ISSUE: ProviderConnectivityIssue = {
  code: "request_failed",
  retryable: true,
  httpStatus: null,
};

export function providerConnectivityTranslationKey(code: ProviderConnectivityIssueCode) {
  return ISSUE_TRANSLATIONS[code];
}

export function providerConnectivityMessage(issue: ProviderConnectivityIssue, t: Translate) {
  return t(providerConnectivityTranslationKey(issue.code), {
    status: issue.httpStatus ?? "—",
  });
}

export function providerOnboardingFallbackMessage(
  t: Translate,
  issue: ProviderConnectivityIssue = PROVIDER_REQUEST_FAILED_ISSUE,
) {
  return `${providerConnectivityMessage(issue, t)} ${t("providers.connectivity.manualFallback")}`;
}
