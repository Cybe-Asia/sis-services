//! The extracurricular records of one class for its homeroom teacher (audit gap P1b): per
//! enrolled Student and activity, attendance counts, the released coach outcome and the team's
//! match results. Reads only; drafts never leave the coach's own read.
use crate::extra_curricular::model::{Activity, Text};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendance {
    pub present: u32,
    pub late: u32,
    pub absent: u32,
    pub excused: u32,
    /// Meetings with a mark for this Student, including a status this read does not name.
    pub recorded: u32,
}
impl Attendance {
    /// One mark per meeting and Student (the record key includes the meeting).
    pub fn tally<'a>(statuses: impl IntoIterator<Item = &'a str>) -> Self {
        let mut a = Self::default();
        for status in statuses {
            a.recorded += 1;
            match status {
                "present" => a.present += 1,
                "late" => a.late += 1,
                "absent" => a.absent += 1,
                "excused" => a.excused += 1,
                _ => {}
            }
        }
        a
    }
    /// What the Growth report calls "meetings attended": being there, on time or late.
    pub fn attended(&self) -> u32 {
        self.present + self.late
    }
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLevel {
    pub criterion_id: String,
    /// The criterion as the catalogue names it; `None` once the catalogue no longer lists it.
    pub title: Option<Text>,
    pub level: u32,
    /// The level's label from the catalogue, `None` when the level is out of its range.
    pub label: Option<Text>,
}
/// The released skill levels with the catalogue's words next to each number, so a reader who is
/// not the coach sees "Collaboration: Secure" rather than "collaboration: 1".
pub fn skill_levels(activity: &Activity, skills: &[Value]) -> Vec<SkillLevel> {
    skills
        .iter()
        .filter_map(|s| {
            let criterion_id = s["criterionId"].as_str()?.to_string();
            let level = u32::try_from(s["level"].as_u64()?).ok()?;
            let criterion = activity.criteria.iter().find(|c| c.id == criterion_id);
            Some(SkillLevel {
                title: criterion.map(|c| c.title.clone()),
                label: criterion.and_then(|c| c.levels.get(level as usize).cloned()),
                criterion_id,
                level,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extra_curricular::model::Criterion;
    use serde_json::json;
    fn text(s: &str) -> Text {
        Text {
            en: s.into(),
            id: s.into(),
        }
    }
    #[test]
    fn attendance_counts_each_status_and_attended_means_present_or_late() {
        let a = Attendance::tally(["present", "late", "present", "excused", "absent", "unknown"]);
        assert_eq!(
            a,
            Attendance {
                present: 2,
                late: 1,
                absent: 1,
                excused: 1,
                recorded: 6,
            }
        );
        assert_eq!(a.attended(), 3);
        assert_eq!(Attendance::tally([]).attended(), 0);
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            json!({"present":2,"late":1,"absent":1,"excused":1,"recorded":6})
        );
    }
    #[test]
    fn skill_levels_carry_the_catalogue_words_and_survive_a_removed_criterion() {
        let mut activity = Activity {
            presentation: None,
            featured: false,
            competitive: false,
            id: "robotics".into(),
            title: text("Robotics"),
            summary: text("School robotics"),
            category: "stem".into(),
            image: None,
            coach_id: "coach".into(),
            section_ids: vec!["class".into()],
            capacity: 20,
            parent_consent: false,
            published: true,
            meetings: vec![],
            criteria: vec![Criterion {
                id: "collaboration".into(),
                title: text("Collaboration"),
                levels: vec![text("Developing"), text("Secure")],
            }],
        };
        let skills = vec![
            json!({"criterionId":"collaboration","level":1}),
            json!({"criterionId":"collaboration","level":7}),
            json!({"criterionId":"gone","level":0}),
            json!({"level":"not a skill"}),
        ];
        let levels = skill_levels(&activity, &skills);
        assert_eq!(levels.len(), 3, "a malformed skill is skipped, not an error");
        assert_eq!(levels[0].title, Some(text("Collaboration")));
        assert_eq!(levels[0].label, Some(text("Secure")));
        assert_eq!(levels[1].label, None, "level beyond the catalogue");
        assert_eq!(levels[2].title, None, "criterion removed from the catalogue");
        assert_eq!(
            serde_json::to_value(&levels[0]).unwrap(),
            json!({"criterionId":"collaboration","title":{"en":"Collaboration","id":"Collaboration"},"level":1,"label":{"en":"Secure","id":"Secure"}})
        );
        activity.criteria.clear();
        assert!(skill_levels(&activity, &skills)
            .iter()
            .all(|s| s.title.is_none() && s.label.is_none()));
    }
}
