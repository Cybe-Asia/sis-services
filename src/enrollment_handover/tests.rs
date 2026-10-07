//! Isolated marker-only graph contract evidence, never primary-runtime acceptance.
use super::{auth::Actor, model::Command, repository};
use neo4rs::{query, Graph};
fn command(mark: &str) -> Command {
    Command {
        idempotency_key: format!("{mark}-request"),
        student_id: format!("{mark}-student"),
        payer_user_id: format!("{mark}-parent"),
        admission_id: format!("{mark}-lead"),
        application_id: format!("{mark}-application"),
        offer_id: format!("{mark}-offer"),
        payment_id: format!("{mark}-payment"),
        offer_revision: 1,
        pricing_snapshot_hash: "a".repeat(64),
        school_id: format!("{mark}-school"),
        tenant_id: format!("{mark}-tenant"),
        section_id: format!("{mark}-section"),
        year_group: "Grade 7".into(),
        academic_year: "2026/2027".into(),
        expected_version: 0,
    }
}
async fn fixture(g: &Graph, mark: &str, c: &Command, a: &Actor) {
    g.run(query("CREATE(staff_user:User {id:$subject,hs_test:$mark})-[:STAFF_MEMBER]->(:StaffMember {id:$actor,membershipStatus:'ACTIVE',roles:['admissions_admin'],schoolIds:[$school],tenantIds:[$tenant],teamIds:[],hs_test:$mark}) CREATE(u:User {id:$payer,email:$email,role:'parent',hs_test:$mark})-[:HAS_APPLICATION]->(l:Lead {lead_id:$lead,email:$email,tenant_id:$tenant,email_otp_verified_at:datetime(),hs_test:$mark})-[:HAS_STUDENT]->(s:Student {studentId:$student,applicantStatus:'enrolment_paid',fullName:'Synthetic graph contract child',hs_test:$mark}) CREATE(u)-[:OWNS_APPLICATION]->(app:Application {application_id:$application,tenant_id:$tenant,hs_test:$mark}) CREATE(l)-[:CONVERTED_TO]->(app) CREATE(app)-[:INCLUDES_STUDENT]->(s) CREATE(s)-[:REQUIRES_DOCUMENT]->(:DocumentRequest {request_type:'application_document_pack',status:'approved',hs_test:$mark}) CREATE(s)-[:HAS_OFFER]->(o:Offer {offer_id:$offer,applicant_student_id:$student,tenant_id:$tenant,target_school_id:$school,target_year_group:'Grade 7',academic_year:'2026/2027',status:'accepted',payment_status:'paid',revision:1,pricing_snapshot_hash:$hash,pricing_snapshot_json:'synthetic-contract-snapshot',terms_hash:'synthetic-terms',hs_test:$mark}) CREATE(o)-[:ACCEPTED_VIA]->(:OfferAcceptance {offer_id:$offer,status:'accepted',responded_by_lead_id:$lead,offer_revision:1,pricing_snapshot_hash:$hash,terms_hash:'synthetic-terms',hs_test:$mark}) CREATE(o)-[:PAID_VIA]->(:Payment {payment_id:$payment,tenant_id:$tenant,lead_id:$lead,status:'paid',payment_type:'offer_due_now',offer_revision:1,pricing_snapshot_hash:$hash,pricing_snapshot_json:'synthetic-contract-snapshot',payment_method:'manual_transfer',currency:'IDR',amount:100,amount_verified:100,paid_at:datetime(),hs_test:$mark}) CREATE(:Section {section_id:$section,name:'Synthetic Section',school_id:$school,tenant_id:$tenant,status:'active',year_group:'Grade 7',academic_year:'2026/2027',hs_test:$mark}) CREATE(:School {school_id:$school,tenant_id:$tenant,hs_test:$mark})")
        .param("mark",mark).param("subject",a.subject.clone()).param("actor",a.staff.clone()).param("payer",c.payer_user_id.clone()).param("email",format!("{mark}@example.test")).param("lead",c.admission_id.clone()).param("student",c.student_id.clone()).param("application",c.application_id.clone()).param("school",c.school_id.clone()).param("tenant",c.tenant_id.clone()).param("offer",c.offer_id.clone()).param("hash",c.pricing_snapshot_hash.clone()).param("payment",c.payment_id.clone()).param("section",c.section_id.clone())).await.unwrap();
}
async fn graph() -> Graph {
    let uri =
        std::env::var("SIS_HANDOVER_TEST_NEO4J_URI").expect("explicit isolated test URI required");
    // These are shared owner runtimes and must never be a test target here.
    assert!(!uri.contains(":3213") && !uri.contains(":3223"));
    Graph::new(
        &uri,
        &std::env::var("SIS_HANDOVER_TEST_NEO4J_USER").unwrap(),
        &std::env::var("SIS_HANDOVER_TEST_NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap()
}
async fn count(g: &Graph, statement: &str, student: &str) -> i64 {
    g.execute(query(statement).param("student", student))
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get("n")
        .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated graph; never primary3213/3223"]
async fn paid_offer_handover_replay_scope_cas_and_no_legacy_repair() {
    let g = graph().await;
    repository::migrate(&g).await.unwrap();
    let mark = format!("hs-{}", uuid::Uuid::new_v4());
    let c = command(&mark);
    let a = Actor {
        subject: format!("{mark}-admin"),
        staff: format!("{mark}-staff"),
        expires: chrono::Utc::now().timestamp() + 600,
    };
    fixture(&g, &mark, &c, &a).await;
    let result=async {
        let first=repository::execute(&g,&a,&c).await.unwrap();
        assert!(first.enrolled_student_id.starts_with("STU-"));assert_ne!(first.enrolled_student_id,c.student_id);
        let retry=repository::execute(&g,&a,&c).await.unwrap();assert_eq!(retry.enrolled_student_id,first.enrolled_student_id);assert_eq!(retry.student_number,first.student_number);
        assert_eq!(count(&g,"MATCH(e:EnrolledStudent {applicant_student_id:$student}) RETURN count(e) AS n",&c.student_id).await,1);
        assert_eq!(count(&g,"MATCH(a:SISEnrollmentAudit {student_id:$student}) RETURN count(a) AS n",&c.student_id).await,1);
        let mut conflict=c.clone();conflict.idempotency_key.push_str("-other");assert!(matches!(repository::execute(&g,&a,&conflict).await,Err(repository::Error::Conflict)));
        let mut wrong=c.clone();wrong.tenant_id.push_str("-foreign");assert!(matches!(repository::execute(&g,&a,&wrong).await,Err(repository::Error::Denied)));
        let mut expired=a.clone();expired.expires=chrono::Utc::now().timestamp()-1;assert!(matches!(repository::execute(&g,&expired,&c).await,Err(repository::Error::Denied)));
        // Revoked enrollment fails replay; no restoring or moving it implicitly.
        g.run(query("MATCH(e:EnrolledStudent {applicant_student_id:$student}) SET e.status='inactive'").param("student",c.student_id.clone())).await.unwrap();
        assert!(matches!(repository::execute(&g,&a,&c).await,Err(repository::Error::Conflict)));
        let mut domain=g.execute(query("MATCH(s:Student {studentId:$student})-[:HAS_OFFER]->(o)-[:PAID_VIA]->(p) RETURN s.applicantStatus AS student_status,o.status AS offer_status,p.status AS payment_status").param("student",c.student_id.clone())).await.unwrap();let row=domain.next().await.unwrap().unwrap();assert_eq!(row.get::<String>("student_status").unwrap(),"handed_to_sis");assert_eq!(row.get::<String>("offer_status").unwrap(),"accepted");assert_eq!(row.get::<String>("payment_status").unwrap(),"paid");
    }.await;
    let cleanup=query("MATCH(n) WHERE n.hs_test=$mark OR (n:EnrolledStudent AND n.applicant_student_id=$student) OR (n:SISEnrollmentHandover AND n.student_id=$student) OR (n:SISEnrollmentAudit AND n.student_id=$student) DETACH DELETE n").param("mark",mark).param("student",c.student_id);
    g.run(cleanup).await.unwrap();
    result
}
#[tokio::test]
#[ignore = "requires isolated graph; never primary3213/3223"]
async fn denial_gates_legacy_preservation_and_queued_revocation() {
    let g = graph().await;
    repository::migrate(&g).await.unwrap();
    let mut scenarios = vec![
        "unpaid",
        "acceptance",
        "contact",
        "staff",
        "section",
        "legacy",
        "queued-payment",
        "queued-staff",
        "queued-expiry",
        "parallel",
    ];
    for scenario in scenarios.drain(..) {
        let mark = format!("hs-{}", uuid::Uuid::new_v4());
        let c = command(&mark);
        let mut a = Actor {
            subject: format!("{mark}-admin"),
            staff: format!("{mark}-staff"),
            expires: chrono::Utc::now().timestamp() + 600,
        };
        if scenario == "queued-expiry" {
            a.expires = chrono::Utc::now().timestamp() + 3;
        }
        fixture(&g, &mark, &c, &a).await;
        match scenario {
            "unpaid" => {
                g.run(
                    query("MATCH(p:Payment {payment_id:$id}) SET p.status='pending'")
                        .param("id", c.payment_id.clone()),
                )
                .await
                .unwrap();
            }
            "acceptance" => {
                g.run(
                    query(
                        "MATCH(o:Offer {offer_id:$id})-[:ACCEPTED_VIA]->(a) SET a.offer_revision=2",
                    )
                    .param("id", c.offer_id.clone()),
                )
                .await
                .unwrap();
            }
            "contact" => {
                g.run(
                    query("MATCH(l:Lead {lead_id:$id}) SET l.email='foreign@example.test'")
                        .param("id", c.admission_id.clone()),
                )
                .await
                .unwrap();
            }
            "staff" => {
                g.run(
                    query("MATCH(a:StaffMember {id:$id}) SET a.roles=['teacher']")
                        .param("id", a.staff.clone()),
                )
                .await
                .unwrap();
            }
            "section" => {
                g.run(
                    query("MATCH(sec:Section {section_id:$id}) SET sec.tenant_id='foreign'")
                        .param("id", c.section_id.clone()),
                )
                .await
                .unwrap();
            }
            "legacy" => {
                g.run(query("MATCH(s:Student {studentId:$student}) CREATE(s)-[:ENROLLED_AS]->(:EnrolledStudent {id:$student,applicant_student_id:$student,status:'active',hs_test:$mark})").param("student",c.student_id.clone()).param("mark",mark.clone())).await.unwrap();
            }
            _ => {}
        }
        if scenario.starts_with("queued-") {
            let mut holding = g.start_txn().await.unwrap();
            holding.run(query("MATCH(sec:Section {section_id:$id}) SET sec.sis_enrollment_lock=coalesce(sec.sis_enrollment_lock,0)+1").param("id",c.section_id.clone())).await.unwrap();
            let (worker_graph, worker_actor, worker_command) = (g.clone(), a.clone(), c.clone());
            let worker = tokio::spawn(async move {
                repository::execute(&worker_graph, &worker_actor, &worker_command).await
            });
            // Wait until B is actually blocked on the held Section lock.
            let mut queued = false;
            for _ in 0..100 {
                let mut rows=g.execute(query("SHOW TRANSACTIONS YIELD currentQuery,status WHERE status CONTAINS 'Blocked' AND currentQuery CONTAINS 'SISEnrollmentHandover' RETURN count(*) AS n")).await.unwrap();
                if rows.next().await.unwrap().unwrap().get::<i64>("n").unwrap() > 0 {
                    queued = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert!(queued, "handover never queued on Section lock");
            match scenario {
                "queued-payment" => {
                    holding
                        .run(
                            query("MATCH(p:Payment {payment_id:$id}) SET p.status='pending'")
                                .param("id", c.payment_id.clone()),
                        )
                        .await
                        .unwrap();
                }
                "queued-staff" => {
                    holding
                        .run(
                            query(
                                "MATCH(a:StaffMember {id:$id}) SET a.membershipStatus='SUSPENDED'",
                            )
                            .param("id", a.staff.clone()),
                        )
                        .await
                        .unwrap();
                }
                "queued-expiry" => {
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                }
                _ => unreachable!(),
            };
            holding.commit().await.unwrap();
            assert!(worker.await.unwrap().is_err());
        } else if scenario == "parallel" {
            let (one, two) = tokio::join!(
                repository::execute(&g, &a, &c),
                repository::execute(&g, &a, &c)
            );
            assert_eq!(
                one.unwrap().enrolled_student_id,
                two.unwrap().enrolled_student_id
            );
        } else {
            assert!(repository::execute(&g, &a, &c).await.is_err());
        }
        assert_eq!(
            count(
                &g,
                "MATCH(a:SISEnrollmentAudit {student_id:$student}) RETURN count(a) AS n",
                &c.student_id
            )
            .await,
            if scenario == "parallel" { 1 } else { 0 }
        );
        if scenario == "legacy" {
            assert_eq!(count(&g,"MATCH(e:EnrolledStudent {applicant_student_id:$student}) WHERE e.student_id IS NULL RETURN count(e) AS n",&c.student_id).await,1);
        }
        g.run(query("MATCH(n) WHERE n.hs_test=$mark OR (n:EnrolledStudent AND n.applicant_student_id=$student) OR (n:SISEnrollmentHandover AND n.student_id=$student) OR (n:SISEnrollmentAudit AND n.student_id=$student) DETACH DELETE n").param("mark",mark).param("student",c.student_id)).await.unwrap();
    }
}

#[test]
fn placement_roles_match_the_execution_guard() {
    let guard = include_str!("current.cypher");
    for role in repository::PLACEMENT_ROLES {
        assert!(guard.contains(&format!("'{role}'")), "{role} missing from current.cypher");
    }
    assert!(repository::PLACEMENT_ROLES.contains(&"admissions_manager"));
    assert!(!repository::PLACEMENT_ROLES.contains(&"admissions_staff"));
}

/// Admissions managers place students school-scoped or unscoped (tenant-wide,
/// as in admission-service); admissions staff, unscoped school admins and
/// managers scoped to another school are refused.
#[tokio::test]
#[ignore = "requires isolated graph; never primary3213/3223"]
async fn admissions_managers_can_place_but_staff_and_foreign_scopes_cannot() {
    let g = graph().await;
    repository::migrate(&g).await.unwrap();
    for (roles, scope, allowed) in [
        (vec!["admissions_manager"], "unscoped", true),
        (vec!["admissions_manager"], "own", true),
        (vec!["admissions_admin"], "unscoped", true),
        (vec!["admissions_manager"], "other", false),
        (vec!["admissions_staff"], "unscoped", false),
        (vec!["admissions_staff"], "own", false),
        (vec!["school_admin"], "unscoped", false),
        (vec!["marketing_manager", "finance_approver"], "unscoped", false),
    ] {
        let mark = format!("hs-{}", uuid::Uuid::new_v4());
        let c = command(&mark);
        let a = Actor {
            subject: format!("{mark}-admin"),
            staff: format!("{mark}-staff"),
            expires: chrono::Utc::now().timestamp() + 600,
        };
        fixture(&g, &mark, &c, &a).await;
        let (schools, tenants): (Vec<String>, Vec<String>) = match scope {
            "own" => (vec![c.school_id.clone()], vec![c.tenant_id.clone()]),
            "other" => (vec![format!("{mark}-other-school")], vec![c.tenant_id.clone()]),
            _ => (vec![], vec![]),
        };
        g.run(
            query("MATCH(s:StaffMember {id:$id}) SET s.roles=$roles, s.schoolIds=$schools, s.tenantIds=$tenants")
                .param("id", a.staff.clone())
                .param("roles", roles.clone())
                .param("schools", schools)
                .param("tenants", tenants),
        )
        .await
        .unwrap();
        let context = super::context::context(&g, &a).await;
        let outcome = repository::execute(&g, &a, &c).await;
        let placed = count(&g, "MATCH(e:EnrolledStudent {applicant_student_id:$student}) RETURN count(e) AS n", &c.student_id).await;
        g.run(query("MATCH(n) WHERE n.hs_test=$mark OR (n:EnrolledStudent AND n.applicant_student_id=$student) OR (n:SISEnrollmentHandover AND n.student_id=$student) OR (n:SISEnrollmentAudit AND n.student_id=$student) DETACH DELETE n").param("mark", mark.clone()).param("student", c.student_id.clone()))
            .await
            .unwrap();
        assert_eq!(outcome.is_ok(), allowed, "{roles:?} {scope}: {outcome:?}");
        assert_eq!(placed, i64::from(allowed), "{roles:?} {scope}");
        if allowed {
            assert!(context.is_ok(), "{roles:?} {scope} context");
        } else if scope != "other" {
            assert!(matches!(context, Err(repository::Error::Denied)), "{roles:?} {scope} context");
        }
    }
}
