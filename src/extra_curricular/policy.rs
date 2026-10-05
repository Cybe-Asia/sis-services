//! Pure school activity policy. No HTTP, Auth client, or database dependencies.
use super::{model::*, ports::*};
pub fn catalog(
    current: Option<&StoredActivity>,
    next: &Activity,
    expected: u32,
    occupied: u32,
    history: u32,
) -> bool {
    current.map_or(0, |c| c.revision) == expected
        && occupied <= next.capacity
        && current.is_none_or(|c| {
            history == 0
                || (c.activity.criteria == next.criteria
                    && c.activity.meetings == next.meetings
                    && c.activity.section_ids == next.section_ids
                    && c.activity.parent_consent == next.parent_consent)
        })
}
pub fn enrollment(
    role: &str,
    current: &StoredActivity,
    old: Option<&Enrollment>,
    command: &Command,
    expected: u32,
    occupied: u32,
) -> Option<String> {
    if !matches!(role, "parent" | "student") {
        return None;
    }
    let status = old.map(|e| e.status.as_str()).unwrap_or("");
    let revision = old.map_or(0, |e| e.revision);
    let a = &current.activity;
    match command {
        Command::Enroll {}
            if a.published
                && !current.archived
                && current.revision == expected
                && matches!(status, "" | "withdrawn" | "rejected")
                && occupied < a.capacity =>
        {
            Some(
                if a.parent_consent {
                    "pending_consent"
                } else {
                    "enrolled"
                }
                .into(),
            )
        }
        Command::Approve {}
            if role == "parent"
                && a.parent_consent
                && !current.archived
                && a.published
                && status == "pending_consent"
                && revision == expected =>
        {
            Some("enrolled".into())
        }
        Command::Reject {}
            if role == "parent" && status == "pending_consent" && revision == expected =>
        {
            Some("rejected".into())
        }
        Command::Withdraw {}
            if matches!(status, "enrolled" | "pending_consent") && revision == expected =>
        {
            Some("withdrawn".into())
        }
        _ => None,
    }
}
pub fn skills(activity: &Activity, skills: &[Skill]) -> bool {
    skills.len() == activity.criteria.len()
        && skills.iter().all(|s| {
            activity
                .criteria
                .iter()
                .find(|c| c.id == s.criterion_id)
                .is_some_and(|c| (s.level as usize) < c.levels.len())
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_or_unknown_criteria_never_produce_an_award() {
        let text = Text {
            en: "Activity".into(),
            id: "Kegiatan".into(),
        };
        let a = Activity {
            presentation: None,
            id: "a".into(),
            title: text.clone(),
            summary: text.clone(),
            category: "stem".into(),
            image: None,
            coach_id: "coach".into(),
            section_ids: vec!["class".into()],
            capacity: 1,
            parent_consent: true,
            published: true,
            meetings: vec![],
            criteria: vec![Criterion {
                id: "skill".into(),
                title: text.clone(),
                levels: vec![text.clone(), text],
            }],
        };
        assert!(!skills(&a, &[]));
        assert!(!skills(
            &a,
            &[Skill {
                criterion_id: "unknown".into(),
                level: 0
            }]
        ));
        assert!(!skills(
            &a,
            &[Skill {
                criterion_id: "skill".into(),
                level: 2
            }]
        ));
        assert!(skills(
            &a,
            &[Skill {
                criterion_id: "skill".into(),
                level: 1
            }]
        ));
    }
}
