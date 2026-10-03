use super::*;

fn shares(c: &[&Opinion]) -> (usize, usize, usize, Option<f64>) {
    let positive = c.iter().filter(|p| p.stance == "optimistic").count();
    let negative = c.iter().filter(|p| p.stance == "pessimistic").count();
    let uncertain = c.iter().filter(|p| p.stance == "uncertain").count();
    let n = positive + negative + uncertain;
    (
        positive,
        uncertain,
        negative,
        (n > 0).then(|| 100.0 * positive as f64 / n as f64),
    )
}
fn sample(opinions: &[Opinion], end: i64) -> (Vec<&Opinion>, usize) {
    let mut sorted: Vec<_> = opinions
        .iter()
        .filter(|p| seconds(&p.published_at).is_some_and(|t| t <= end && t > end - 6 * 3600))
        .collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(seconds(&p.published_at)));
    let mut authors = HashSet::new();
    let mut texts = HashSet::new();
    let mut count = 0;
    sorted.retain(|p| {
        let author = format!("{}:{}", p.channel, p.author.to_lowercase());
        let normalized: String = p
            .text
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        let unique = authors.insert(author) && texts.insert(normalized);
        if !unique {
            count += 1;
        }
        unique
    });
    (sorted, count)
}
fn adjustment(base: f64, score: f64) -> f64 {
    // Bounded log-odds tilt. Coefficients are explicit priors for the v1
    // experiment, not learned weights or a claim of forecast calibration.
    let p = (base / 100.0).clamp(0.001, 0.999);
    (100.0 / (1.0 + (-(p / (1.0 - p)).ln() - score).exp())).clamp(0.0, 100.0)
}
fn round(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub(super) fn evaluate(view: &mut RadarView, at: &UtcTimestamp) {
    let time = seconds(at.as_str()).unwrap();
    view.community.opinions.retain(|p| {
        seconds(&p.published_at).is_some_and(|t| t <= time + 60 && t > time - 12 * 3600)
    });
    let (current, duplicates) = sample(&view.community.opinions, time);
    let (previous, _) = sample(&view.community.opinions, time - 6 * 3600);
    let (positive, uncertain, negative, share) = shares(&current);
    let previous_share = shares(&previous).3;
    let authors = current.len();
    let communities = current
        .iter()
        .map(|p| &p.channel)
        .collect::<HashSet<_>>()
        .len();
    let wishes = current.iter().filter(|p| p.stance == "wish").count();
    let observations = current.iter().filter(|p| p.stance == "observation").count();
    let facts: HashSet<_> = view
        .forecasts
        .iter()
        .flat_map(|f| f.evidence_urls.iter())
        .collect();
    let fresh_channels: HashSet<_> = view
        .community
        .channels
        .iter()
        .filter(|c| {
            c.issue.is_none()
                && c.success_at
                    .as_ref()
                    .and_then(|s| seconds(s))
                    .is_some_and(|t| (0..1800).contains(&(time - t)))
        })
        .map(|c| c.id.as_str())
        .collect();
    let last_reset = view
        .forecasts
        .iter()
        .find(|f| f.id == "codex_reset")
        .and_then(|f| f.last_reset_at.as_ref())
        .and_then(|s| seconds(s));
    let independent: Vec<_> = current
        .iter()
        .filter(|p| {
            fresh_channels.contains(p.channel.as_str())
                && matches!(p.stance.as_str(), "optimistic" | "pessimistic")
                && p.horizon_hours.is_some()
                && last_reset.is_none_or(|reset| seconds(&p.published_at).unwrap_or(0) > reset)
                && !p.evidence_urls.iter().any(|u| facts.contains(u))
                && !p.text.to_lowercase().contains("tibo")
                && !p.text.contains("thsottiaux")
        })
        .copied()
        .collect();
    let independent_authors = independent.len();
    let community_active = independent_authors >= 3;
    let mut grouped: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in view.forecasts.iter_mut().enumerate() {
        f.weight = 0.0;
        f.exclusion = if f.probability_24h.is_none() || f.probability_48h.is_none() {
            Some("unavailable")
        } else if f.issue.is_some() {
            Some("fetch_failed")
        } else if last_reset
            .zip(f.last_reset_at.as_ref().and_then(|s| seconds(s)))
            .is_some_and(|(latest, s)| latest - s > 12 * 3600)
        {
            Some("reset_mismatch")
        } else if seconds(&f.collected_at).is_none_or(|t| !(0..1800).contains(&(time - t))) {
            Some("stale")
        } else if f
            .updated_at
            .as_ref()
            .and_then(|s| seconds(s))
            .is_some_and(|t| {
                !(-60..if f.id == "reset_monitor" {
                    8 * 3600
                } else {
                    1800
                })
                    .contains(&(time - t))
            })
        {
            Some("stale")
        } else if community_active && (f.uses_community || f.method == "mixed") {
            Some("community_overlap")
        } else {
            None
        }
        .map(str::to_owned);
        if f.exclusion.is_none() {
            grouped.entry(f.method.clone()).or_default().push(i);
        }
    }
    // Convex pooling averages opinions. Sources sharing a method divide that
    // method's weight; copying a radar never multiplies the odds of its claim.
    for (method, indices) in &grouped {
        let budget = match method.as_str() {
            "cadence" => 0.50,
            "statements" => 0.35,
            _ => 0.15,
        };
        let mut opinions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for i in indices {
            let f = &view.forecasts[*i];
            let mut evidence = f.evidence_urls.clone();
            evidence.sort();
            evidence.dedup();
            let signature = format!(
                "{:?}:{:?}:{:?}",
                f.probability_24h, f.probability_48h, evidence
            );
            opinions.entry(signature).or_default().push(*i);
        }
        let per_opinion = budget / opinions.len() as f64;
        for copies in opinions.values() {
            for i in copies {
                view.forecasts[*i].weight = per_opinion / copies.len() as f64;
            }
        }
    }
    let weight_sum: f64 = view.forecasts.iter().map(|f| f.weight).sum();
    let mut pooled = [0.0; 2];
    let mut low = [100.0_f64; 2];
    let mut high = [0.0_f64; 2];
    let mut count = 0;
    let mut groups = HashSet::new();
    for f in &mut view.forecasts {
        if f.weight == 0.0 {
            continue;
        }
        f.weight /= weight_sum;
        count += 1;
        let values = [f.probability_24h.unwrap(), f.probability_48h.unwrap()];
        for h in 0..2 {
            pooled[h] += f.weight * values[h];
            low[h] = low[h].min(values[h]);
            high[h] = high[h].max(values[h]);
        }
        groups.insert("history".to_owned());
        for u in &f.evidence_urls {
            groups.insert(u.clone());
        }
        if f.method == "mixed" {
            groups.insert(format!("undisclosed:{}", f.id));
        }
    }
    let tilt = |hours: u32| {
        let views: Vec<_> = independent
            .iter()
            .filter(|p| p.horizon_hours.is_some_and(|h| h <= hours))
            .collect();
        let n = views.len();
        if n < 3 {
            return 0.0;
        }
        let balance = views
            .iter()
            .map(|p| if p.stance == "optimistic" { 1.0 } else { -1.0 })
            .sum::<f64>()
            / n as f64;
        let coverage = (views
            .iter()
            .map(|p| &p.channel)
            .collect::<HashSet<_>>()
            .len() as f64
            / 2.0)
            .min(1.0);
        0.35 * balance * n as f64 / (n as f64 + 20.0) * coverage
    };
    let p24 = if community_active {
        adjustment(pooled[0], tilt(24))
    } else {
        pooled[0]
    };
    let p48 = if community_active {
        adjustment(pooled[1], tilt(48)).max(p24)
    } else {
        pooled[1]
    };
    view.estimate = (count > 0).then(|| Estimate {
        probability_24h: round(p24),
        probability_48h: round(p48),
        pooled_24h: round(pooled[0]),
        pooled_48h: round(pooled[1]),
        community_adjustment_24h: round(p24 - pooled[0]),
        community_adjustment_48h: round(p48 - pooled[1]),
        source_count: count,
        spread_24h: round(high[0] - low[0]),
        spread_48h: round(high[1] - low[1]),
        evidence_groups: groups.len(),
        model_version: MODEL_VERSION.into(),
    });
    let c = &mut view.community;
    c.optimistic = positive;
    c.uncertain = uncertain;
    c.pessimistic = negative;
    c.optimistic_share = share;
    c.previous_share = previous_share;
    c.authors = authors;
    c.communities = communities;
    c.duplicates = duplicates;
    c.wishes = wishes;
    c.observations = observations;
    c.independent_authors = independent_authors;
}
