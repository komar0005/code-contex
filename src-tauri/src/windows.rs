use crate::model::UsageEvent;
use crate::pricing::{event_cost, PricingTable};
use chrono::{DateTime, Datelike, Duration, DurationRound, Local, TimeZone, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct TokenCost {
    pub tokens: u64,
    pub cost: f64,
    pub unpriced_count: u64,
}

pub fn aggregate<'a>(
    events: impl Iterator<Item = &'a UsageEvent>,
    pricing: &PricingTable,
) -> TokenCost {
    let mut tokens = 0u64;
    let mut cost = 0.0;
    let mut unpriced_count = 0u64;
    for event in events {
        tokens += event.total_tokens();
        match event_cost(event, pricing) {
            Some(c) => cost += c,
            None => unpriced_count += 1,
        }
    }
    TokenCost { tokens, cost, unpriced_count }
}

/// "Hoy"/"mes en curso" use the SYSTEM LOCAL calendar per the design spec —
/// comparing in UTC would make the daily counter roll over mid-evening for
/// anyone west of UTC. The `_in` variants exist so tests can pin an explicit
/// offset instead of depending on the machine's timezone.
pub fn is_same_calendar_day_in<Tz: TimeZone>(a: DateTime<Utc>, b: DateTime<Utc>, tz: &Tz) -> bool {
    let a = a.with_timezone(tz);
    let b = b.with_timezone(tz);
    a.year() == b.year() && a.ordinal() == b.ordinal()
}

pub fn is_same_calendar_day(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    is_same_calendar_day_in(a, b, &Local)
}

pub fn is_same_calendar_month_in<Tz: TimeZone>(
    a: DateTime<Utc>,
    b: DateTime<Utc>,
    tz: &Tz,
) -> bool {
    let a = a.with_timezone(tz);
    let b = b.with_timezone(tz);
    a.year() == b.year() && a.month() == b.month()
}

pub fn is_same_calendar_month(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    is_same_calendar_month_in(a, b, &Local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;
    use crate::pricing::embedded_pricing_table;
    use chrono::FixedOffset;
    use chrono::TimeZone;

    fn utc_minus_4() -> FixedOffset {
        FixedOffset::west_opt(4 * 3600).unwrap()
    }

    fn utc_offset_zero() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn event(model: &str, ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: model.into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn aggregate_sums_tokens_and_tracks_unpriced() {
        let table = embedded_pricing_table();
        let t = Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap();
        let events = vec![event("claude-sonnet-5", t), event("unknown-model", t)];
        let result = aggregate(events.iter(), &table);
        assert_eq!(result.tokens, 2_000_000);
        assert_eq!(result.unpriced_count, 1);
        assert!((result.cost - 3.0).abs() < 1e-9); // only the priced event counts
    }

    #[test]
    fn same_day_at_utc_when_offset_zero() {
        let a = Utc.with_ymd_and_hms(2026, 7, 14, 1, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 14, 23, 59, 0).unwrap();
        assert!(is_same_calendar_day_in(a, b, &utc_offset_zero()));
    }

    #[test]
    fn different_utc_days_can_be_same_local_day() {
        // 23:00Z Jul 14 and 01:00Z Jul 15 are both Jul 14 evening at UTC-4.
        let a = Utc.with_ymd_and_hms(2026, 7, 14, 23, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 15, 1, 0, 0).unwrap();
        assert!(!is_same_calendar_day_in(a, b, &utc_offset_zero()));
        assert!(is_same_calendar_day_in(a, b, &utc_minus_4()));
    }

    #[test]
    fn same_utc_day_can_be_different_local_days() {
        // 01:00Z and 23:00Z on Jul 15 are Jul 14 (21:00) and Jul 15 (19:00) at UTC-4.
        let a = Utc.with_ymd_and_hms(2026, 7, 15, 1, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 15, 23, 0, 0).unwrap();
        assert!(is_same_calendar_day_in(a, b, &utc_offset_zero()));
        assert!(!is_same_calendar_day_in(a, b, &utc_minus_4()));
    }

    #[test]
    fn month_boundary_respects_local_offset() {
        // 02:00Z Aug 1 is still Jul 31 at UTC-4.
        let a = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 8, 1, 2, 0, 0).unwrap();
        assert!(!is_same_calendar_month_in(a, b, &utc_offset_zero()));
        assert!(is_same_calendar_month_in(a, b, &utc_minus_4()));
    }

    #[test]
    fn same_calendar_month_ignores_day() {
        let a = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 31, 23, 0, 0).unwrap();
        assert!(is_same_calendar_month_in(a, b, &utc_offset_zero()));
    }
}

const BLOCK_DURATION_HOURS: i64 = 5;

#[derive(Debug, Clone)]
pub struct Block {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub events: Vec<UsageEvent>,
}

/// Groups `events` into rolling 5h blocks: a new block starts at the first
/// event overall, or at any event whose timestamp is >= 5h after the start
/// of the current block. Later events within that 5h window join the same
/// block even if there's a smaller gap inside it — this mirrors how Claude's
/// plan session windows behave (fixed-length from first message, not
/// extended by later activity). A new block's `start` is floored to the top
/// of the hour (UTC), matching how Anthropic's real 5h windows are anchored.
pub fn compute_blocks(events: &[UsageEvent]) -> Vec<Block> {
    let mut sorted: Vec<&UsageEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.timestamp);
    let block_len = Duration::hours(BLOCK_DURATION_HOURS);
    let mut blocks: Vec<Block> = Vec::new();
    for event in sorted {
        let needs_new_block = match blocks.last() {
            None => true,
            Some(b) => event.timestamp >= b.start + block_len,
        };
        if needs_new_block {
            // Anchor to the top of the hour: Anthropic's real 5h windows
            // start on the hour containing the first message, not at the
            // message's exact timestamp.
            let start = event
                .timestamp
                .duration_trunc(Duration::hours(1))
                .unwrap_or(event.timestamp);
            blocks.push(Block {
                start,
                end: start + block_len,
                events: Vec::new(),
            });
        }
        blocks.last_mut().unwrap().events.push(event.clone());
    }
    blocks
}

/// Returns the block covering `now` (`block.start <= now < block.end`), if any.
pub fn active_block<'a>(blocks: &'a [Block], now: DateTime<Utc>) -> Option<&'a Block> {
    blocks.iter().find(|b| b.start <= now && now < b.end)
}

#[cfg(test)]
mod block_tests {
    use super::*;
    use crate::model::Agent;
    use chrono::TimeZone;

    fn event_at(ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: "claude-sonnet-5".into(),
            input_tokens: 1,
            output_tokens: 1,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn events_within_5h_of_block_start_join_same_block() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![
            event_at(t0),
            event_at(t0 + Duration::hours(1)),
            event_at(t0 + Duration::minutes(4 * 60 + 59)),
        ];
        let blocks = compute_blocks(&events);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].events.len(), 3);
        assert_eq!(blocks[0].start, t0);
        assert_eq!(blocks[0].end, t0 + Duration::hours(5));
    }

    #[test]
    fn event_at_exactly_5h_starts_a_new_block() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![event_at(t0), event_at(t0 + Duration::hours(5))];
        let blocks = compute_blocks(&events);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].start, t0 + Duration::hours(5));
    }

    #[test]
    fn active_block_finds_block_covering_now() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![event_at(t0)];
        let blocks = compute_blocks(&events);
        let now = t0 + Duration::hours(2);
        assert!(active_block(&blocks, now).is_some());
    }

    #[test]
    fn active_block_none_when_last_activity_over_5h_ago() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![event_at(t0)];
        let blocks = compute_blocks(&events);
        let now = t0 + Duration::hours(6);
        assert!(active_block(&blocks, now).is_none());
    }

    #[test]
    fn block_start_floors_to_the_hour() {
        let first_event = Utc.with_ymd_and_hms(2026, 7, 14, 8, 47, 12).unwrap();
        let blocks = compute_blocks(&[event_at(first_event)]);
        assert_eq!(blocks[0].start, Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap());
        assert_eq!(blocks[0].end, Utc.with_ymd_and_hms(2026, 7, 14, 13, 0, 0).unwrap());
    }

    #[test]
    fn fixed_anchor_not_sliding_window() {
        // First event 8:47 -> block [8:00, 13:00). An event at 12:30 (only
        // ~1h45m after the previous event) must still open NO new block; an
        // event at 13:00 must. A sliding-window bug (measuring the gap from
        // the previous event instead of from the block start) would keep
        // 13:00 inside the first block, since 13:00 - 12:30 < 5h.
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 47, 0).unwrap();
        let mid = Utc.with_ymd_and_hms(2026, 7, 14, 12, 30, 0).unwrap();
        let boundary = Utc.with_ymd_and_hms(2026, 7, 14, 13, 0, 0).unwrap();
        let blocks = compute_blocks(&[event_at(t0), event_at(mid), event_at(boundary)]);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].events.len(), 2);
        assert_eq!(blocks[1].start, boundary); // 13:00 floors to itself
    }
}

/// Returns references to every event with `now - 7 days <= timestamp <= now`.
pub fn last_7_days<'a>(events: &'a [UsageEvent], now: DateTime<Utc>) -> Vec<&'a UsageEvent> {
    let cutoff = now - Duration::days(7);
    events
        .iter()
        .filter(|e| e.timestamp >= cutoff && e.timestamp <= now)
        .collect()
}

#[cfg(test)]
mod seven_day_tests {
    use super::*;
    use crate::model::Agent;
    use chrono::TimeZone;

    fn event_at(ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: "claude-sonnet-5".into(),
            input_tokens: 1,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn includes_events_within_window_excludes_older() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![
            event_at(now - Duration::days(1)),
            event_at(now - Duration::days(6) - Duration::hours(23)),
            event_at(now - Duration::days(8)),
        ];
        let result = last_7_days(&events, now);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn boundary_at_exactly_7_days_is_included() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event_at(now - Duration::days(7))];
        assert_eq!(last_7_days(&events, now).len(), 1);
    }
}
