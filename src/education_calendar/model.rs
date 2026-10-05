use crate::school_portal::model::{date, identifier};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub school_id: String,
    pub tenant_id: String,
    pub academic_year: String,
    pub kind: String,
    pub title: String,
    pub description: String,
    pub start_date: String,
    pub end_date: String,
    pub audience: String,
    pub section_ids: Vec<String>,
}
impl Entry {
    pub fn valid(&self) -> bool {
        [&self.id, &self.school_id, &self.tenant_id]
            .iter()
            .all(|s| identifier(s))
            && !self.academic_year.trim().is_empty()
            && self.academic_year.len() <= 32
            && [
                "academic_year",
                "semester",
                "holiday",
                "exam_period",
                "report_distribution",
                "school_activity",
            ]
            .contains(&self.kind.as_str())
            && !self.title.trim().is_empty()
            && self.title.len() <= 256
            && self.description.len() <= 2000
            && date(&self.start_date)
            && date(&self.end_date)
            && self.start_date <= self.end_date
            && chrono::NaiveDate::parse_from_str(&self.end_date, "%Y-%m-%d")
                .ok()
                .zip(chrono::NaiveDate::parse_from_str(&self.start_date, "%Y-%m-%d").ok())
                .is_some_and(|(e, s)| (e - s).num_days() <= 550)
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
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub entry: Entry,
    pub expected_version: u32,
    pub action: String,
}
impl Change {
    pub fn valid(&self) -> bool {
        self.entry.valid()
            && self.expected_version < i32::MAX as u32
            && ["save_draft", "publish", "archive"].contains(&self.action.as_str())
            && (self.expected_version > 0 || self.action == "save_draft")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_inclusive_dates_and_explicit_audience() {
        let mut e = Entry {
            id: "one".into(),
            school_id: "school".into(),
            tenant_id: "tenant".into(),
            academic_year: "2026/2027".into(),
            kind: "holiday".into(),
            title: "Holiday".into(),
            description: "".into(),
            start_date: "2026-12-24".into(),
            end_date: "2027-01-03".into(),
            audience: "school".into(),
            section_ids: vec![],
        };
        assert!(e.valid());
        e.end_date = "2026-12-23".into();
        assert!(!e.valid());
        e.end_date = "2027-02-30".into();
        assert!(!e.valid());
        e.end_date = "2027-01-03".into();
        e.audience = "sections".into();
        assert!(!e.valid());
        e.section_ids = vec!["class".into()];
        assert!(e.valid());
        e.section_ids.push("class".into());
        assert!(!e.valid());
    }
}
