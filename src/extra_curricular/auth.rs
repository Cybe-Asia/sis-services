pub use super::model::Actor;
use super::{failure, model::Scope, Failure};
use axum::http::{HeaderMap, StatusCode};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
pub async fn actor(
    config: &crate::config::config::AppConfig,
    h: &HeaderMap,
    role: &str,
    s: &Scope,
) -> Result<Actor, Failure> {
    let mut actor_role = role.to_string();
    let id = match role {
        "admin" => {
            let (id, current_role) =
                crate::learning_access::administrator_actor(config, h, &s.tenant_id, &s.school_id)
                    .await?;
            if current_role == "owner" {
                actor_role = current_role;
            }
            id
        }
        "teacher" => {
            let m = crate::school_portal::auth::teacher_in(config, h, &s.school_id, &s.tenant_id)
                .await?;
            if !m.school_ids.contains(&s.school_id) || !m.tenant_ids.contains(&s.tenant_id) {
                return Err(failure(StatusCode::FORBIDDEN));
            }
            if m.is_owner() {
                actor_role = "owner".into();
            }
            m.staff_member_id
        }
        "student" => crate::school_portal::student_identity::actor(config, h).await?,
        "parent" => {
            #[derive(Deserialize)]
            struct Claims {
                sub: String,
                exp: usize,
                role: Option<String>,
                scope: Option<String>,
            }
            let key = &config.jwt_secret;
            if key.len() < 32 {
                return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
            }
            let mut v = Validation::new(Algorithm::HS256);
            v.leeway = 0;
            let c = decode::<Claims>(
                crate::school_portal::auth::bearer(h)?,
                &DecodingKey::from_secret(key.as_bytes()),
                &v,
            )
            .map_err(|_| failure(StatusCode::UNAUTHORIZED))?
            .claims;
            let _ = c.exp;
            if c.scope.is_some()
                || !crate::school_portal::model::identifier(&c.sub)
                || c.role.as_deref().is_some_and(|r| r != role)
                || role == "student" && c.role.as_deref() != Some("student")
            {
                return Err(failure(StatusCode::UNAUTHORIZED));
            }
            c.sub
        }
        _ => return Err(failure(StatusCode::BAD_REQUEST)),
    };
    if (role == "parent") != s.student_id.is_some() && role != "teacher" {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    Ok(Actor {
        id,
        role: actor_role,
    })
}
/// Extracurricular management (ADR 0018 slice 7).
pub const EXTRACURRICULAR_MANAGE: &str = "academic.extracurricular.manage";
/// The Admin path for staff who are not administrators: role `manager` while they hold
/// `academic.extracurricular.manage` at the school.
pub async fn manager(
    graph: &neo4rs::Graph,
    config: &crate::config::config::AppConfig,
    h: &HeaderMap,
    s: &Scope,
) -> Result<Actor, Failure> {
    let (id, role) = crate::learning_access::capable_actor(
        graph,
        config,
        h,
        &s.tenant_id,
        &s.school_id,
        EXTRACURRICULAR_MANAGE,
    )
    .await?;
    if role != "staff" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(Actor {
        id,
        role: "manager".into(),
    })
}
/// `actor`, and on the Admin path a capability holder when the caller is not an administrator.
pub async fn admin_or_manager(
    graph: &neo4rs::Graph,
    config: &crate::config::config::AppConfig,
    h: &HeaderMap,
    role: &str,
    s: &Scope,
) -> Result<Actor, Failure> {
    match actor(config, h, role, s).await {
        Err(e) if role == "admin" && e.0 == StatusCode::FORBIDDEN => manager(graph, config, h, s).await,
        other => other,
    }
}
/// Reused inside every transaction after acquiring the activity lock.
pub fn authority(a: &Actor) -> String {
    match a.role.as_str(){
 "owner"=>format!("MATCH(actor:StaffMember {{id:$actor,membershipStatus:'ACTIVE'}}) WHERE {} WITH DISTINCT actor",crate::school_portal::staff_capability::owner("actor","$school","$tenant")),
 "admin"=>"MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]) WITH DISTINCT actor".into(),
 // ADR 0018 slice 7: a member of the school the route found holding `academic.extracurricular.manage`.
 "manager"=>"MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE NOT 'owner' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} WITH DISTINCT actor".into(),
 "teacher"=>"MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE NOT 'owner' IN coalesce(actor.roles,[]) AND 'teacher' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]) WITH DISTINCT actor".into(),
 "student"=>"MATCH(actor:User {id:$actor,role:'student'}) MATCH(b:LearningStudentBinding {user_id:actor.id,status:'ACTIVE',school_id:$school,tenant_id:$tenant}) MATCH(s:Student {studentId:b.student_id})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(sec:Section {school_id:$school,tenant_id:$tenant,status:'active',academic_year:$year}) WITH actor,collect(DISTINCT s) AS students WHERE size(students)=1 UNWIND students AS student WITH actor,student".into(),
 "parent"=>format!("{} AND u.id=$actor WITH collect(DISTINCT u) AS owners,collect(DISTINCT s) AS children WHERE size(owners)=1 UNWIND children AS student WITH owners[0] AS actor,student WHERE student.studentId=$student MATCH(student)-[:ENROLLED_AS]->(:EnrolledStudent {{status:'active',school_id:$school,tenant_id:$tenant}})-[:ENROLLED_IN]->(:Section {{school_id:$school,tenant_id:$tenant,status:'active',academic_year:$year}}) WITH DISTINCT actor,student",crate::school_portal::repository::PARENT),_=>"WITH 1 AS denied WHERE false".into()}
}
pub const ELIGIBLE:&str="EXISTS { MATCH(student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(sec:Section {school_id:$school,tenant_id:$tenant,status:'active',academic_year:$year}) WHERE sec.section_id IN activity.section_ids }";
pub const ROSTER:&str="MATCH(enrollment:ExtraCurricularEnrollment {activity_key:activity.key,status:'enrolled'}) MATCH(student:Student {studentId:enrollment.student_id}) WHERE EXISTS { MATCH(student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:activity.school_id,tenant_id:activity.tenant_id})-[:ENROLLED_IN]->(sec:Section {school_id:activity.school_id,tenant_id:activity.tenant_id,status:'active',academic_year:activity.academic_year}) WHERE sec.section_id IN activity.section_ids }";
