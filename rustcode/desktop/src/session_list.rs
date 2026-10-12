//! Ordering and time labels for the sidebar's chat list.

use chrono::{DateTime, Datelike, FixedOffset};
use rustcode::controller::SessionChoice;

/// When a chat last had activity: the timestamp of its newest message.
/// `None` for sessions saved before timestamps were recorded.
pub(crate) fn last_activity(session: &SessionChoice) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(session.when.trim()).ok()
}

/// Newest activity first. The chat with a turn in flight leads the list: its
/// saved timestamp is from before the turn started, but it is the one chat
/// where something is happening right now.
pub(crate) fn sort_newest_first(sessions: &mut [&SessionChoice], running: Option<&str>) {
    sessions.sort_by_cached_key(|session| {
        let is_running = running == Some(session.id.as_str());
        std::cmp::Reverse((is_running, last_activity(session)))
    });
}

/// A short "how long ago" label: `now`, `5m`, `3h`, `2d`, then a date.
pub(crate) fn relative_label(then: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let seconds = now.signed_duration_since(then).num_seconds();
    match seconds {
        // A clock that moved backwards still reads as just now.
        ..60 => "now".to_owned(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..604_800 => format!("{}d", seconds / 86_400),
        _ if then.year() == now.year() => then.format("%b %-d").to_string(),
        _ => then.format("%b %-d, %Y").to_string(),
    }
}

/// The sidebar label for a chat's last activity, empty when it is unknown.
pub(crate) fn activity_label(session: &SessionChoice, now: DateTime<FixedOffset>) -> String {
    last_activity(session)
        .map(|then| relative_label(then, now))
        .unwrap_or_default()
}

/// Full detail for the row's tooltip: title, exact time and size.
pub(crate) fn activity_tooltip(session: &SessionChoice) -> String {
    let messages = match session.message_count {
        1 => "1 message".to_owned(),
        count => format!("{count} messages"),
    };
    match last_activity(session) {
        Some(then) => format!(
            "{} · {} · {messages}",
            session.title,
            then.format("%b %-d, %Y %H:%M")
        ),
        None => format!("{} · {messages}", session.title),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, when: &str) -> SessionChoice {
        SessionChoice {
            id: id.to_owned(),
            title: format!("Chat {id}"),
            when: when.to_owned(),
            message_count: 3,
            workspace: None,
        }
    }

    fn at(when: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(when).expect("timestamp")
    }

    #[test]
    fn chats_sort_by_last_activity_with_the_running_chat_first() {
        let old = session("old", "2026-10-10T09:00:00+02:00");
        let new = session("new", "2026-10-12T18:35:01.171875+02:00");
        // The same instant as 17:00+02:00, written in another offset.
        let middle = session("middle", "2026-10-12T15:00:00+00:00");
        let undated = session("undated", "today");
        let mut sessions = vec![&undated, &old, &middle, &new];

        sort_newest_first(&mut sessions, None);
        let order = |sessions: &[&SessionChoice]| {
            sessions
                .iter()
                .map(|session| session.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&sessions), ["new", "middle", "old", "undated"]);

        sort_newest_first(&mut sessions, Some("old"));
        assert_eq!(order(&sessions), ["old", "new", "middle", "undated"]);
    }

    #[test]
    fn activity_label_shortens_recent_times_and_dates_older_ones() {
        let now = at("2026-10-12T18:00:00+02:00");
        let label = |when: &str| relative_label(at(when), now);

        assert_eq!(label("2026-10-12T17:59:30+02:00"), "now");
        assert_eq!(label("2026-10-12T18:05:00+02:00"), "now");
        assert_eq!(label("2026-10-12T17:15:00+02:00"), "45m");
        assert_eq!(label("2026-10-12T12:00:00+02:00"), "6h");
        assert_eq!(label("2026-10-09T18:00:00+02:00"), "3d");
        assert_eq!(label("2026-08-03T10:00:00+02:00"), "Aug 3");
        assert_eq!(label("2025-12-24T10:00:00+01:00"), "Dec 24, 2025");
        assert_eq!(activity_label(&session("a", "today"), now), "");
    }

    #[test]
    fn tooltip_names_the_exact_time_and_message_count() {
        let chat = session("a", "2026-10-12T18:35:01+02:00");
        assert_eq!(
            activity_tooltip(&chat),
            "Chat a · Oct 12, 2026 18:35 · 3 messages"
        );
        assert_eq!(activity_tooltip(&session("b", "")), "Chat b · 3 messages");
    }
}
