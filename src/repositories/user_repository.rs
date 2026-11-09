// Minimal User repository for sis-services. auth-services owns the
// :User label; we only need the one function that walks the User's
// Lead edges for the parent-facing SIS endpoints.
//
// Why it lives here at all: the parent /me/sections, /me/grades,
// /me/attendance endpoints need to return data for EVERY child the
// authenticated parent has, across multiple Lead records (the "add
// another child" flow creates additional Leads sharing an email).
// That traversal starts at the Lead from the JWT and expands to
// siblings via the User edge.

use neo4rs::{query, Graph};

use crate::repositories::lead_repository::RepositoryError;

/// List every lead_id owned by the User that owns `any_lead_id` through
/// `(User)-[:HAS_APPLICATION]->(Lead)`, never by the typed Lead.email.
/// Returns at least the input when
/// the User edge is missing (new-parent cold path), so callers can
/// always treat this as the definitive list to scatter queries over.
pub async fn list_lead_ids_for_user_of_lead(
    graph: &Graph,
    any_lead_id: &str,
) -> Result<Vec<String>, RepositoryError> {
    let q = query(
        "MATCH (seed:Lead {lead_id: $seed}) \
         OPTIONAL MATCH (u:User)-[:HAS_APPLICATION]->(seed) \
         OPTIONAL MATCH (u)-[:HAS_APPLICATION]->(l:Lead) \
         WITH seed, collect(DISTINCT l.lead_id) AS linked \
         RETURN CASE WHEN size(linked) = 0 THEN [seed.lead_id] ELSE linked END AS ids",
    )
    .param("seed", any_lead_id.to_string());
    let mut rs = graph
        .execute(q)
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs
        .next()
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?
    {
        Ok(row.get::<Vec<String>>("ids").unwrap_or_default())
    } else {
        Ok(vec![any_lead_id.to_string()])
    }
}

#[cfg(test)]
mod parent_scope_tests {
    use super::*;
    use crate::repositories::sis_repository::{list_attendance_for_parent, list_grades_for_parent};

    #[tokio::test]
    #[ignore = "requires disposable SIS_PARENT_TEST_BOLT graph (password test)"]
    async fn lead_tokens_never_reach_another_familys_grades_or_attendance() {
        let graph = Graph::new(
            std::env::var("SIS_PARENT_TEST_BOLT").unwrap(),
            "neo4j",
            "test",
        )
        .await
        .unwrap();
        let run = uuid::Uuid::new_v4().simple().to_string();
        let (victim_lead, squat) = (format!("LEAD-V-{run}"), format!("LEAD-S-{run}"));
        let user = format!("u-{run}");
        graph
            .run(
                query(
                    "CREATE (sec:Section {section_id:$run}) \
                     CREATE (:User {id:$user, email:$email, role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:$victim, email:$email, status:'verified'}) \
                       -[:HAS_STUDENT]->(:Student {studentId:$run, fullName:'Kid'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active'})-[:ENROLLED_IN]->(sec) \
                     CREATE (:GradeEntry {section_id:$run, applicant_student_id:$run, subject:'Math', score:90.0, max_score:100.0, recorded_at:datetime()}) \
                     CREATE (:AttendanceRecord {section_id:$run, applicant_student_id:$run, date:'2026-01-05', status:'present'}) \
                     CREATE (:Lead {lead_id:$squat, email:toUpper($email), status:'new'})",
                )
                .param("run", run.clone())
                .param("user", user.clone())
                .param("email", format!("victim-{run}@example.test"))
                .param("victim", victim_lead.clone())
                .param("squat", squat.clone()),
            )
            .await
            .unwrap();
        let (from, to) = ("2000-01-01", "2100-01-01");

        // The squatted lead only expands to itself and reads nothing.
        let squat_leads = list_lead_ids_for_user_of_lead(&graph, &squat)
            .await
            .unwrap();
        assert_eq!(squat_leads, vec![squat.clone()]);
        assert!(list_grades_for_parent(&graph, &squat_leads, &squat)
            .await
            .unwrap()
            .is_empty());
        // Even handed the victim's lead id, a principal that is not its parent reads nothing.
        let victim_leads = vec![victim_lead.clone()];
        assert!(list_grades_for_parent(&graph, &victim_leads, &squat)
            .await
            .unwrap()
            .is_empty());
        assert!(
            list_attendance_for_parent(&graph, &victim_leads, &squat, from, to)
                .await
                .unwrap()
                .is_empty()
        );
        // The parent still reads their child.
        assert_eq!(
            list_grades_for_parent(&graph, &victim_leads, &user)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            list_attendance_for_parent(&graph, &victim_leads, &user, from, to)
                .await
                .unwrap()
                .len(),
            1
        );

        graph
            .run(query("MATCH (n) WHERE n.section_id = $run OR n.studentId = $run OR n.id = $user OR n.lead_id IN [$v, $s] OR (n:EnrolledStudent AND NOT (n)--()) DETACH DELETE n")
                .param("run", run).param("user", user).param("v", victim_lead).param("s", squat))
            .await
            .unwrap();
    }
}
