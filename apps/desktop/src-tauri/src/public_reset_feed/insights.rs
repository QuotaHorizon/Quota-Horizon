//! Attributed third-party estimates and public conversation context. These are
//! display data, never Horizon forecasts, quota facts or mutation inputs.
use super::*;
use serde_json::Value;
use url::Url;

pub(crate) const FORECAST_URL: &str = "https://codex-reset.com/api/forecast";
pub(crate) const POSTS_URL: &str = "https://codex-reset.com/api/feed?locale=zh";
const MAX_CACHE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalForecast {
    pub source_url: String,
    pub updated_at: String,
    pub checked_at: String,
    pub probability_24h: f64,
    pub probability_48h: f64,
    pub confidence: String,
    pub mode: String,
    pub last_reset_at: Option<String>,
    pub signal_score: Option<f64>,
    pub signal_url: Option<String>,
    pub signal_published_at: Option<String>,
    pub signal_deadline: Option<String>,
    #[serde(default)]
    pub signal_state: Option<String>,
    pub signal_corrected: bool,
    #[serde(default)]
    pub model_probability_24h: Option<f64>,
    #[serde(default)]
    pub model_probability_48h: Option<f64>,
    #[serde(default)]
    pub evidence_urls: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForecastState {
    pub value: Option<ExternalForecast>,
    pub attempted_at: Option<String>,
    pub issue: Option<SourceIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParentPost {
    pub id: String,
    pub author: String,
    pub url: String,
    pub text: Option<String>,
    pub checked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicPost {
    pub id: String,
    pub url: String,
    pub text: String,
    pub translated_text: Option<String>,
    pub published_at: String,
    pub kind: String,
    pub is_reply: Option<bool>,
    pub parent: Option<ParentPost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostFeed {
    pub fetched_at: String,
    pub checked_at: String,
    pub posts: Vec<PublicPost>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicInsights {
    pub forecast: ForecastState,
    pub posts: Option<PostFeed>,
}

fn text(value: &Value, key: &str, max: usize) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|value| {
            !value.trim().is_empty()
                && value.len() <= max
                && !value
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        })
        .map(str::to_owned)
}

fn timestamp(value: &Value, key: &str) -> Option<String> {
    let value = text(value, key, 64)?;
    DateTime::parse_from_rfc3339(&value).ok()?;
    Some(value)
}

fn percent(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
}

fn post_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 24 && value.bytes().all(|c| c.is_ascii_digit())
}

fn handle(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 30
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

pub(crate) fn parse_forecast(
    bytes: &[u8],
    at: &UtcTimestamp,
) -> Result<ExternalForecast, SourceIssue> {
    if bytes.len() > MAX_CACHE_BYTES {
        return Err(SourceIssue::ResponseTooLarge);
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|_| SourceIssue::InvalidResponse)?;
    let updated = timestamp(&v, "updated_at").ok_or(SourceIssue::SchemaChanged)?;
    if DateTime::parse_from_rfc3339(&updated)
        .unwrap()
        .signed_duration_since(DateTime::parse_from_rfc3339(at.as_str()).unwrap())
        .num_seconds()
        > 60
    {
        return Err(SourceIssue::InvalidResponse);
    }
    let confidence = text(&v, "confidence", 32)
        .filter(|v| ["low", "medium", "high"].contains(&v.as_str()))
        .ok_or(SourceIssue::SchemaChanged)?;
    let mode = text(&v, "mode", 32).ok_or(SourceIssue::SchemaChanged)?;
    let alert = &v["latest_alert"];
    let signal_url = text(alert, "url", 256).filter(|url| {
        canonical_public_url(url).is_ok_and(|(canonical, _)| canonical == *url)
            && url.starts_with("https://x.com/thsottiaux/status/")
    });
    Ok(ExternalForecast {
        source_url: FORECAST_URL.into(),
        updated_at: updated,
        checked_at: at.as_str().into(),
        probability_24h: percent(&v["probabilities"]["rounded_24h"])
            .ok_or(SourceIssue::SchemaChanged)?,
        probability_48h: percent(&v["probabilities"]["rounded_48h"])
            .ok_or(SourceIssue::SchemaChanged)?,
        confidence,
        mode,
        last_reset_at: timestamp(&v, "last_reset_at"),
        signal_score: signal_url.as_ref().and_then(|_| percent(&alert["score"])),
        signal_url,
        signal_published_at: timestamp(alert, "source_at"),
        signal_deadline: timestamp(&alert["window"], "end_at"),
        signal_state: text(alert, "state", 32),
        signal_corrected: alert
            .get("corrected")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || alert
                .get("state")
                .and_then(Value::as_str)
                .is_some_and(|state| matches!(state, "corrected" | "retracted" | "withdrawn")),
        model_probability_24h: percent(&v["probabilities"]["model_24h"]),
        model_probability_48h: percent(&v["probabilities"]["model_48h"]),
        evidence_urls: v["context"]["evidence_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| post_id(id))
            .take(32)
            .map(|id| format!("https://x.com/thsottiaux/status/{id}"))
            .collect(),
    })
}

pub(crate) fn decode_posts(v: &Value, at: &UtcTimestamp) -> Option<PostFeed> {
    let fetched_at = timestamp(v, "fetched_at")?;
    let mut posts = Vec::new();
    for row in v
        .get("tweets")?
        .as_array()?
        .iter()
        .chain(
            v.get("radar_context")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .take(4096)
    {
        let Some(id) = text(row, "id", 24).filter(|id| post_id(id)) else {
            continue;
        };
        let url = format!("https://x.com/thsottiaux/status/{id}");
        if row.get("url").and_then(Value::as_str) != Some(url.as_str()) {
            continue;
        }
        let (Some(body), Some(published_at)) = (text(row, "text", 8192), timestamp(row, "at"))
        else {
            continue;
        };
        if posts.iter().any(|post: &PublicPost| post.id == id) {
            continue;
        }
        let is_reply = row.get("is_reply").and_then(Value::as_bool);
        let parent = if is_reply == Some(true) {
            text(row, "in_reply_to_tweet_id", 24)
                .filter(|id| post_id(id))
                .zip(text(row, "replying_to", 30).filter(|author| handle(author)))
                .map(|(id, author)| ParentPost {
                    url: format!("https://x.com/{author}/status/{id}"),
                    id,
                    author,
                    text: None,
                    checked_at: None,
                })
        } else {
            None
        };
        let kind = match row.get("kind").and_then(Value::as_str) {
            Some("banked") => "grant",
            Some("candidate" | "reset")
                if row.get("explicit_reset_claim").and_then(Value::as_bool) == Some(true) =>
            {
                "reset_report"
            }
            Some("candidate" | "reset" | "signal") => "notice",
            Some("limits") => "limits",
            _ => "context",
        };
        posts.push(PublicPost {
            id,
            url,
            text: body,
            published_at,
            kind: kind.into(),
            is_reply,
            parent,
            translated_text: (row.get("translation_status").and_then(Value::as_str)
                == Some("translated"))
            .then(|| text(row, "localized_text", 16384))
            .flatten(),
        });
    }
    posts.sort_by_key(|post| {
        std::cmp::Reverse(DateTime::parse_from_rfc3339(&post.published_at).ok())
    });
    posts.truncate(32);
    Some(PostFeed {
        fetched_at,
        checked_at: at.as_str().into(),
        posts,
    })
}

/// Consume only text from X's public embed response. No HTML, widget scripts,
/// cookies, image requests or remote navigation enter the WebView.
pub(crate) fn decode_parent(bytes: &[u8], parent: &ParentPost) -> Option<String> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("url")?.as_str()? != parent.url {
        return None;
    }
    let html = text(&v, "html", 32 * 1024)?;
    let begin = html.find("<p ").or_else(|| html.find("<p>"))?;
    let html = &html[begin..];
    let inner = &html[html.find('>')? + 1..html.find("</p>")?];
    let mut output = String::new();
    let mut rest = inner;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let end = rest.find('>')?;
            let tag = &rest[1..end];
            if tag == "br" || tag == "br/" || tag == "br /" {
                output.push('\n');
            } else if tag != "/a" && !tag.starts_with("a ") {
                return None;
            }
            rest = &rest[end + 1..];
        } else if rest.starts_with('&') {
            if let Some(end) = rest.find(';').filter(|end| *end < 16) {
                let entity = &rest[1..end];
                let decoded = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    _ => entity
                        .strip_prefix("#x")
                        .and_then(|n| u32::from_str_radix(n, 16).ok())
                        .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                        .and_then(char::from_u32),
                };
                if let Some(c) = decoded {
                    output.push(c);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            output.push('&');
            rest = &rest[1..];
        } else {
            let c = rest.chars().next()?;
            output.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if output.len() > 8192
        || output.trim().is_empty()
        || output
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return None;
    }
    Some(output)
}

pub(crate) fn enrich_parents(
    client: &reqwest::blocking::Client,
    feed: &mut PostFeed,
    cached: Option<&PostFeed>,
    at: &UtcTimestamp,
) {
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<_> = feed
        .posts
        .iter()
        .filter_map(|post| post.parent.clone())
        .filter(|parent| seen.insert(parent.url.clone()))
        .take(8)
        .collect();
    let results = std::thread::scope(|scope| {
        targets
            .into_iter()
            .map(|mut parent| {
                scope.spawn(move || {
                    let previous = cached
                        .into_iter()
                        .flat_map(|feed| &feed.posts)
                        .filter_map(|post| post.parent.as_ref())
                        .find(|p| p.url == parent.url && p.text.is_some());
                    if previous.is_some_and(|p| {
                        p.checked_at.as_ref().is_some_and(|time| {
                            DateTime::parse_from_rfc3339(time).ok().is_some_and(|time| {
                                (0..86400).contains(
                                    &DateTime::parse_from_rfc3339(at.as_str())
                                        .unwrap()
                                        .signed_duration_since(time)
                                        .num_seconds(),
                                )
                            })
                        })
                    }) {
                        return previous.unwrap().clone();
                    }
                    // Query values are constructed from validated public handles and IDs.
                    let mut url = Url::parse("https://publish.x.com/oembed").unwrap();
                    url.query_pairs_mut()
                        .append_pair("url", &parent.url)
                        .append_pair("omit_script", "true")
                        .append_pair("dnt", "true");
                    parent.checked_at = Some(at.as_str().into());
                    parent.text = client
                        .get(url)
                        .send()
                        .ok()
                        .and_then(|r| read_response(r).ok())
                        .and_then(|bytes| decode_parent(&bytes, &parent));
                    // Preserve the last known parent and its actual fetch time on failure.
                    if parent.text.is_none() {
                        if let Some(previous) = previous {
                            return previous.clone();
                        }
                    }
                    parent
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .collect::<Vec<_>>()
    });
    for post in &mut feed.posts {
        if let Some(parent) = post.parent.as_mut() {
            if let Some(result) = results.iter().find(|p| p.url == parent.url) {
                *parent = result.clone();
            }
        }
    }
}

fn validate_cache(value: &PublicInsights) -> bool {
    let date = |v: &str| v.len() <= 64 && DateTime::parse_from_rfc3339(v).is_ok();
    let body = |v: &str, max| {
        !v.trim().is_empty()
            && v.len() <= max
            && !v
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    };
    if value
        .forecast
        .attempted_at
        .as_deref()
        .is_some_and(|v| !date(v))
    {
        return false;
    }
    if let Some(v) = &value.forecast.value {
        if v.source_url != FORECAST_URL
            || !date(&v.updated_at)
            || !date(&v.checked_at)
            || ![v.probability_24h, v.probability_48h]
                .iter()
                .all(|v| v.is_finite() && (0.0..=100.0).contains(v))
            || v.signal_score
                .is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
            || !["low", "medium", "high"].contains(&v.confidence.as_str())
            || !body(&v.mode, 32)
            || v.signal_state
                .as_deref()
                .is_some_and(|state| !body(state, 32))
            || [&v.last_reset_at, &v.signal_deadline, &v.signal_published_at]
                .iter()
                .any(|v| v.as_deref().is_some_and(|v| !date(v)))
            || v.signal_url.as_ref().is_some_and(|v| {
                !v.strip_prefix("https://x.com/thsottiaux/status/")
                    .is_some_and(post_id)
            })
        {
            return false;
        }
    }
    if let Some(feed) = &value.posts {
        if feed.posts.len() > 32 || !date(&feed.fetched_at) || !date(&feed.checked_at) {
            return false;
        }
        for post in &feed.posts {
            if !post_id(&post.id)
                || post.url != format!("https://x.com/thsottiaux/status/{}", post.id)
                || !body(&post.text, 8192)
                || post
                    .translated_text
                    .as_deref()
                    .is_some_and(|v| !body(v, 16384))
                || !date(&post.published_at)
                || !["grant", "reset_report", "notice", "limits", "context"]
                    .contains(&post.kind.as_str())
            {
                return false;
            }
            if let Some(p) = &post.parent {
                if post.is_reply != Some(true)
                    || !post_id(&p.id)
                    || !handle(&p.author)
                    || p.url != format!("https://x.com/{}/status/{}", p.author, p.id)
                    || p.text.as_deref().is_some_and(|v| !body(v, 8192))
                    || p.checked_at.as_deref().is_some_and(|v| !date(v))
                {
                    return false;
                }
            }
        }
    }
    true
}

pub(super) fn read_cache(conn: &Connection) -> Result<PublicInsights, String> {
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='public_insights')",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if !has_table {
        return Ok(PublicInsights::default());
    }
    let mut stmt = conn
        .prepare("SELECT payload FROM public_insights WHERE id=1")
        .map_err(db_error)?;
    let mut rows = stmt.query([]).map_err(db_error)?;
    let Some(row) = rows.next().map_err(db_error)? else {
        return Ok(PublicInsights::default());
    };
    let json: String = row.get(0).map_err(db_error)?;
    if json.len() > MAX_CACHE_BYTES {
        return Err(db_error("insight bounds"));
    }
    let value = serde_json::from_str(&json).map_err(db_error)?;
    if !validate_cache(&value) {
        return Err(db_error("invalid insights"));
    }
    Ok(value)
}

pub(super) fn save_cache(conn: &Connection, value: &PublicInsights) -> Result<(), String> {
    if !validate_cache(value) {
        return Err(db_error("invalid insights"));
    }
    let json = serde_json::to_string(value).map_err(db_error)?;
    if json.len() > MAX_CACHE_BYTES {
        return Err(db_error("insight bounds"));
    }
    conn.execute("INSERT INTO public_insights(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", [json]).map_err(db_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn at() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-23T03:00:00Z").unwrap()
    }
    #[test]
    fn forecast_keeps_model_probability_separate_from_watch_score() {
        let mut v = json!({"updated_at":"2026-09-23T02:59:00Z","mode":"model","confidence":"low",
            "probabilities":{"rounded_24h":20,"rounded_48h":35},"latest_alert":{"score":93,
            "url":"https://x.com/thsottiaux/status/123", "state":"active"}});
        let parsed = parse_forecast(&serde_json::to_vec(&v).unwrap(), &at()).unwrap();
        assert_eq!(parsed.probability_24h, 20.0);
        assert_eq!(parsed.signal_score, Some(93.0));
        v["probabilities"]["rounded_24h"] = json!(101);
        assert!(parse_forecast(&serde_json::to_vec(&v).unwrap(), &at()).is_err());
        v["probabilities"]["rounded_24h"] = json!(20);
        v["updated_at"] = json!("2099-01-01T00:00:00Z");
        assert!(parse_forecast(&serde_json::to_vec(&v).unwrap(), &at()).is_err());
    }
    #[test]
    fn completed_and_expired_alerts_are_not_corrections() {
        for (state, corrected) in [
            ("active", false),
            ("confirmed", false),
            ("expired", false),
            ("inactive", false),
            ("corrected", true),
            ("retracted", true),
            ("withdrawn", true),
        ] {
            let mut value = json!({"updated_at":"2026-09-23T02:59:00Z", "mode":"model", "confidence":"low",
                "probabilities":{"rounded_24h":16,"rounded_48h":29}, "latest_alert":{
                    "url":"https://x.com/thsottiaux/status/123", "state":state, "corrected":false}});
            let parsed = parse_forecast(&serde_json::to_vec(&value).unwrap(), &at()).unwrap();
            assert_eq!(parsed.signal_state.as_deref(), Some(state));
            assert_eq!(parsed.signal_corrected, corrected, "{state}");
            value["latest_alert"]["corrected"] = json!(true);
            assert!(
                parse_forecast(&serde_json::to_vec(&value).unwrap(), &at())
                    .unwrap()
                    .signal_corrected
            );
        }
    }

    #[test]
    fn preserves_reply_topology_translation_and_unclassified_notices() {
        let v = json!({"fetched_at":"2026-09-23T02:59:00Z","tweets":[{
            "id":"123","url":"https://x.com/thsottiaux/status/123","text":"OK fine. Tuesday.",
            "at":"2026-09-22T04:00:00Z","kind":"signal","is_reply":true,"replying_to":"someone",
            "in_reply_to_tweet_id":"122","localized_text":"好吧，周二。","translation_status":"translated"}]});
        let feed = decode_posts(&v, &at()).unwrap();
        let post = &feed.posts[0];
        assert_eq!(post.kind, "notice");
        assert_eq!(post.parent.as_ref().unwrap().id, "122");
        assert!(post.translated_text.is_some());
    }
    #[test]
    fn parent_embed_becomes_plain_text_and_rejects_wrong_identity_or_script() {
        let p = ParentPost {
            id: "123".into(),
            author: "someone".into(),
            url: "https://x.com/someone/status/123".into(),
            text: None,
            checked_at: None,
        };
        let encode =
            |url: &str, html: &str| serde_json::to_vec(&json!({"url":url,"html":html})).unwrap();
        assert_eq!(
            decode_parent(
                &encode(
                    &p.url,
                    "<blockquote><p lang=\"en\">banked &amp; reset<br>please</p></blockquote>"
                ),
                &p
            )
            .unwrap(),
            "banked & reset\nplease"
        );
        assert!(decode_parent(
            &encode("https://x.com/someone/status/124", "<p>wrong</p>"),
            &p
        )
        .is_none());
        assert!(decode_parent(&encode(&p.url, "<p><script>bad</script></p>"), &p).is_none());
    }

    #[test]
    fn persisted_insights_reject_tampered_urls_and_invalid_probabilities() {
        let v = json!({"updated_at":"2026-09-23T02:59:00Z","mode":"model","confidence":"low",
            "probabilities":{"rounded_24h":20,"rounded_48h":35}});
        let forecast = parse_forecast(&serde_json::to_vec(&v).unwrap(), &at()).unwrap();
        let mut cached = PublicInsights {
            forecast: ForecastState {
                value: Some(forecast),
                ..Default::default()
            },
            posts: None,
        };
        assert!(validate_cache(&cached));
        cached.forecast.value.as_mut().unwrap().probability_24h = 101.0;
        assert!(!validate_cache(&cached));
        cached.forecast.value.as_mut().unwrap().probability_24h = 20.0;
        cached.forecast.value.as_mut().unwrap().signal_url =
            Some("https://x.com/thsottiaux/status/123?redirect=secret".into());
        assert!(!validate_cache(&cached));
    }

    #[test]
    fn invalid_parent_identity_never_becomes_a_network_target() {
        let mut v = json!({"fetched_at":"2026-09-23T02:59:00Z","tweets":[{
            "id":"123","url":"https://x.com/thsottiaux/status/123","text":"OK fine.",
            "at":"2026-09-22T04:00:00Z","kind":"signal","is_reply":true,"replying_to":"localhost/../x",
            "in_reply_to_tweet_id":"122"}]});
        assert!(decode_posts(&v, &at()).unwrap().posts[0].parent.is_none());
        v["tweets"][0]["replying_to"] = json!("someone");
        v["tweets"][0]["in_reply_to_tweet_id"] = json!("122?redirect=http://localhost");
        assert!(decode_posts(&v, &at()).unwrap().posts[0].parent.is_none());
    }

    #[test]
    #[ignore = "explicit public-network verification only; no account or runtime writes"]
    fn live_public_insights_include_forecast_and_reply_context() {
        let client = public_client().unwrap();
        let at = now();
        let forecast = parse_forecast(
            &read_response(client.get(FORECAST_URL).send().unwrap()).unwrap(),
            &at,
        )
        .unwrap();
        let mut batch = fetch_source(&client, PublicTimelineSource::CodexResetPosts).unwrap();
        let feed = batch.posts.as_mut().unwrap();
        enrich_parents(&client, feed, None, &now());
        assert!(!feed.posts.is_empty());
        assert!(feed
            .posts
            .iter()
            .any(|post| post.parent.as_ref().is_some_and(|p| p.text.is_some())));
        assert!(validate_cache(&PublicInsights {
            forecast: ForecastState {
                value: Some(forecast.clone()),
                ..Default::default()
            },
            posts: Some(feed.clone())
        }));
        eprintln!(
            "Public-only check: 24h={}%, 48h={}%, posts={}, fetched parents={}",
            forecast.probability_24h,
            forecast.probability_48h,
            feed.posts.len(),
            feed.posts
                .iter()
                .filter(|p| p.parent.as_ref().is_some_and(|p| p.text.is_some()))
                .count()
        );
    }
}
