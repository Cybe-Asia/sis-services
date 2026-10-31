use crate::school_portal::model::identifier;
use serde::{Deserialize, Serialize};

pub const KINDS: [&str; 2] = ["article", "webinar"];
pub const CATEGORIES: [&str; 4] = ["parenting", "academic", "wellbeing", "school_news"];
pub const ACTIONS: [&str; 4] = ["save_draft", "publish", "unpublish", "archive"];

/// A calendar day as written. Unlike `school_portal::model::date` it has no window around today:
/// a published webinar must stay readable long after it took place.
fn day(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok_and(|d| d.to_string() == value)
}
fn clock(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && [0, 1, 3, 4].iter().all(|i| b[*i].is_ascii_digit())
        && chrono::NaiveTime::parse_from_str(value, "%H:%M").is_ok()
}
/// Where a family joins: an https address on a named host, without credentials.
fn join_address(value: &str) -> bool {
    value.len() <= 2048
        && value.starts_with("https://")
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        && reqwest::Url::parse(value).is_ok_and(|u| {
            u.scheme() == "https"
                && u.username().is_empty()
                && u.password().is_none()
                && u.host_str().is_some_and(|h| h.contains('.'))
        })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Article {
    pub id: String,
    pub school_id: String,
    pub tenant_id: String,
    pub kind: String,
    pub category: String,
    pub title: String,
    pub body: String,
    pub audience: String,
    pub section_ids: Vec<String>,
    /// A webinar's day and school-clock times; an article has none.
    pub date: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub join_url: Option<String>,
    /// The uploaded cover photograph this article shows, if any. An article saved before covers
    /// existed has none.
    #[serde(default)]
    pub cover: Option<String>,
    /// The staff members who host a webinar in the school's own room, beside its Owners. Left out
    /// when there are none, so a reader that predates hosts keeps reading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}
pub const MAX_HOSTS: usize = 10;
impl Article {
    /// The day of a webinar held in the school's own room, the only kind a host opens.
    pub fn room_day(&self) -> Option<&str> {
        (self.kind == "webinar" && self.join_url.is_none())
            .then_some(self.date.as_deref())
            .flatten()
    }
    pub fn valid(&self) -> bool {
        [&self.id, &self.school_id, &self.tenant_id]
            .iter()
            .all(|s| identifier(s))
            && KINDS.contains(&self.kind.as_str())
            && CATEGORIES.contains(&self.category.as_str())
            && !self.title.trim().is_empty()
            && self.title.len() <= 256
            && !self.title.chars().any(char::is_control)
            && !self.body.trim().is_empty()
            && self.body.len() <= 8000
            && !self.body.chars().any(|c| c.is_control() && c != '\n')
            && self.cover.as_deref().is_none_or(identifier)
            && self.hosts.len() <= MAX_HOSTS
            && self.hosts.iter().all(|s| identifier(s))
            && self
                .hosts
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.hosts.len()
            && (self.hosts.is_empty() || self.room_day().is_some())
            && self.section_ids.len() <= 200
            && self.section_ids.iter().all(|s| identifier(s))
            && self
                .section_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.section_ids.len()
            && match self.audience.as_str() {
                "school" => self.section_ids.is_empty(),
                "sections" => !self.section_ids.is_empty(),
                _ => false,
            }
            && match self.kind.as_str() {
                "webinar" => {
                    self.date.as_deref().is_some_and(day)
                        && self.start.as_deref().is_some_and(clock)
                        && self.end.as_deref().is_none_or(|end| {
                            clock(end) && self.start.as_deref().is_some_and(|start| start < end)
                        })
                        && self.join_url.as_deref().is_none_or(join_address)
                }
                _ => {
                    self.date.is_none()
                        && self.start.is_none()
                        && self.end.is_none()
                        && self.join_url.is_none()
                }
            }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub entry: Article,
    pub expected_version: u32,
    pub action: String,
}
impl Change {
    pub fn valid(&self) -> bool {
        self.entry.valid()
            && self.expected_version < i32::MAX as u32
            && ACTIONS.contains(&self.action.as_str())
            && (self.expected_version > 0 || self.action == "save_draft")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn article() -> Article {
        Article {
            id: "one".into(),
            school_id: "school".into(),
            tenant_id: "tenant".into(),
            kind: "article".into(),
            category: "parenting".into(),
            title: "Calm before the examination".into(),
            body: "First paragraph.\n\nSecond paragraph with **emphasis**.".into(),
            audience: "school".into(),
            section_ids: vec![],
            date: None,
            start: None,
            end: None,
            join_url: None,
            cover: None,
            hosts: vec![],
        }
    }
    #[test]
    fn only_a_webinar_in_the_school_room_names_hosts_and_none_is_written_out() {
        let mut w = article();
        w.kind = "webinar".into();
        w.date = Some("2026-11-02".into());
        w.start = Some("19:00".into());
        assert_eq!(w.room_day(), Some("2026-11-02"));
        assert!(!serde_json::to_string(&w).unwrap().contains("hosts"));
        w.hosts = vec!["staff-1".into(), "staff-2".into()];
        assert!(w.valid());
        w.hosts = vec!["staff-1".into(), "staff-1".into()];
        assert!(!w.valid());
        w.hosts = vec!["../x".into()];
        assert!(!w.valid());
        w.hosts = (0..=MAX_HOSTS).map(|i| format!("s{i}")).collect();
        assert!(!w.valid());
        w.hosts = vec!["staff-1".into()];
        w.join_url = Some("https://meet.google.com/abc-defg-hij".into());
        assert!(!w.valid() && w.room_day().is_none());
        let mut a = article();
        a.hosts = vec!["staff-1".into()];
        assert!(!a.valid());
    }
    #[test]
    fn a_cover_is_an_identifier_and_an_earlier_record_has_none() {
        let mut a = article();
        a.cover = Some("3f1c9a52-0d7e-4c55-9a53-0a6f6f2d9b11".into());
        assert!(a.valid());
        a.cover = Some("../other".into());
        assert!(!a.valid());
        let earlier = serde_json::json!({"id":"one","schoolId":"school","tenantId":"tenant","kind":"article","category":"parenting","title":"T","body":"B","audience":"school","sectionIds":[],"date":null,"start":null,"end":null,"joinUrl":null});
        let read: Article = serde_json::from_value(earlier).unwrap();
        assert!(read.valid() && read.cover.is_none());
    }
    #[test]
    fn an_article_has_text_an_audience_and_no_schedule() {
        let mut a = article();
        assert!(a.valid());
        a.body = "  ".into();
        assert!(!a.valid());
        a.body = "Line\r\nbreak".into();
        assert!(!a.valid());
        a.body = "Text".into();
        a.title = "Two\nlines".into();
        assert!(!a.valid());
        a.title = "One line".into();
        a.category = "lecture".into();
        assert!(!a.valid());
        a.category = "academic".into();
        a.audience = "sections".into();
        assert!(!a.valid());
        a.section_ids = vec!["class".into(), "class".into()];
        assert!(!a.valid());
        a.section_ids = vec!["class".into()];
        assert!(a.valid());
        a.date = Some("2026-11-02".into());
        assert!(!a.valid());
        a.date = None;
        a.join_url = Some("https://meet.example.org/abc".into());
        assert!(!a.valid());
    }
    #[test]
    fn a_webinar_needs_a_day_and_a_start_and_keeps_an_old_date() {
        let mut w = article();
        w.kind = "webinar".into();
        assert!(!w.valid());
        w.date = Some("2021-01-11".into());
        assert!(!w.valid());
        w.start = Some("19:00".into());
        assert!(w.valid());
        w.end = Some("19:00".into());
        assert!(!w.valid());
        w.end = Some("20:30".into());
        assert!(w.valid());
        w.date = Some("2026-02-30".into());
        assert!(!w.valid());
        w.date = Some("2026-11-02".into());
        w.start = Some("7:00".into());
        assert!(!w.valid());
        w.start = Some(" 7:00".into());
        assert!(!w.valid());
        w.start = Some("24:00".into());
        assert!(!w.valid());
        w.start = Some("07:00".into());
        assert!(w.valid());
    }
    #[test]
    fn a_join_address_is_https_on_a_named_host_without_credentials() {
        let mut w = article();
        w.kind = "webinar".into();
        w.date = Some("2026-11-02".into());
        w.start = Some("19:00".into());
        for (address, accepted) in [
            ("https://zoom.us/j/123?pwd=abc", true),
            ("https://meet.google.com/abc-defg-hij", true),
            ("http://zoom.us/j/123", false),
            ("https://user:secret@zoom.us/j/123", false),
            ("https://localhost/room", false),
            ("https://zoom.us/j/1 23", false),
            ("javascript:alert(1)", false),
            ("", false),
        ] {
            w.join_url = Some(address.into());
            assert_eq!(w.valid(), accepted, "{address}");
        }
    }
    #[test]
    fn a_first_save_is_a_draft() {
        let mut c = Change {
            entry: article(),
            expected_version: 0,
            action: "publish".into(),
        };
        assert!(!c.valid());
        c.action = "save_draft".into();
        assert!(c.valid());
        c.expected_version = 3;
        c.action = "unpublish".into();
        assert!(c.valid());
        c.action = "delete".into();
        assert!(!c.valid());
    }
}
