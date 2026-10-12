//! Repetition analysis: drafts routines from messages a user keeps resending.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, Timelike, Utc};
use openside_proto::{RoutineDraft, RoutineSchedule, SessionId};

use crate::{schedule, ROUTINE_MESSAGE_PREFIX};

/// Fewest occurrences before a repeated prompt becomes a suggestion.
pub const MIN_OCCURRENCES: usize = 3;
/// Word-set overlap above which two messages count as the same request.
const SIMILARITY: f64 = 0.8;
/// Occurrences closer together than this are retries, not repetitions.
const RETRY_WINDOW_SECS: i64 = 300;
/// Largest relative spread of the gaps between occurrences still called "regular".
const MAX_SPREAD: f64 = 0.35;
/// Only the most recent messages are clustered, bounding the quadratic comparison.
pub const MAX_MESSAGES: usize = 5_000;
/// Shorter messages ("yes", "continue") are never treated as tasks.
const MIN_NORMALIZED_CHARS: usize = 8;

/// A user message considered for repetition analysis.
#[derive(Debug, Clone)]
pub struct UserMessage {
    /// Session the message was sent in.
    pub session_id: SessionId,
    /// Idempotency key; messages sent by routines themselves are ignored.
    pub message_id: String,
    /// Original text.
    pub text: String,
    /// Time the message was queued.
    pub at: DateTime<Utc>,
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        // Numbers and dates vary between otherwise identical requests.
        .filter(|word| !word.is_empty() && !word.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

/// Lowercased, punctuation- and number-free form used to compare prompts.
pub fn normalize(text: &str) -> String {
    tokens(text).into_iter().collect::<Vec<_>>().join(" ")
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let shared = a.intersection(b).count();
    let total = a.len() + b.len() - shared;
    if total == 0 {
        0.0
    } else {
        shared as f64 / total as f64
    }
}

struct Cluster<'a> {
    words: BTreeSet<String>,
    members: Vec<&'a UserMessage>,
}

fn median(values: &mut [i64]) -> i64 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn guess_schedule(times: &[DateTime<Utc>], mean_gap: f64) -> RoutineSchedule {
    let near = |target: f64| (mean_gap - target).abs() <= target * 0.2;
    // Median over minute-of-day, so jitter across an hour boundary cannot split hour and minute.
    let minute_of_day = median(
        &mut times
            .iter()
            .map(|t| i64::from(t.hour() * 60 + t.minute()))
            .collect::<Vec<_>>(),
    );
    let (hour, minute) = (minute_of_day / 60, minute_of_day % 60);
    let cron = |expr: String| RoutineSchedule::Cron {
        expr,
        timezone: Some("UTC".into()),
    };
    if near(86_400.0) {
        return cron(format!("{minute} {hour} * * *"));
    }
    if near(7.0 * 86_400.0) {
        let mut counts = BTreeMap::<u32, usize>::new();
        for time in times {
            *counts
                .entry(time.weekday().num_days_from_sunday())
                .or_default() += 1;
        }
        let weekday = counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map_or(0, |(day, _)| day);
        return cron(format!("{minute} {hour} * * {weekday}"));
    }
    for hours in [1_u32, 2, 3, 4, 6, 8, 12] {
        if near(f64::from(hours) * 3_600.0) {
            let hour_field = if hours == 1 {
                "*".to_owned()
            } else {
                format!("*/{hours}")
            };
            return cron(format!("{minute} {hour_field} * * *"));
        }
    }
    RoutineSchedule::Interval {
        secs: (mean_gap.round() as u64).max(1),
    }
}

/// Find prompts the user keeps repeating at roughly regular spacing.
///
/// Messages are grouped by word-set similarity; a group with at least
/// [`MIN_OCCURRENCES`] occurrences whose gaps vary by no more than about a third of
/// their mean becomes a draft. Prompts equal to an entry of `existing_prompts` are
/// skipped. Nothing is persisted.
pub fn suggest(messages: &[UserMessage], existing_prompts: &[String]) -> Vec<RoutineDraft> {
    let existing: BTreeSet<String> = existing_prompts.iter().map(|p| normalize(p)).collect();
    let mut ordered: Vec<&UserMessage> = messages
        .iter()
        .filter(|m| !m.message_id.starts_with(ROUTINE_MESSAGE_PREFIX))
        .collect();
    ordered.sort_by_key(|m| m.at);
    let skip = ordered.len().saturating_sub(MAX_MESSAGES);
    let mut clusters: Vec<Cluster<'_>> = Vec::new();
    for message in ordered.into_iter().skip(skip) {
        let words = tokens(&message.text);
        if words.iter().map(|w| w.chars().count() + 1).sum::<usize>() < MIN_NORMALIZED_CHARS {
            continue;
        }
        match clusters
            .iter_mut()
            .find(|cluster| jaccard(&cluster.words, &words) >= SIMILARITY)
        {
            Some(cluster) => cluster.members.push(message),
            None => clusters.push(Cluster {
                words,
                members: vec![message],
            }),
        }
    }
    let mut drafts = Vec::new();
    for cluster in clusters {
        let mut times: Vec<DateTime<Utc>> = Vec::new();
        for member in &cluster.members {
            if times.last().map_or(true, |last| {
                (member.at - *last).num_seconds() >= RETRY_WINDOW_SECS
            }) {
                times.push(member.at);
            }
        }
        if times.len() < MIN_OCCURRENCES {
            continue;
        }
        let gaps: Vec<f64> = times
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).num_seconds() as f64)
            .collect();
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        let variance = gaps.iter().map(|gap| (gap - mean).powi(2)).sum::<f64>() / gaps.len() as f64;
        if mean < 1.0 || variance.sqrt() / mean > MAX_SPREAD {
            continue;
        }
        let Some(latest) = cluster.members.last() else {
            continue;
        };
        if existing.contains(&normalize(&latest.text)) {
            continue;
        }
        let schedule = guess_schedule(&times, mean);
        if schedule::validate(&schedule, latest.at).is_err() {
            continue;
        }
        let mut sessions = Vec::<SessionId>::new();
        for member in &cluster.members {
            if !sessions.contains(&member.session_id) {
                sessions.push(member.session_id.clone());
            }
        }
        let prompt = latest.text.trim().to_owned();
        drafts.push(RoutineDraft {
            name: prompt.chars().take(48).collect(),
            prompt,
            schedule,
            occurrences: u32::try_from(times.len()).unwrap_or(u32::MAX),
            session_ids: sessions,
            first_seen: times[0],
            last_seen: times[times.len() - 1],
        });
    }
    drafts.sort_by(|a, b| {
        b.occurrences
            .cmp(&a.occurrences)
            .then(b.last_seen.cmp(&a.last_seen))
    });
    drafts
}
