use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrowthReport {
    pub term: String,
    pub academic: Option<Academic>,
    pub attendance: Option<Attendance>,
    pub extracurricular: Vec<Club>,
    pub comparisons: Vec<Comparison>,
    pub analysis: Option<String>,
    pub teacher_note: Option<String>,
    pub portfolio: Vec<Artifact>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Academic {
    pub average: f64,
    pub maximum: f64,
    pub subject_count: u32,
    pub mastery_threshold: f64,
    pub mastered_count: u32,
    pub rating: Option<String>,
    pub top_performer: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attendance {
    pub present: u32,
    pub sick: u32,
    pub excused: u32,
    pub absent: u32,
    pub active_days: u32,
    pub exemplary: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Club {
    pub name: String,
    pub active: bool,
    pub critical_thinking: Option<f64>,
    pub creativity: Option<f64>,
    pub attended: u32,
    pub meetings: u32,
    pub exemplary: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    pub area: String,
    pub score: f64,
    pub median: f64,
    pub maximum: f64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub title: String,
    pub subject: String,
    pub summary: String,
    pub format: String,
    pub score: Option<f64>,
    pub maximum: Option<f64>,
    pub award: Option<String>,
    pub document_id: Option<String>,
}
fn bounded(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit
}
fn optional(value: &Option<String>, limit: usize) -> bool {
    value.as_ref().is_none_or(|s| bounded(s, limit))
}
fn score(value: f64, max: f64) -> bool {
    value.is_finite()
        && max.is_finite()
        && max > 0.0
        && max <= 10000.0
        && value >= 0.0
        && value <= max
}
impl GrowthReport {
    pub fn valid(&self) -> bool {
        serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= 480_000)
            && bounded(&self.term, 128)
            && self.academic.as_ref().is_none_or(|a| {
                score(a.average, a.maximum)
                    && score(a.mastery_threshold, a.maximum)
                    && a.subject_count > 0
                    && a.subject_count <= 100
                    && a.mastered_count <= a.subject_count
                    && optional(&a.rating, 128)
            })
            && self.attendance.as_ref().is_none_or(|a| {
                a.active_days > 0
                    && a.active_days <= 366
                    && u64::from(a.present)
                        + u64::from(a.sick)
                        + u64::from(a.excused)
                        + u64::from(a.absent)
                        == u64::from(a.active_days)
            })
            && self.extracurricular.len() <= 20
            && self.extracurricular.iter().all(|a| {
                bounded(&a.name, 256)
                    && a.meetings <= 366
                    && a.attended <= a.meetings
                    && a.critical_thinking.is_none_or(|v| score(v, 100.0))
                    && a.creativity.is_none_or(|v| score(v, 100.0))
            })
            && self.comparisons.len() <= 20
            && self.comparisons.iter().all(|a| {
                bounded(&a.area, 256) && score(a.score, a.maximum) && score(a.median, a.maximum)
            })
            && optional(&self.analysis, 4000)
            && optional(&self.teacher_note, 4000)
            && self.portfolio.len() <= 50
            && self.portfolio.iter().all(|a| {
                bounded(&a.title, 256)
                    && bounded(&a.subject, 128)
                    && bounded(&a.summary, 4000)
                    && bounded(&a.format, 256)
                    && optional(&a.award, 128)
                    && a.document_id.as_ref().is_none_or(|s| super::identifier(s))
                    && match (a.score, a.maximum) {
                        (None, None) => true,
                        (Some(s), Some(m)) => score(s, m),
                        _ => false,
                    }
            })
            && (self.academic.is_some()
                || self.attendance.is_some()
                || !self.extracurricular.is_empty()
                || !self.comparisons.is_empty()
                || self.analysis.is_some()
                || self.teacher_note.is_some()
                || !self.portfolio.is_empty())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_published_metrics_without_inventing_missing_values() {
        let mut report = GrowthReport {
            term: "Term 1".into(),
            academic: None,
            attendance: Some(Attendance {
                present: 68,
                sick: 1,
                excused: 0,
                absent: 0,
                active_days: 69,
                exemplary: false,
            }),
            extracurricular: vec![],
            comparisons: vec![],
            analysis: None,
            teacher_note: None,
            portfolio: vec![],
        };
        assert!(report.valid());
        report.attendance.as_mut().unwrap().active_days = 68;
        assert!(!report.valid());
        report.attendance = None;
        report.comparisons.push(Comparison {
            area: "STEM".into(),
            score: 91.3,
            median: 79.2,
            maximum: 100.0,
        });
        assert!(report.valid());
        report.comparisons[0].median = f64::NAN;
        assert!(!report.valid());
    }
}
