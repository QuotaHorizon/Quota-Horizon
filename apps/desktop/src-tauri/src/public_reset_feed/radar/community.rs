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

// Classify the author's own judgment separately from a quoted announcement.
// Wishes, delivery reports and individual timers remain separate observations.
pub(super) fn classify(text: &str) -> Option<(&'static str, &'static str, Option<u32>)> {
    let t = text.to_lowercase();
    if !contains_any(&t, &["reset", "重置", "发卡"])
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
            "used a reset",
            "redeemed a reset",
            "received a reset",
            "got a reset card",
            "my banked",
            "我的重置卡",
            "用了重置卡",
            "收到重置卡",
            "redemption entries",
            "reset history",
            "use usage resets",
            "reset countdown",
            "resets in ",
        ],
    ) {
        return Some(("observation", "account_or_card", None));
    }
    let personal_inference = contains_any(
        &t,
        &[
            "i think",
            "i believe",
            "i expect",
            "i predict",
            "i reckon",
            "i bet",
            "my guess",
            "我认为",
            "我觉得",
            "我预计",
            "我估计",
            "我猜",
            "大概率",
            "should reset",
            "likely to reset",
        ],
    );
    if !personal_inference
        && contains_any(
            &t,
            &[
                "hope",
                "wish",
                "please reset",
                "pls reset",
                "want a reset",
                "need another banked",
                "need a reset",
                "begging",
                "希望",
                "求重置",
                "盼",
                "想要重置",
            ],
        )
    {
        return Some(("wish", "wish", None));
    }
    if !personal_inference
        && contains_any(
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
        )
    {
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
            "won't issue",
            "will not issue",
            "no banked reset",
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
            "不会发卡",
        ],
    ) {
        return Some(("pessimistic", "explicit_prediction", horizon));
    }
    let expects_credits = contains_any(
        &t,
        &[
            "banked reset",
            "reset credit",
            "reset card",
            "重置卡",
            "发卡",
        ],
    ) && (t
        .split(|c: char| !c.is_alphabetic())
        .any(|word| matches!(word, "expect" | "predict"))
        || contains_any(
            &t,
            &[
                "will give",
                "will issue",
                "will get",
                "likely to receive",
                "likely to get",
                "应该",
                "大概率",
                "预计",
                "会发",
            ],
        ));
    if expects_credits
        || contains_any(
            &t,
            &[
                "will reset",
                "will be a reset",
                "expect a reset",
                "expect another reset",
                "expect another banked reset",
                "expect a banked reset",
                "will issue a reset",
                "will issue another reset",
                "will give a banked reset",
                "will give another banked reset",
                "expect the reset",
                "likely to reset",
                "reset is coming",
                "probably reset",
                "probably get a reset",
                "should reset",
                "expect codex",
                "expect tibo",
                "expect a codex",
                "think codex will",
                "think tibo will",
                "bet on a reset",
                "会重置",
                "大概率重置",
                "预计重置",
                "看好重置",
                "会发卡",
                "大概率发卡",
                "预计发卡",
            ],
        )
    {
        return Some((
            "optimistic",
            if personal_inference {
                "personal_inference"
            } else {
                "explicit_prediction"
            },
            horizon,
        ));
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
    if !(0..48 * 3600).contains(&age)
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
        .filter(|line| contains_any(&line.to_lowercase(), &["reset", "重置", "发卡"]))
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
            "codex reset · comments · exact words · newest 100 · 48h"
        } else {
            "openai/codex · latest 100 comments + reset issues updated within 48h + last 100 comments from 3 active threads"
        }
        .into(),
        attempted_at: at.as_str().into(),
        success_at: None,
        issue: None,
        scanned: 0,
        truncated: false,
        partial: false,
    };
    let mut opinions = vec![];
    let result = (|| -> Result<(), SourceIssue> {
        let client = public_client()?;
        let cutoff = seconds(at.as_str()).unwrap() - 48 * 3600;
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
            let mut successes = 0;
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
            let comments = match json(&client, comments_url.as_str()) {
                Ok(v) => {
                    successes += 1;
                    v
                }
                Err(e) => {
                    channel.issue = Some(e);
                    Value::Array(vec![])
                }
            };
            let rows = comments.as_array().ok_or(SourceIssue::SchemaChanged)?;
            channel.scanned += rows.len();
            channel.truncated |= rows.len() == 100;
            let mut all = rows.clone();
            let mut issues_url = url::Url::parse("https://api.github.com/search/issues").unwrap();
            issues_url
                .query_pairs_mut()
                .append_pair(
                    "q",
                    &format!("repo:openai/codex is:issue reset in:title,body updated:>{since}"),
                )
                .append_pair("sort", "updated")
                .append_pair("order", "desc")
                .append_pair("per_page", "100");
            let issues = match json(&client, issues_url.as_str()) {
                Ok(v) => {
                    successes += 1;
                    v
                }
                Err(e) => {
                    channel.issue = Some(e);
                    serde_json::json!({"items": []})
                }
            };
            let rows = issues["items"]
                .as_array()
                .ok_or(SourceIssue::SchemaChanged)?;
            channel.scanned += rows.len();
            channel.truncated |= issues["total_count"].as_u64().unwrap_or(0) > rows.len() as u64;
            all.extend(rows.clone());
            // An old reset discussion can have fresh predictions. Fetch its
            // latest page rather than limiting discovery to newly created issues.
            for thread in rows
                .iter()
                .filter(|r| r["comments"].as_u64().unwrap_or(0) > 0)
                .take(3)
            {
                let Some(number) = thread["number"].as_u64() else {
                    continue;
                };
                let count = thread["comments"].as_u64().unwrap_or(0);
                let page = count.div_ceil(100).max(1);
                let url = format!("https://api.github.com/repos/openai/codex/issues/{number}/comments?per_page=100&page={page}");
                let comments = match json(&client, &url) {
                    Ok(v) => {
                        successes += 1;
                        v
                    }
                    Err(e) => {
                        channel.issue = Some(e);
                        continue;
                    }
                };
                let rows = comments.as_array().ok_or(SourceIssue::SchemaChanged)?;
                channel.scanned += rows.len();
                channel.truncated |= page > 1;
                all.extend(rows.clone());
            }
            if successes == 0 {
                return Err(channel.issue.unwrap_or(SourceIssue::RequestFailed));
            }
            channel.partial = channel.issue.is_some();
            let mut seen = HashSet::new();
            for row in &all {
                let (Some(key), Some(author), Some(text), Some(time), Some(link)) = (
                    row["id"].as_u64(),
                    short(&row["user"]["login"], 100),
                    short(&row["body"], 65536),
                    stamp(&row["created_at"]),
                    short(&row["html_url"], 512),
                ) else {
                    continue;
                };
                if !seen.insert(key) {
                    continue;
                }
                if !link.starts_with("https://github.com/openai/codex/") {
                    continue;
                }
                if let Some(p) = opinion(
                    format!("github:{key}"),
                    id,
                    &author,
                    &link,
                    &format!("{}\n{text}", row["title"].as_str().unwrap_or("")),
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
