//! Next calendar event from an iCalendar (.ics) file or URL, such as a calendar's private iCal address.
//! Recurring events are expanded with RRULE/RDATE/EXDATE.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, TimeDelta};
use rrule::{RRuleSet, Tz};
use tracing::{debug, warn};

use super::WidgetHandle;
use super::model::{CalendarEvent, WidgetValue};

const MAX_ICS_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
struct VEvent {
    summary: String,
    /// The recurrence-relevant lines, ready for `RRuleSet` parsing.
    rule_text: String,
    duration: TimeDelta,
    all_day: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub title: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
}

fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        if let Some(cont) = raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(cont);
            }
        } else {
            lines.push(raw.trim_end_matches('\r').to_string());
        }
    }
    lines
}

fn unescape(v: &str) -> String {
    v.replace("\\n", " ").replace("\\N", " ").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\")
}

/// "20261023T090000Z" or "20261023" to a naive timestamp for duration maths.
fn naive(value: &str) -> Option<chrono::NaiveDateTime> {
    let v = value.trim_end_matches('Z');
    chrono::NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .ok()
        .or_else(|| chrono::NaiveDate::parse_from_str(v, "%Y%m%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)))
}

fn parse_events(text: &str) -> Vec<VEvent> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<(String, String)>> = None;
    for line in unfold(text) {
        match line.as_str() {
            "BEGIN:VEVENT" => cur = Some(Vec::new()),
            "END:VEVENT" => {
                if let Some(props) = cur.take()
                    && let Some(ev) = build_event(&props)
                {
                    out.push(ev);
                }
            }
            _ => {
                if let (Some(props), Some((name, value))) = (cur.as_mut(), line.split_once(':')) {
                    props.push((name.to_string(), value.to_string()));
                }
            }
        }
    }
    out
}

fn build_event(props: &[(String, String)]) -> Option<VEvent> {
    let key = |n: &str| props.iter().find(|(k, _)| k == n || k.starts_with(&format!("{n};")));
    if key("STATUS").is_some_and(|(_, v)| v.eq_ignore_ascii_case("CANCELLED")) {
        return None;
    }
    let (start_k, start_v) = key("DTSTART")?;
    let all_day = start_k.contains("VALUE=DATE") && !start_k.contains("VALUE=DATE-TIME");
    let start_n = naive(start_v)?;
    let duration = key("DTEND")
        .and_then(|(_, v)| naive(v))
        .map(|end| end - start_n)
        .filter(|d| *d > TimeDelta::zero())
        .unwrap_or(if all_day { TimeDelta::days(1) } else { TimeDelta::zero() });

    // rrule parses date-only starts as midnight; it rejects VALUE=DATE parameters, so drop them.
    let dtstart = if all_day { format!("DTSTART:{}T000000", start_v.trim()) } else { format!("{start_k}:{start_v}") };
    let mut rule_text = dtstart;
    let mut recurring = false;
    for (k, v) in props {
        let name = k.split(';').next().unwrap_or("");
        if matches!(name, "RRULE" | "RDATE" | "EXDATE") {
            recurring |= name != "EXDATE";
            rule_text.push('\n');
            rule_text.push_str(&format!("{k}:{v}"));
        }
    }
    if !recurring {
        rule_text.push_str("\nRRULE:FREQ=DAILY;COUNT=1");
    }
    let summary = key("SUMMARY").map(|(_, v)| unescape(v)).unwrap_or_else(|| "(no title)".into());
    Some(VEvent { summary, rule_text, duration, all_day })
}

/// The event that is running now or starts next.
pub fn next_occurrence(ics: &str, now: DateTime<Local>) -> Option<Occurrence> {
    let mut best: Option<Occurrence> = None;
    for ev in parse_events(ics) {
        let set: RRuleSet = match ev.rule_text.parse() {
            Ok(s) => s,
            Err(e) => {
                debug!(event = %ev.summary, "skipping event: {e}");
                continue;
            }
        };
        let from = (now - ev.duration - TimeDelta::seconds(1)).with_timezone(&Tz::LOCAL);
        let found = set.after(from).all(1).dates;
        let Some(start) = found.first() else { continue };
        let start = start.with_timezone(&Local);
        let occ = Occurrence { title: ev.summary.clone(), start, end: start + ev.duration, all_day: ev.all_day };
        if occ.end < now || (occ.end == now && ev.duration > TimeDelta::zero()) {
            continue;
        }
        if best.as_ref().is_none_or(|b| occ.start < b.start) {
            best = Some(occ);
        }
    }
    best
}

pub fn describe(occ: &Occurrence, now: DateTime<Local>) -> String {
    if occ.start <= now {
        return "Now".into();
    }
    let mins = (occ.start - now).num_minutes();
    if !occ.all_day && mins < 60 {
        return format!("in {} min", mins.max(1));
    }
    let days = (occ.start.date_naive() - now.date_naive()).num_days();
    let day = match days {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        2..=6 => occ.start.format("%a").to_string(),
        _ => format!("{:02}/{:02}", occ.start.day(), occ.start.month()),
    };
    if occ.all_day { day } else { format!("{day} {}", occ.start.format("%H:%M")) }
}

async fn fetch(source: &str) -> Result<String, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
        let resp = client.get(source).send().await.map_err(|e| format!("calendar fetch failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("calendar fetch failed: HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_ICS_BYTES {
            return Err("calendar file is larger than 8 MB".into());
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        let text = tokio::fs::read_to_string(source).await.map_err(|e| format!("cannot read {source}: {e}"))?;
        if text.len() > MAX_ICS_BYTES {
            return Err("calendar file is larger than 8 MB".into());
        }
        Ok(text)
    }
}

/// Refetches every `refresh`; recomputes the "when" text every 30 s (pushed only when it changes).
pub fn spawn(handle: WidgetHandle, source: String, refresh: Duration) {
    tokio::spawn(async move {
        let mut ics = String::new();
        let mut fetched_at: Option<tokio::time::Instant> = None;
        loop {
            if fetched_at.is_none_or(|t| t.elapsed() >= refresh) {
                match fetch(&source).await {
                    Ok(t) => ics = t,
                    Err(e) => warn!("{e}"),
                }
                fetched_at = Some(tokio::time::Instant::now());
            }
            let now = Local::now();
            let text = ics.clone();
            let value = tokio::task::spawn_blocking(move || {
                next_occurrence(&text, now).map(|o| CalendarEvent { when: describe(&o, now), title: o.title })
            })
            .await
            .unwrap_or(None);
            handle.update(WidgetValue::Calendar(value)).await;
            if handle.is_closed() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn stamp(t: DateTime<Local>) -> String {
        t.with_timezone(&chrono::Utc).format("%Y%m%dT%H%M%SZ").to_string()
    }

    fn ics(events: &[String]) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{}END:VCALENDAR\r\n", events.concat())
    }

    fn event(summary: &str, start: DateTime<Local>, mins: i64, extra: &str) -> String {
        format!(
            "BEGIN:VEVENT\r\nSUMMARY:{summary}\r\nDTSTART:{}\r\nDTEND:{}\r\n{extra}END:VEVENT\r\n",
            stamp(start),
            stamp(start + TimeDelta::minutes(mins))
        )
    }

    #[test]
    fn picks_the_soonest_and_skips_past_and_cancelled() {
        let now = local(2026, 10, 23, 9, 0);
        let text = ics(&[
            event("Past", local(2026, 10, 22, 9, 0), 30, ""),
            event("Later", local(2026, 10, 23, 15, 0), 30, ""),
            event("Cancelled", local(2026, 10, 23, 9, 30), 30, "STATUS:CANCELLED\r\n"),
            event("Soon, with\\, comma", local(2026, 10, 23, 9, 20), 30, ""),
        ]);
        let o = next_occurrence(&text, now).unwrap();
        assert_eq!(o.title, "Soon, with, comma");
        assert_eq!(describe(&o, now), "in 20 min");
    }

    #[test]
    fn expands_weekly_rules_with_exdates() {
        // Weekly on the start's weekday, created a month ago; this week's instance is excluded.
        let first = local(2026, 9, 25, 10, 0);
        let skipped = local(2026, 10, 23, 10, 0);
        let text = ics(&[event("Standup", first, 15, &format!("RRULE:FREQ=WEEKLY\r\nEXDATE:{}\r\n", stamp(skipped)))]);
        let now = local(2026, 10, 23, 8, 0);
        let o = next_occurrence(&text, now).unwrap();
        assert_eq!(o.start, local(2026, 10, 30, 10, 0));
        assert_eq!(describe(&o, now), format!("{:02}/{:02} 10:00", 30, 10));
    }

    #[test]
    fn running_events_show_now_and_all_day_events_have_no_time() {
        let now = local(2026, 10, 23, 9, 10);
        let text = ics(&[event("Meeting", local(2026, 10, 23, 9, 0), 60, "")]);
        assert_eq!(describe(&next_occurrence(&text, now).unwrap(), now), "Now");

        let all_day = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:Holiday\nDTSTART;VALUE=DATE:20261024\n\
                       DTEND;VALUE=DATE:20261025\nEND:VEVENT\nEND:VCALENDAR\n";
        let o = next_occurrence(all_day, now).unwrap();
        assert!(o.all_day);
        assert_eq!(describe(&o, now), "Tomorrow");
    }

    #[test]
    fn folded_lines_and_garbage_are_handled() {
        let now = local(2026, 10, 23, 9, 0);
        let start = stamp(local(2026, 10, 23, 11, 0));
        let text = format!("BEGIN:VEVENT\r\nSUMMARY:Long ti\r\n tle\r\nDTSTART:{start}\r\nEND:VEVENT\r\n");
        assert_eq!(next_occurrence(&text, now).unwrap().title, "Long title");
        assert!(next_occurrence("not a calendar", now).is_none());
        assert!(next_occurrence("BEGIN:VEVENT\nDTSTART:garbage\nEND:VEVENT\n", now).is_none());
    }
}
