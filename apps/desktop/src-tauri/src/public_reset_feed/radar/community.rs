use super::*;

fn contains_any(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| t.contains(w))
}

fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in text.chars() {
        if c == '<' {
            tag = true;
            out.push(' ');
        } else if c == '>' {
            tag = false;
        } else if !tag {
            out.push(c);
        }
    }
    for (entity, value) in [
        ("&#x27;", "'"),
        ("&#39;", "'"),
        ("&#x2F;", "/"),
        ("&quot;", "\""),
        ("&amp;", "&"),
        ("&gt;", ">"),
        ("&lt;", "<"),
    ] {
        out = out.replace(entity, value);
    }
    out
}

// v1 labels explicit language only. Desire, completed delivery, personal timers
// and quoted announcements have separate labels and never create bullish votes.
pub(super) fn classify(text: &str) -> Option<(&'static str, &'static str, Option<u32>)> {
    let t = text.to_lowercase();
    if !contains_any(&t, &["reset", "重置"])
        || !contains_any(
            &t,
            &["codex", "quota", "usage", "limit", "tibo", "额度", "限额"],
        )
    {
        return None;
    }
    if contains_any(
        &t,
        &[
            "git reset",
            "password reset",
            "reset the app",
            "reset context",
            "重置密码",
            "reset authentication",
        ],
    ) {
        return None;
    }
    if contains_any(
        &t,
        &[
            "my weekly",
            "my quota",
            "my limit",
            "my reset",
            "rolls over",
            "rolling window",
            "personal reset",
            "我的额度",
            "我的周",
            "自然重置",
            "reset credit",
            "banked reset",
            "reset card",
            "重置卡",
        ],
    ) {
        return Some(("observation", "account_or_card", None));
    }
    if contains_any(
        &t,
        &[
            "hope",
            "wish",
            "please reset",
            "pls reset",
            "want a reset",
            "need a reset",
            "begging",
            "希望",
            "求重置",
            "盼",
            "想要重置",
        ],
    ) {
        return Some(("wish", "wish", None));
    }
    if contains_any(
        &t,
        &[
            "tibo said",
            "tibo says",
            "tibo announced",
            "thsottiaux/status/",
            "according to",
            "官宣",
            "tibo说",
            "tibo 说",
        ],
    ) {
        return Some(("observation", "quoted_evidence", None));
    }
    let horizon = if contains_any(
        &t,
        &["tomorrow", "48h", "48 hours", "明天", "48小时", "48 小时"],
    ) {
        Some(48)
    } else if contains_any(
        &t,
        &[
            "today",
            "tonight",
            "24h",
            "24 hours",
            "next few hours",
            "今天",
            "今晚",
            "24小时",
            "24 小时",
        ],
    ) {
        Some(24)
    } else {
        None
    };
    if contains_any(
        &t,
        &[
            "won't reset",
            "will not reset",
            "no reset expected",
            "don't expect",
            "do not expect",
            "don't think",
            "do not think",
            "unlikely to",
            "not likely to",
            "不会重置",
            "不认为",
            "不太可能重置",
            "不看好",
        ],
    ) {
        return Some(("pessimistic", "explicit_prediction", horizon));
    }
    if contains_any(
        &t,
        &[
            "will reset",
            "will be a reset",
            "expect a reset",
            "expect another reset",
            "expect the reset",
            "likely to reset",
            "reset is coming",
            "probably reset",
            "probably get a reset",
            "会重置",
            "大概率重置",
            "预计重置",
            "看好重置",
        ],
    ) {
        return Some(("optimistic", "explicit_prediction", horizon));
    }
    if contains_any(
        &t,
        &[
            "maybe",
            "might",
            "whether",
            "will there",
            "any reset",
            "reset?",
            "会不会",
            "是否",
            "可能",
            "重置吗",
        ],
    ) {
        return Some(("uncertain", "uncertain", horizon));
    }
    Some(("observation", "discussion", None))
}

fn opinion(
    id: String,
    channel: &str,
    author: &str,
    url: &str,
    text: &str,
    published: &str,
    at: &UtcTimestamp,
) -> Option<Opinion> {
    let age = seconds(at.as_str())? - seconds(published)?;
    if !(0..12 * 3600).contains(&age)
        || !public_url(url)
        || author.is_empty()
        || author.len() > 100
        || author.ends_with("[bot]")
    {
        return None;
    }
    let stripped = plain(text);
    // Keep relevant prose, excluding issue templates, code and quoted messages.
    let relevant = stripped
        .lines()
        .filter(|line| !line.trim_start().starts_with(['>', '#', '`']))
        .filter(|line| contains_any(&line.to_lowercase(), &["reset", "重置"]))
        .collect::<Vec<_>>()
        .join(" ");
    let excerpt: String = relevant.chars().take(1200).collect();
    let (stance, reason, horizon_hours) = classify(&excerpt)?;
    let evidence_urls = stripped
        .split_whitespace()
        .map(|s| {
            s.trim_matches(|c: char| matches!(c, '(' | ')' | '"' | ',' | '.' | '<' | '>' | ']'))
        })
        .filter(|s| s.starts_with("https://x.com/") && public_url(s))
        .take(8)
        .map(str::to_owned)
        .collect();
    Some(Opinion {
        id,
        channel: channel.into(),
        author: author.into(),
        url: url.into(),
        text: excerpt,
        published_at: published.into(),
        stance: stance.into(),
        reason: reason.into(),
        evidence_urls,
        horizon_hours,
    })
}

fn json(client: &reqwest::blocking::Client, url: &str) -> Result<Value, SourceIssue> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .map_err(|_| SourceIssue::RequestFailed)?;
    serde_json::from_slice(&read_response(response)?).map_err(|_| SourceIssue::InvalidResponse)
}

fn collect_channel(id: &str, at: &UtcTimestamp) -> (Channel, Vec<Opinion>) {
    let hn = id == "hacker_news";
    let mut channel = Channel {
        id: id.into(),
        name: if hn {
            "Hacker News"
        } else {
            "GitHub · openai/codex"
        }
        .into(),
        url: if hn {
            "https://news.ycombinator.com"
        } else {
            "https://github.com/openai/codex/issues"
        }
        .into(),
        query: if hn {
            "codex reset · comments · exact words · newest 100 · 12h"
        } else {
            "openai/codex · quota reset issues + recent comments · newest 100 each · 12h"
        }
        .into(),
        attempted_at: at.as_str().into(),
        success_at: None,
        issue: None,
        scanned: 0,
        truncated: false,
    };
    let mut opinions = vec![];
    let result = (|| -> Result<(), SourceIssue> {
        let client = public_client()?;
        let cutoff = seconds(at.as_str()).unwrap() - 12 * 3600;
        if hn {
            let url = format!("https://hn.algolia.com/api/v1/search_by_date?query=codex%20reset&tags=comment&hitsPerPage=100&typoTolerance=false&removeWordsIfNoResults=none&queryType=prefixNone&restrictSearchableAttributes=comment_text&numericFilters=created_at_i%3E{cutoff}");
            let v = json(&client, &url)?;
            let rows = v["hits"].as_array().ok_or(SourceIssue::SchemaChanged)?;
            channel.scanned = rows.len();
            channel.truncated = v["nbHits"].as_u64().unwrap_or(0) > rows.len() as u64;
            for row in rows.iter().take(100) {
                let (Some(key), Some(author), Some(text), Some(time)) = (
                    short(&row["objectID"], 30),
                    short(&row["author"], 100),
                    short(&row["comment_text"], 32768),
                    stamp(&row["created_at"]),
                ) else {
                    continue;
                };
                if !key.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                if let Some(p) = opinion(
                    format!("hn:{key}"),
                    id,
                    &author,
                    &format!("https://news.ycombinator.com/item?id={key}"),
                    &text,
                    &time,
                    at,
                ) {
                    opinions.push(p);
                }
            }
        } else {
            let since = DateTime::from_timestamp(cutoff, 0)
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true);
            let mut comments_url =
                url::Url::parse("https://api.github.com/repos/openai/codex/issues/comments")
                    .unwrap();
            comments_url
                .query_pairs_mut()
                .append_pair("since", &since)
                .append_pair("per_page", "100")
                .append_pair("sort", "created")
                .append_pair("direction", "desc");
            let comments = json(&client, comments_url.as_str())?;
            let rows = comments.as_array().ok_or(SourceIssue::SchemaChanged)?;
            channel.scanned += rows.len();
            channel.truncated |= rows.len() == 100;
            let mut all = rows.clone();
            let mut issues_url = url::Url::parse("https://api.github.com/search/issues").unwrap();
            issues_url
                .query_pairs_mut()
                .append_pair(
                    "q",
                    &format!("repo:openai/codex is:issue quota reset created:>{since}"),
                )
                .append_pair("sort", "created")
                .append_pair("order", "desc")
                .append_pair("per_page", "100");
            let issues = json(&client, issues_url.as_str())?;
            let rows = issues["items"]
                .as_array()
                .ok_or(SourceIssue::SchemaChanged)?;
            channel.scanned += rows.len();
            channel.truncated |= issues["total_count"].as_u64().unwrap_or(0) > rows.len() as u64;
            all.extend(rows.clone());
            for row in all.iter().take(200) {
                let (Some(key), Some(author), Some(text), Some(time), Some(link)) = (
                    row["id"].as_u64(),
                    short(&row["user"]["login"], 100),
                    short(&row["body"], 65536),
                    stamp(&row["created_at"]),
                    short(&row["html_url"], 512),
                ) else {
                    continue;
                };
                if !link.starts_with("https://github.com/openai/codex/") {
                    continue;
                }
                if let Some(p) = opinion(
                    format!("github:{key}"),
                    id,
                    &author,
                    &link,
                    &text,
                    &time,
                    at,
                ) {
                    opinions.push(p);
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => channel.success_at = Some(at.as_str().into()),
        Err(e) => {
            channel.issue = Some(e);
            opinions.clear();
        }
    }
    opinions.sort_by_key(|p| std::cmp::Reverse(seconds(&p.published_at)));
    if opinions.len() > 64 {
        channel.truncated = true;
        opinions.truncate(64);
    }
    (channel, opinions)
}

pub(super) fn collect(at: &UtcTimestamp) -> Vec<(Channel, Vec<Opinion>)> {
    std::thread::scope(|scope| {
        let hn = scope.spawn(|| collect_channel("hacker_news", at));
        let gh = collect_channel("github", at);
        let mut all = vec![gh];
        if let Ok(result) = hn.join() {
            all.push(result);
        }
        all
    })
}
