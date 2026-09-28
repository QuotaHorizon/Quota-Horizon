//! A small, explicitly dated editorial archive bundled with the application.
//! This is not a live feed and has no networking, disk writes, or account input.

use capacity_domain::public_reset::{
    PublicEventKind, PublicEvidenceDisposition, PublicEvidenceReview, PublicEvidenceScope,
    PublicEvidenceSource, PublicResetLedger, PublicResetSignal, PublicSignalSemantics,
    PublicSourceClass,
};
use capacity_domain::UtcTimestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::public_reset_sources::canonical_public_url;

const ARCHIVE: &str = include_str!("../resources/public-reset-archive.json");
const DISCOVERY_URLS: &[&str] = &[
    "https://quotaresets.com/api/v1/events.json",
    "https://codex-reset.com/api/timeline",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicResetCopy {
    en: String,
    zh: String,
}

impl PublicResetCopy {
    fn is_valid(&self, max: usize) -> bool {
        [&self.en, &self.zh].into_iter().all(|text| {
            !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveFile {
    schema_version: u32,
    reviewed_at: UtcTimestamp,
    entries: Vec<ArchiveRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveRecord {
    id: String,
    family_id: String,
    kind: PublicEventKind,
    scope: PublicEvidenceScope,
    semantics: PublicSignalSemantics,
    source_class: PublicSourceClass,
    review: PublicEvidenceReview,
    source_url: String,
    author: String,
    published_at: Option<UtcTimestamp>,
    discovered_via: Vec<String>,
    title: PublicResetCopy,
    summary: PublicResetCopy,
    interpretation: PublicResetCopy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicResetArchiveEntry {
    signal: PublicResetSignal,
    disposition: PublicEvidenceDisposition,
    title: PublicResetCopy,
    summary: PublicResetCopy,
    interpretation: PublicResetCopy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicResetArchive {
    schema_version: u32,
    mode: &'static str,
    reviewed_at: UtcTimestamp,
    entries: Vec<PublicResetArchiveEntry>,
    evidence_family_count: usize,
    history_complete: bool,
    forecast_status: &'static str,
}

fn load_archive(source: &str) -> Result<PublicResetArchive, &'static str> {
    if source.len() > 256 * 1024 {
        return Err("public_archive_too_large");
    }
    let file: ArchiveFile = serde_json::from_str(source).map_err(|_| "public_archive_invalid")?;
    if file.schema_version != 1 || file.entries.len() > 128 {
        return Err("public_archive_unsupported");
    }
    let mut ledger = PublicResetLedger::default();
    let mut entries = Vec::new();
    for record in file.entries {
        let (canonical, class) = canonical_public_url(&record.source_url)
            .map_err(|_| "public_archive_invalid_source")?;
        if canonical != record.source_url
            || (class != record.source_class
                && !(record.source_class == PublicSourceClass::ProviderUiObservation
                    && class == PublicSourceClass::IndependentTracker))
            || record
                .discovered_via
                .iter()
                .any(|url| !DISCOVERY_URLS.contains(&url.as_str()))
            || !record.title.is_valid(256)
            || !record.summary.is_valid(4096)
            || !record.interpretation.is_valid(4096)
        {
            return Err("public_archive_invalid_source");
        }
        let bytes = serde_json::to_vec(&record).map_err(|_| "public_archive_invalid")?;
        let signal = PublicResetSignal {
            signal_id: record.id,
            revision: 1,
            evidence_family_id: record.family_id,
            // The archive has no exact, reviewed occurrence times. It must not
            // accidentally become the frozen historical set for calibration.
            event_id: None,
            occurred_at: None,
            kind: record.kind,
            scope: record.scope,
            semantics: record.semantics,
            source: PublicEvidenceSource {
                canonical_url: canonical,
                author: record.author,
                source_class: record.source_class,
                review: record.review,
                published_at: record.published_at,
                collected_at: file.reviewed_at.clone(),
                content_sha256: format!("{:x}", Sha256::digest(&bytes)),
                parser_version: "bundled-review-v1".into(),
                discovered_via: record.discovered_via,
            },
            recorded_at: file.reviewed_at.clone(),
            title: record.title.en.clone(),
            summary: record.summary.en.clone(),
            announcement_timing: None,
        };
        if !ledger
            .append(signal.clone())
            .map_err(|_| "public_archive_invalid_record")?
        {
            return Err("public_archive_duplicate_record");
        }
        entries.push(PublicResetArchiveEntry {
            disposition: signal.disposition(),
            signal,
            title: record.title,
            summary: record.summary,
            interpretation: record.interpretation,
        });
    }
    Ok(PublicResetArchive {
        schema_version: 1,
        mode: "bundled_archive",
        forecast_status: "abstained",
        history_complete: false,
        evidence_family_count: ledger.evidence_family_count_at(&file.reviewed_at),
        reviewed_at: file.reviewed_at,
        entries,
    })
}

#[tauri::command]
pub(crate) fn get_public_reset_archive() -> Result<PublicResetArchive, String> {
    // The same dated archive is returned on every call. Reopening the page or
    // loading it again never advances its review time or implies a source check.
    load_archive(ARCHIVE).map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_review_confirms_grant_programme_not_full_reset_or_personal_receipt() {
        let archive = load_archive(ARCHIVE).unwrap();
        assert_eq!(archive.evidence_family_count, 3);
        assert_eq!(archive.forecast_status, "abstained");
        assert!(!archive.history_complete);
        let confirmed: Vec<_> = archive
            .entries
            .iter()
            .filter(|entry| entry.disposition == PublicEvidenceDisposition::Confirmed)
            .collect();
        assert_eq!(confirmed.len(), 1);
        assert_eq!(
            confirmed[0].signal.kind,
            PublicEventKind::GlobalBankedResetGrant
        );
        assert!(archive
            .entries
            .iter()
            .all(|entry| entry.signal.event_id.is_none() && entry.signal.occurred_at.is_none()));
    }

    #[test]
    fn reloading_is_not_a_live_refresh_and_does_not_change_provenance() {
        let first = get_public_reset_archive().unwrap();
        let second = get_public_reset_archive().unwrap();
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(second).unwrap()
        );
    }

    #[test]
    fn malformed_archive_and_invalid_public_source_fail_closed() {
        assert!(load_archive("{}").is_err());
        assert!(
            load_archive(&ARCHIVE.replace("https://help.openai.com/", "https://localhost/"))
                .is_err()
        );
        assert!(
            load_archive(&ARCHIVE.replace("\"schemaVersion\": 1", "\"schemaVersion\": 99"))
                .is_err()
        );
    }
}
