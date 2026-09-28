import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { LegacyProviderImportRow } from "../../types";
import { ProviderImportRows } from "./LegacyProviderMigration";
import { providerImportReason, providerImportReasoning, providerMigrationCopy } from "./legacyProviderMigrationCopy";

// Rendering the pure row view must not initialize the desktop/browser adapter.
vi.mock("../../api/backend", () => ({}));

const ready: LegacyProviderImportRow = { sourceOrdinal: 1, label: "My_API_connection", endpoint: "https://fixture.test/v1",
  model: "fixture-model", apiFormat: "openaiResponses", reasoningEffort: "high", contextWindow: 128000,
  disposition: "import", reasonCode: null, confirmToken: "fixture-single-use-token", expiresAt: "2026-09-07T06:00:00Z" };

describe("Viewer API import presentation", () => {
  it("shows real connection settings and preserves a user label without exposing native tokens or enums", () => {
    const markup = renderToStaticMarkup(<ProviderImportRows rows={[ready]} language="zh" disabled={false} onSelect={() => undefined} />);
    for (const visible of ["My_API_connection", "https://fixture.test/v1", "fixture-model", "Responses API", "128,000", "可导入", "选择导入"]) {
      expect(markup).toContain(visible);
    }
    expect(markup).not.toContain(ready.confirmToken);
    expect(markup).not.toContain("openaiResponses");
  });

  it("does not offer import for existing, unsupported, or missing-token entries", () => {
    const rows: LegacyProviderImportRow[] = [
      { ...ready, disposition: "keep_existing", reasonCode: "legacy_provider_existing" },
      { ...ready, sourceOrdinal: 2, disposition: "review", reasonCode: "new_internal_reason" },
      { ...ready, sourceOrdinal: 3, confirmToken: null },
    ];
    const markup = renderToStaticMarkup(<ProviderImportRows rows={rows} language="zh" disabled={false} onSelect={() => undefined} />);
    expect(markup).toContain("保留现有连接");
    expect(markup).toContain("需要复核");
    expect(markup).not.toContain("选择导入");
    expect(markup).not.toContain("legacy_provider_existing");
    expect(markup).not.toContain("new_internal_reason");
  });

  it("explains review reasons in both languages, with a safe future fallback", () => {
    for (const code of ["legacy_provider_auth_unsupported", "legacy_provider_auth_invalid", "legacy_provider_mixed_auth",
      "legacy_provider_key_missing", "legacy_provider_config_missing", "legacy_provider_config_invalid", "legacy_provider_model_missing",
      "legacy_provider_endpoint_missing", "legacy_provider_endpoint_invalid", "legacy_provider_custom_auth_review",
      "legacy_provider_transport_review", "legacy_provider_reasoning_review", "legacy_provider_context_invalid",
      "legacy_provider_name_invalid", "legacy_provider_profile_invalid", "legacy_provider_preview_limit", "future_unknown_status"]) {
      for (const language of ["zh", "en"] as const) {
        expect(providerImportReason(code, language)).not.toContain("_");
        expect(providerImportReason(code, language)?.length).toBeGreaterThan(6);
      }
    }
    expect(providerImportReason(null, "zh")).toBeNull();
    expect(providerImportReasoning("future_internal", "zh")).toBe("需要复核");
    expect(providerMigrationCopy.zh.description).toContain("不试连");
    expect(providerMigrationCopy.zh.rollbackHint).toContain("正在使用");
  });
});
