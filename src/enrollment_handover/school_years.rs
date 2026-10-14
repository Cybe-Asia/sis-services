//! The years of a child's school, stored on `EnrolledStudent` when it is placed: the year it joins (`entry_year`) and
//! the year it finishes the school (`graduation_year`). Families read both through `/me/sections`.
use neo4rs::{query, Graph};

/// The last grade of the school: IISS (SMP) ends with grade 9, IIHS (SMA) with 12; another school with the end of the
/// grade's own stage.
fn final_grade(school_id: &str, grade: i64) -> i64 {
    match school_id {
        "SCH-IISS" => 9,
        "SCH-IIHS" => 12,
        _ if grade <= 6 => 6,
        _ if grade <= 9 => 9,
        _ => 12,
    }
}

/// The first number in a year group ("Grade 7", "year7", "Kelas 10"), when it is a grade from 1 to 12.
fn grade(year_group: &str) -> Option<i64> {
    let digits: String = year_group
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok().filter(|g| (1..=12).contains(g))
}

/// The first calendar year of an academic year: "2026/2027" → 2026, and a section that names only its first year
/// ("2026", as on staging) → 2026.
fn entry_year(academic_year: &str) -> Option<i64> {
    let academic_year = academic_year.trim();
    let (start, end) = match academic_year.split_once('/') {
        Some((start, end)) => (start, Some(end)),
        None => (academic_year, None),
    };
    let start: i64 = start.parse().ok().filter(|_| start.len() == 4)?;
    match end {
        Some(end) => (end.parse::<i64>().ok()? == start + 1).then_some(start),
        None => Some(start),
    }
}

/// Entry and graduation year of a child placed in `year_group` for `academic_year`: one grade a year, finishing in
/// the second calendar year of the last grade (grade 7 of IISS in 2026/2027 → 2029). None when either is unreadable
/// or the grade is past the school's last.
pub(super) fn school_years(school_id: &str, year_group: &str, academic_year: &str) -> Option<(i64, i64)> {
    let (start, grade) = (entry_year(academic_year)?, grade(year_group)?);
    let last = final_grade(school_id, grade);
    (grade <= last).then_some((start, start + (last - grade) + 1))
}

/// Fills the years of children placed before they were stored, from the section each was placed in. Only empty
/// values are written, so it can run on every start.
pub async fn backfill(graph: &Graph) -> Result<usize, neo4rs::Error> {
    let mut rows = graph
        .execute(query(
            "MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section) WHERE e.graduation_year IS NULL \
             RETURN e.student_id AS id, coalesce(e.school_id, sec.school_id, '') AS school, \
               coalesce(e.year_group, sec.year_group, '') AS year, coalesce(sec.academic_year, '') AS academic LIMIT 5000",
        ))
        .await?;
    let mut updates = Vec::new();
    while let Some(row) = rows.next().await? {
        let (Ok(id), Ok(school), Ok(year), Ok(academic)) = (
            row.get::<String>("id"),
            row.get::<String>("school"),
            row.get::<String>("year"),
            row.get::<String>("academic"),
        ) else {
            continue;
        };
        if let Some((entry, graduation)) = school_years(&school, &year, &academic) {
            updates.push(neo4rs::BoltType::from(std::collections::HashMap::from([
                ("id".to_string(), neo4rs::BoltType::from(id)),
                ("entry".to_string(), neo4rs::BoltType::from(entry)),
                ("graduation".to_string(), neo4rs::BoltType::from(graduation)),
            ])));
        }
    }
    let count = updates.len();
    if count > 0 {
        graph
            .run(
                query(
                    "UNWIND $rows AS row MATCH (e:EnrolledStudent {student_id: row.id}) WHERE e.graduation_year IS NULL \
                     SET e.entry_year = coalesce(e.entry_year, row.entry), e.graduation_year = row.graduation",
                )
                .param("rows", updates),
            )
            .await?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::school_years;

    #[test]
    fn counts_the_years_from_the_grade_and_academic_year_of_the_placement() {
        assert_eq!(school_years("SCH-IISS", "Grade 7", "2026/2027"), Some((2026, 2029)));
        assert_eq!(school_years("SCH-IISS", "year9", "2026/2027"), Some((2026, 2027)));
        assert_eq!(school_years("SCH-IISS", "year7", "2026"), Some((2026, 2029)));
        assert_eq!(school_years("SCH-IIHS", "Grade 10", "2025/2026"), Some((2025, 2028)));
        assert_eq!(school_years("SCH-IIBS", "Kelas 8", "2026/2027"), Some((2026, 2028)));
        assert_eq!(school_years("SCH-IIBS", "Year 11", "2026/2027"), Some((2026, 2028)));
    }

    #[test]
    fn stores_nothing_it_cannot_read() {
        assert_eq!(school_years("SCH-IISS", "Grade 7", "26"), None);
        assert_eq!(school_years("SCH-IISS", "Grade 7", "2026/"), None);
        assert_eq!(school_years("SCH-IISS", "Grade 7", "2026/2028"), None);
        assert_eq!(school_years("SCH-IISS", "Playgroup", "2026/2027"), None);
        assert_eq!(school_years("SCH-IISS", "Grade 10", "2026/2027"), None);
        assert_eq!(school_years("SCH-IIHS", "Grade 13", "2026/2027"), None);
    }
}
