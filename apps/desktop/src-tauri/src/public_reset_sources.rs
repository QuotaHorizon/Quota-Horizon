//! Bounded, offline decoders for the public sources selected in the reset
//! intelligence contract. These are collector adapters, not client HTTP routes.
//! No account data or credentials enter this module; no URL is fetched here.
//!
//! Reuses CodexResetRadar's source separation and original-post identity approach,
//! plus the two trackers' documented timeline schemas. Unlike a keyword alert,
//! a tracker label or a usage-dashboard observation never proves a global reset.

use capacity_domain::public_reset::{
    PublicAnnouncementTiming, PublicEventKind, PublicEvidenceReview, PublicEvidenceScope,
    PublicEvidenceSource, PublicResetSignal, PublicSignalSemantics, PublicSourceClass,
};
use capacity_domain::UtcTimestamp;
use chrono::{DateTime, SecondsFormat};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 2048;
const PARSER_VERSION: &str = "public-candidate-v5";
const QUOTA_RESETS: &str = "https://quotaresets.com/api/v1/events.json";
const CODEX_RESET: &str = "https://codex-reset.com/api/timeline";
const CODEX_RESET_POSTS: &str = "https://codex-reset.com/api/feed";
const OPENAI_STATUS: &str = "https://status.openai.com/api/v2/incidents.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicTimelineSource {
    QuotaResets,
    CodexReset,
    CodexResetPosts,
    OpenAiStatus,
}

impl PublicTimelineSource {
    pub(crate) const ALL: [Self; 4] = [
        Self::QuotaResets,
        Self::CodexReset,
        Self::CodexResetPosts,
        Self::OpenAiStatus,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::QuotaResets => "quotaresets",
            Self::CodexReset => "codex_reset",
            Self::CodexResetPosts => "codex_reset_posts",
            Self::OpenAiStatus => "openai_status",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.id() == id)
    }

    pub(crate) fn url(self) -> &'static str {
        match self {
            Self::QuotaResets => QUOTA_RESETS,
            Self::CodexReset => CODEX_RESET,
            Self::CodexResetPosts => CODEX_RESET_POSTS,
            Self::OpenAiStatus => OPENAI_STATUS,
        }
    }

    fn array_key(self) -> &'static str {
        match self {
            Self::QuotaResets => "data",
            Self::CodexReset => "events",
            Self::CodexResetPosts => "tweets",
            Self::OpenAiStatus => "incidents",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicCandidateBatch {
    pub source_url: &'static str,
    pub collected_at: UtcTimestamp,
    pub candidates: Vec<PublicResetSignal>,
    pub rejected_records: usize,
    pub skipped_records: usize,
    pub posts: Option<crate::public_reset_feed::insights::PostFeed>,
}

/// Schema drift is an error, not a successful empty timeline. Unrelated
/// providers are skipped, while malformed in-scope rows remain counted failures.
pub(crate) fn decode_public_timeline(
    source: PublicTimelineSource,
    bytes: &[u8],
    collected_at: UtcTimestamp,
) -> Result<PublicCandidateBatch, &'static str> {
    if bytes.len() > MAX_BODY_BYTES {
        return Err("public_source_too_large");
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "public_source_invalid_json")?;
    if source == PublicTimelineSource::CodexResetPosts {
        if value.get("version").and_then(Value::as_u64) != Some(1)
            || value.pointer("/profile/handle").and_then(Value::as_str) != Some("thsottiaux")
            || value.get("stale").and_then(Value::as_bool).is_none()
        {
            return Err("public_source_schema_changed");
        }
        if value.get("stale").and_then(Value::as_bool) == Some(true) {
            return Err("public_source_upstream_stale");
        }
        let fetched = optional_time(&value, "fetched_at")
            .map_err(|_| "public_source_schema_changed")?
            .ok_or("public_source_schema_changed")?;
        let fetched = DateTime::parse_from_rfc3339(fetched.as_str())
            .map_err(|_| "public_source_schema_changed")?;
        let observed = DateTime::parse_from_rfc3339(collected_at.as_str())
            .map_err(|_| "public_source_schema_changed")?;
        let age = observed.signed_duration_since(fetched).num_seconds();
        if !(-60..=1800).contains(&age) {
            return Err("public_source_upstream_stale");
        }
    }
    let rows = value
        .get(source.array_key())
        .and_then(Value::as_array)
        .ok_or("public_source_schema_changed")?;
    if rows.len() > MAX_ITEMS {
        return Err("public_source_too_many_records");
    }
    let mut result = PublicCandidateBatch {
        source_url: source.url(),
        collected_at: collected_at.clone(),
        candidates: Vec::new(),
        rejected_records: 0,
        skipped_records: 0,
        posts: if source == PublicTimelineSource::CodexResetPosts {
            crate::public_reset_feed::insights::decode_posts(&value, &collected_at)
        } else {
            None
        },
    };
    for row in rows {
        let parsed = match source {
            PublicTimelineSource::QuotaResets => quota_resets_candidate(row, &result.collected_at),
            PublicTimelineSource::CodexReset => codex_reset_candidate(row, &result.collected_at),
            PublicTimelineSource::CodexResetPosts => codex_reset_post(row, &result.collected_at),
            PublicTimelineSource::OpenAiStatus => status_candidate(row, &result.collected_at),
        };
        match parsed {
            Ok(Some(candidate)) if candidate.validate().is_ok() => {
                result.candidates.push(candidate)
            }
            Ok(None) => result.skipped_records += 1,
            _ => result.rejected_records += 1,
        }
    }
    Ok(result)
}

fn field<'a>(row: &'a Value, name: &str, max: usize) -> Result<&'a str, ()> {
    let value = row.get(name).and_then(Value::as_str).ok_or(())?;
    if value.trim().is_empty()
        || value.len() > max
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(());
    }
    Ok(value)
}

fn optional_text<'a>(row: &'a Value, name: &str, max: usize) -> Result<Option<&'a str>, ()> {
    match row.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => field(row, name, max).map(Some),
    }
}

fn optional_time(row: &Value, name: &str) -> Result<Option<UtcTimestamp>, ()> {
    let Some(value) = optional_text(row, name, 64)? else {
        return Ok(None);
    };
    // Day-resolution timestamps carry no exact event/publication time. Do not
    // silently invent midnight; they remain in the content fingerprint.
    if value.len() == 10 && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() {
        return Ok(None);
    }
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| ())?;
    UtcTimestamp::parse(
        parsed
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
    .map(Some)
    .map_err(|_| ())
}

fn stable_id(namespace: &str, value: &str) -> String {
    format!("{namespace}:{:x}", Sha256::digest(value.as_bytes()))
}

/// The collector only accepts reviewed source domains and public item paths.
/// Dashboard observations stay on their tracker page, never on a logged-in URL.
/// Strip tracking parameters before family identity; do not trust author labels.
pub(crate) fn canonical_public_url(value: &str) -> Result<(String, PublicSourceClass), ()> {
    if value.len() > 2048 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(());
    }
    let mut url = Url::parse(value).map_err(|_| ())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(());
    }
    let host = url.host_str().ok_or(())?.to_ascii_lowercase();
    let class = match host.as_str() {
        "x.com" | "twitter.com" | "www.x.com" | "www.twitter.com" => {
            let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
            if parts.len() != 3
                || parts[1] != "status"
                || parts[2].is_empty()
                || parts[2].len() > 24
                || !parts[2].bytes().all(|b| b.is_ascii_digit())
            {
                return Err(());
            }
            let handle = parts[0].to_ascii_lowercase();
            if handle.is_empty()
                || handle.len() > 32
                || !handle
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(());
            }
            let path = format!("/{handle}/status/{}", parts[2]);
            url.set_host(Some("x.com")).map_err(|_| ())?;
            url.set_path(&path);
            if matches!(
                handle.as_str(),
                "thsottiaux" | "openai" | "openaidevs" | "romainhuet"
            ) {
                PublicSourceClass::OfficialSocial
            } else {
                PublicSourceClass::IndependentTracker
            }
        }
        "help.openai.com" if url.path().starts_with("/en/articles/") => {
            PublicSourceClass::OfficialDocumentation
        }
        "openai.com" if url.path().starts_with("/index/") => {
            PublicSourceClass::OfficialAnnouncement
        }
        "status.openai.com" if url.path().starts_with("/incidents/") => {
            PublicSourceClass::OfficialStatus
        }
        "quotaresets.com" if url.path().starts_with("/events/") => {
            PublicSourceClass::IndependentTracker
        }
        "codex-reset.com" if !url.path().starts_with("/api/") => {
            PublicSourceClass::IndependentTracker
        }
        _ => return Err(()),
    };
    url.set_query(None);
    url.set_fragment(None);
    Ok((url.to_string(), class))
}

fn family_id(canonical_url: &str) -> String {
    if let Ok(url) = Url::parse(canonical_url) {
        if url.host_str() == Some("x.com") {
            if let Some(id) = url.path_segments().and_then(|mut parts| parts.next_back()) {
                return format!("x:{id}");
            }
        }
    }
    stable_id("public-url", canonical_url)
}

fn source_author(canonical_url: &str, class: PublicSourceClass) -> String {
    if let Ok(url) = Url::parse(canonical_url) {
        if url.host_str() == Some("x.com") {
            if let Some(handle) = url.path_segments().and_then(|mut parts| parts.next()) {
                return format!("@{handle}");
            }
        }
    }
    match class {
        PublicSourceClass::OfficialAnnouncement | PublicSourceClass::OfficialDocumentation => {
            "OpenAI"
        }
        PublicSourceClass::OfficialStatus => "OpenAI Status",
        PublicSourceClass::ProviderUiObservation => "QuotaResets observation",
        _ => "Independent tracker",
    }
    .into()
}

struct CandidateInput<'a> {
    source_url: &'static str,
    external_id: &'a str,
    event_id: Option<&'a str>,
    kind: PublicEventKind,
    semantics: PublicSignalSemantics,
    canonical_url: String,
    source_class: PublicSourceClass,
    published_at: Option<UtcTimestamp>,
    occurred_at: Option<UtcTimestamp>,
    title: &'a str,
    summary: &'a str,
}

fn candidate(
    input: CandidateInput<'_>,
    raw: &Value,
    collected: &UtcTimestamp,
) -> Result<PublicResetSignal, ()> {
    let normalized_record = serde_json::to_vec(raw).map_err(|_| ())?;
    let family = family_id(&input.canonical_url);
    let event_id = input.event_id.map(|id| {
        stable_id(
            if input.kind == PublicEventKind::GlobalBankedResetGrant {
                "banked-candidate"
            } else {
                "reset-candidate"
            },
            id,
        )
    });
    Ok(PublicResetSignal {
        signal_id: stable_id(
            "candidate",
            &format!("{}:{}", input.source_url, input.external_id),
        ),
        revision: 1,
        evidence_family_id: family,
        event_id,
        kind: input.kind,
        // The collector has not reviewed the primary scope, even if a tracker
        // says "all users". Review is a separate, revisioned operation.
        scope: if input.source_class == PublicSourceClass::ProviderUiObservation {
            PublicEvidenceScope::Personal
        } else {
            PublicEvidenceScope::Unknown
        },
        semantics: input.semantics,
        source: PublicEvidenceSource {
            author: source_author(&input.canonical_url, input.source_class),
            canonical_url: input.canonical_url,
            source_class: input.source_class,
            review: PublicEvidenceReview::Indirect,
            published_at: input.published_at,
            collected_at: collected.clone(),
            content_sha256: format!("{:x}", Sha256::digest(&normalized_record)),
            parser_version: PARSER_VERSION.into(),
            discovered_via: vec![input.source_url.into()],
        },
        recorded_at: collected.clone(),
        occurred_at: input.occurred_at,
        title: input.title.into(),
        summary: input.summary.into(),
        announcement_timing: None,
    })
}

fn quota_resets_candidate(
    row: &Value,
    collected: &UtcTimestamp,
) -> Result<Option<PublicResetSignal>, ()> {
    if field(row, "provider", 64)? != "openai" {
        return Ok(None);
    }
    let id = field(row, "slug", 192)?;
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(());
    }
    let source = row.get("source").filter(|v| v.is_object()).ok_or(())?;
    let observed = optional_text(source, "kind", 64)? == Some("provider_ui");
    let (canonical, class) = if observed {
        // Never follow, persist, or advertise the private usage-dashboard link
        // as an official broad announcement. Keep the public tracker reference.
        (
            format!("https://quotaresets.com/events/{id}/"),
            PublicSourceClass::ProviderUiObservation,
        )
    } else {
        canonical_public_url(field(source, "url", 2048)?)?
    };
    let kind = match field(row, "type", 64)? {
        "hard_reset" if field(row, "state", 64)? == "confirmed" => PublicEventKind::GlobalFullReset,
        "banked_reset" => PublicEventKind::GlobalBankedResetGrant,
        _ => PublicEventKind::Unclassified,
    };
    let semantics = match field(row, "state", 64)? {
        "confirmed" => PublicSignalSemantics::ConfirmedReset,
        "likely" => PublicSignalSemantics::PossibleSignal,
        "retracted" => PublicSignalSemantics::Retracted,
        "corrected" => PublicSignalSemantics::Corrected,
        _ => PublicSignalSemantics::ContextOnly,
    };
    let mut signal = candidate(
        CandidateInput {
            source_url: QUOTA_RESETS,
            external_id: id,
            event_id: Some(optional_text(row, "resetId", 192)?.unwrap_or(id)),
            kind,
            semantics,
            canonical_url: canonical,
            source_class: class,
            published_at: optional_time(source, "publishedAt")?,
            occurred_at: optional_time(row, "confirmedAt")?,
            title: field(row, "title", 256)?,
            summary: field(row, "summary", 4096)?,
        },
        row,
        collected,
    )?;
    if let Some(expected_on) = optional_text(row, "expectedOn", 10)? {
        let timing = row.get("timingSource").ok_or(())?;
        let timing_url = canonical_public_url(field(timing, "url", 2048)?)?.0;
        if timing_url != signal.source.canonical_url {
            // A valid occurrence row can reference another post for timing.
            // Keep the row, but do not lend that interpretation to this family.
            return Ok(Some(signal));
        }
        signal.announcement_timing = Some(PublicAnnouncementTiming {
            expected_on: expected_on.into(),
            time_zone: field(timing, "timeZone", 64)?.into(),
            source_url: QUOTA_RESETS.into(),
        });
    }
    Ok(Some(signal))
}

fn codex_reset_post(
    row: &Value,
    collected: &UtcTimestamp,
) -> Result<Option<PublicResetSignal>, ()> {
    let id = field(row, "id", 192)?;
    if !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    let (canonical, class) = canonical_public_url(field(row, "url", 2048)?)?;
    if canonical != format!("https://x.com/thsottiaux/status/{id}") {
        return Err(());
    }
    let kind_text = field(row, "kind", 64)?;
    let kind = match kind_text {
        "candidate" | "reset"
            if row.get("explicit_reset_claim").and_then(Value::as_bool) == Some(true) =>
        {
            PublicEventKind::GlobalFullReset
        }
        "candidate" | "reset" | "signal" | "codex" => PublicEventKind::Unclassified,
        "banked" => PublicEventKind::GlobalBankedResetGrant,
        _ => return Ok(None),
    };
    let text = field(row, "text", 4096)?;
    let published_at = optional_time(row, "at")?.ok_or(())?;
    // Likes, repost counts and fetch timestamps must not create new evidence
    // revisions. Preserve substantive text, identity and source classifications.
    let normalized = serde_json::json!({ "id": id, "url": canonical, "text": text,
        "at": published_at, "kind": kind_text });
    candidate(
        CandidateInput {
            source_url: CODEX_RESET_POSTS,
            external_id: id,
            event_id: None,
            kind,
            semantics: PublicSignalSemantics::PossibleSignal,
            canonical_url: canonical,
            source_class: class,
            published_at: Some(published_at),
            occurred_at: None,
            title: "Public Tibo statement",
            summary: text,
        },
        &normalized,
        collected,
    )
    .map(Some)
}

fn codex_reset_candidate(
    row: &Value,
    collected: &UtcTimestamp,
) -> Result<Option<PublicResetSignal>, ()> {
    let id = field(row, "id", 192)?;
    let (canonical, class) = canonical_public_url(field(row, "url", 2048)?)?;
    let kind = match (
        field(row, "type", 64)?,
        optional_text(row, "reset_kind", 64)?,
    ) {
        (_, Some("banked")) => PublicEventKind::GlobalBankedResetGrant,
        (_, Some("hard")) => PublicEventKind::GlobalFullReset,
        ("boost", _) => PublicEventKind::QuotaPolicyChange,
        _ => PublicEventKind::Unclassified,
    };
    // The timeline's broad "reset" group includes replies such as "Yes" and
    // ordinary usage explanations. Missing reset_kind must stay unclassified.
    let semantics = match optional_text(row, "announcement_state", 64)? {
        Some("confirmed") => PublicSignalSemantics::ConfirmedReset,
        Some("announced") => PublicSignalSemantics::ExplicitFutureReset,
        Some("retracted") => PublicSignalSemantics::Retracted,
        Some("corrected") => PublicSignalSemantics::Corrected,
        _ if kind.is_forecast_target() => PublicSignalSemantics::PossibleSignal,
        _ => PublicSignalSemantics::ContextOnly,
    };
    // The timeline's short summary can omit a grant announcement at the end
    // of a release post. Prefer its bounded full text when supplied, including
    // after that post ages out of the separate recent-post feed.
    let summary = match optional_text(row, "text", 4096)? {
        Some(text) => text,
        None => field(row, "summary", 4096)?,
    };
    candidate(
        CandidateInput {
            source_url: CODEX_RESET,
            external_id: id,
            event_id: None,
            kind,
            semantics,
            canonical_url: canonical,
            source_class: class,
            published_at: optional_time(row, "announced_at")?,
            occurred_at: optional_time(row, "effective_at")?,
            title: "Public Codex usage statement",
            summary,
        },
        row,
        collected,
    )
    .map(Some)
}

fn status_candidate(
    row: &Value,
    collected: &UtcTimestamp,
) -> Result<Option<PublicResetSignal>, ()> {
    let id = field(row, "id", 128)?;
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(());
    }
    let title = field(row, "name", 256)?;
    let updates = row
        .get("incident_updates")
        .and_then(Value::as_array)
        .ok_or(())?;
    if updates.len() > 128 {
        return Err(());
    }
    // One bounded incident context, not one global reset per incident update.
    let mut summary = String::new();
    for update in updates {
        let body = field(update, "body", 4096)?;
        if summary.len() + body.len() + 1 <= 4096 {
            if !summary.is_empty() {
                summary.push('\n');
            }
            summary.push_str(body);
        }
    }
    if summary.is_empty() {
        summary = title.into();
    }
    let context = format!("{title} {summary}").to_ascii_lowercase();
    if !(context.contains("codex") || context.contains("chatgpt work")) {
        return Ok(None);
    }
    candidate(
        CandidateInput {
            source_url: OPENAI_STATUS,
            external_id: id,
            event_id: None,
            kind: PublicEventKind::ServiceIncident,
            semantics: PublicSignalSemantics::ContextOnly,
            canonical_url: format!("https://status.openai.com/incidents/{id}"),
            source_class: PublicSourceClass::OfficialStatus,
            published_at: optional_time(row, "created_at")?,
            occurred_at: None,
            title,
            summary: &summary,
        },
        row,
        collected,
    )
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capacity_domain::public_reset::PublicEvidenceDisposition;
    use serde_json::json;

    fn collected() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-07T04:30:00Z").unwrap()
    }

    fn parse(source: PublicTimelineSource, value: Value) -> PublicCandidateBatch {
        decode_public_timeline(source, &serde_json::to_vec(&value).unwrap(), collected()).unwrap()
    }

    fn tracker_row() -> Value {
        json!({"provider":"openai", "slug":"sample-reset-1", "type":"hard_reset",
            "state":"confirmed", "confirmedAt":"2026-09-07T03:51:16Z",
            "title":"Synthetic reported reset", "summary":"Synthetic tracker claim, not a real event.",
            "source":{"url":"https://x.com/thsottiaux/status/123456", "kind":"authorized_social",
                "publishedAt":"2026-09-07T03:51:16Z"}})
    }

    #[test]
    fn tracker_expected_date_keeps_named_zone_without_becoming_an_occurrence() {
        let mut row = tracker_row();
        row["state"] = json!("likely");
        row["confirmedAt"] = Value::Null;
        row["expectedOn"] = json!("2026-09-11");
        row["timingSource"] = json!({"url":"https://x.com/thsottiaux/status/123456", "timeZone":"America/Los_Angeles"});
        let batch = parse(
            PublicTimelineSource::QuotaResets,
            json!({"data":[row.clone()]}),
        );
        let item = &batch.candidates[0];
        assert!(item.occurred_at.is_none());
        assert_eq!(item.disposition(), PublicEvidenceDisposition::Context);
        assert_eq!(item.kind, PublicEventKind::Unclassified);
        assert_eq!(
            item.announcement_timing.as_ref().unwrap().time_zone,
            "America/Los_Angeles"
        );
        row["timingSource"]["url"] = json!("https://x.com/thsottiaux/status/999999");
        let unrelated = parse(PublicTimelineSource::QuotaResets, json!({"data":[row]}));
        assert!(unrelated.candidates[0].announcement_timing.is_none());
    }

    #[test]
    fn full_post_keeps_a_late_commitment_without_versions_for_social_counters() {
        let text = format!(
            "{}\nA reset is landing by midnight today.",
            "Synthetic background explanation. ".repeat(12)
        );
        let mut row = json!({"id":"123456", "url":"https://x.com/thsottiaux/status/123456",
            "kind":"candidate", "text":text, "at":"2026-09-07T03:51:16Z", "likes":10});
        let value = |row: Value| {
            json!({"version":1, "profile":{"handle":"thsottiaux"}, "stale":false,
            "fetched_at":"2026-09-07T04:25:00Z", "tweets":[row]})
        };
        let first = parse(PublicTimelineSource::CodexResetPosts, value(row.clone()));
        assert_eq!(first.candidates[0].summary, text);
        assert!(first.candidates[0].summary.len() > 160);
        assert_eq!(
            first.candidates[0].disposition(),
            PublicEvidenceDisposition::Context
        );
        assert!(first.candidates[0].occurred_at.is_none());
        row["likes"] = json!(99999);
        row["reposts"] = json!(2345);
        let second = parse(PublicTimelineSource::CodexResetPosts, value(row.clone()));
        assert_eq!(
            first.candidates[0].source.content_sha256,
            second.candidates[0].source.content_sha256
        );
        row["text"] = json!("We will reset usage tomorrow.");
        let third = parse(PublicTimelineSource::CodexResetPosts, value(row.clone()));
        assert_ne!(
            first.candidates[0].source.content_sha256,
            third.candidates[0].source.content_sha256
        );
        row["url"] = json!("https://x.com/other/status/123456");
        assert_eq!(
            parse(PublicTimelineSource::CodexResetPosts, value(row)).rejected_records,
            1
        );
    }

    #[test]
    fn full_post_feed_accepts_signal_classification_without_promoting_it() {
        let batch = parse(
            PublicTimelineSource::CodexResetPosts,
            json!({
                "version":1, "profile":{"handle":"thsottiaux"}, "stale":false,
                "fetched_at":"2026-09-07T04:25:00Z", "tweets":[{
                    "id":"123456", "url":"https://x.com/thsottiaux/status/123456",
                    "kind":"signal", "text":"I promised a reset for Tuesday.",
                    "at":"2026-09-07T03:51:16Z"
                }]
            }),
        );
        assert_eq!(batch.candidates.len(), 1);
        assert_eq!(batch.candidates[0].kind, PublicEventKind::Unclassified);
        assert_eq!(
            batch.candidates[0].disposition(),
            PublicEvidenceDisposition::Context
        );
        assert_eq!(
            batch.candidates[0].semantics,
            PublicSignalSemantics::PossibleSignal
        );
        assert!(batch.candidates[0].occurred_at.is_none());
    }

    #[test]
    fn full_post_feed_rejects_stale_or_changed_profile_metadata() {
        let mut value = json!({"version":1, "profile":{"handle":"thsottiaux"}, "stale":true,
            "fetched_at":"2026-09-07T04:25:00Z", "tweets":[]});
        let decode = |value: &Value| {
            decode_public_timeline(
                PublicTimelineSource::CodexResetPosts,
                &serde_json::to_vec(value).unwrap(),
                collected(),
            )
        };
        assert_eq!(decode(&value).unwrap_err(), "public_source_upstream_stale");
        value["stale"] = json!(false);
        value["profile"]["handle"] = json!("another-account");
        assert_eq!(decode(&value).unwrap_err(), "public_source_schema_changed");
        value["profile"]["handle"] = json!("thsottiaux");
        value["fetched_at"] = json!("2026-09-07T03:00:00Z");
        assert_eq!(decode(&value).unwrap_err(), "public_source_upstream_stale");
    }

    #[test]
    fn september_dashboard_shape_is_retained_as_personal_unverified_evidence() {
        let mut row = tracker_row();
        row["source"]["kind"] = json!("provider_ui");
        row["source"]["url"] = json!("https://chatgpt.com/codex/settings/usage#observation");
        let batch = parse(PublicTimelineSource::QuotaResets, json!({"data":[row]}));
        let item = &batch.candidates[0];
        assert_eq!(item.scope, PublicEvidenceScope::Personal);
        assert_eq!(item.disposition(), PublicEvidenceDisposition::NeedsReview);
        assert!(item
            .source
            .canonical_url
            .starts_with("https://quotaresets.com/events/"));
        assert!(!serde_json::to_string(item)
            .unwrap()
            .contains("chatgpt.com/codex/settings"));
    }

    #[test]
    fn tracker_confirmation_never_substitutes_for_primary_source_review() {
        let batch = parse(
            PublicTimelineSource::QuotaResets,
            json!({"data":[tracker_row()]}),
        );
        assert_eq!(
            batch.candidates[0].source.source_class,
            PublicSourceClass::OfficialSocial
        );
        assert_eq!(
            batch.candidates[0].disposition(),
            PublicEvidenceDisposition::NeedsReview
        );
    }

    #[test]
    fn original_post_identity_deduplicates_trackers_and_url_aliases() {
        let first = parse(
            PublicTimelineSource::QuotaResets,
            json!({"data":[tracker_row()]}),
        );
        let second = parse(
            PublicTimelineSource::CodexReset,
            json!({"events":[{
                "id":"123456", "url":"https://twitter.com/thsottiaux/status/123456?s=20",
                "type":"reset", "reset_kind":"hard", "announcement_state":"announced",
                "announced_at":"2026-09-07T03:51:16Z", "summary":"A synthetic second mirror."
            }]}),
        );
        assert_eq!(
            first.candidates[0].evidence_family_id,
            second.candidates[0].evidence_family_id
        );
        assert_eq!(
            second.candidates[0].source.canonical_url,
            "https://x.com/thsottiaux/status/123456"
        );
    }

    #[test]
    fn banked_timeline_keeps_full_statement_instead_of_release_only_snippet() {
        let text = "Synthetic release introduction. We are loading a banked reset into all accounts of our Plus, Pro and Business users.";
        let mut row = json!({"id":"123456", "url":"https://x.com/thsottiaux/status/123456",
            "type":"credits", "reset_kind":"banked", "announcement_state":"none",
            "announced_at":"2026-09-07T03:51:16Z", "effective_at":null,
            "summary":"Synthetic release introduction.", "text":text});
        let decode = |row: Value| parse(PublicTimelineSource::CodexReset, json!({"events":[row]}));
        let full = decode(row.clone());
        assert_eq!(full.candidates[0].summary, text);
        assert_eq!(
            full.candidates[0].kind,
            PublicEventKind::GlobalBankedResetGrant
        );
        assert_eq!(
            full.candidates[0].disposition(),
            PublicEvidenceDisposition::NeedsReview
        );
        assert!(full.candidates[0].occurred_at.is_none());
        row["text"] = Value::Null;
        assert_eq!(
            decode(row.clone()).candidates[0].summary,
            "Synthetic release introduction."
        );
        row["text"] = json!("x".repeat(4097));
        assert_eq!(decode(row.clone()).rejected_records, 1);
        row["text"] = json!(42);
        assert_eq!(decode(row).rejected_records, 1);
    }

    #[test]
    fn banked_and_full_reset_do_not_share_a_training_event_identity() {
        let mut banked = tracker_row();
        banked["type"] = json!("banked_reset");
        let batch = parse(
            PublicTimelineSource::QuotaResets,
            json!({"data":[tracker_row(), banked]}),
        );
        assert_ne!(batch.candidates[0].event_id, batch.candidates[1].event_id);
        assert_eq!(
            batch.candidates[1].kind,
            PublicEventKind::GlobalBankedResetGrant
        );
    }

    #[test]
    fn reset_group_replies_and_scores_cannot_create_events_or_probabilities() {
        let batch = parse(
            PublicTimelineSource::CodexReset,
            json!({"events":[{
                "id":"123456", "url":"https://x.com/thsottiaux/status/123456",
                "type":"reset", "reset_kind":null, "confidence":"high", "score":93,
                "scope":"global", "summary":"Yes", "announcement_state":"none"
            }]}),
        );
        assert_eq!(batch.candidates[0].kind, PublicEventKind::Unclassified);
        assert_eq!(
            batch.candidates[0].disposition(),
            PublicEvidenceDisposition::Context
        );
        assert!(batch.candidates[0].event_id.is_none());
    }

    #[test]
    fn incidents_are_context_even_if_update_mentions_a_reset() {
        let batch = parse(
            PublicTimelineSource::OpenAiStatus,
            json!({"incidents":[{
                "id":"sample-incident", "name":"Codex usage issue", "created_at":"2026-09-07T02:00:00Z",
                "incident_updates":[{"body":"Synthetic reset-related service update."}]
            }]}),
        );
        assert_eq!(
            batch.candidates[0].disposition(),
            PublicEvidenceDisposition::Context
        );
    }

    #[test]
    fn private_local_spoofed_or_credentialed_urls_are_rejected() {
        for url in [
            "http://openai.com/index/reset",
            "https://localhost/incidents/a",
            "https://openai.com.evil.test/index/reset",
            "https://user:secret@x.com/thsottiaux/status/1",
            "https://chatgpt.com/codex/settings/usage",
            "https://127.0.0.1/index/reset",
            "https://x.com/thsottiaux/status/not-an-id",
        ] {
            let mut row = tracker_row();
            row["source"]["url"] = json!(url);
            let batch = parse(PublicTimelineSource::QuotaResets, json!({"data":[row]}));
            assert_eq!(batch.rejected_records, 1);
            assert!(batch.candidates.is_empty());
        }
    }

    #[test]
    fn unknown_schema_and_resource_limits_are_not_empty_successes() {
        assert!(
            decode_public_timeline(PublicTimelineSource::QuotaResets, b"{}", collected()).is_err()
        );
        assert!(decode_public_timeline(
            PublicTimelineSource::QuotaResets,
            &vec![b' '; MAX_BODY_BYTES + 1],
            collected()
        )
        .is_err());
        let batch = parse(
            PublicTimelineSource::QuotaResets,
            json!({"data":[null, {"provider":"anthropic"}, tracker_row()]}),
        );
        assert_eq!(batch.rejected_records, 1);
        assert_eq!(batch.skipped_records, 1);
        assert_eq!(batch.candidates.len(), 1);
    }

    #[test]
    fn malformed_and_future_dates_are_rejected_but_date_only_is_not_midnight() {
        let mut row = tracker_row();
        row["confirmedAt"] = json!("2026-09-07");
        let batch = parse(
            PublicTimelineSource::QuotaResets,
            json!({"data":[row.clone()]}),
        );
        assert!(batch.candidates[0].occurred_at.is_none());
        row["source"]["publishedAt"] = json!("2026-09-08T00:00:00Z");
        let batch = parse(PublicTimelineSource::QuotaResets, json!({"data":[row]}));
        assert_eq!(batch.rejected_records, 1);
    }
}
