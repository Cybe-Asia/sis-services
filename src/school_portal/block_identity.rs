//! A Parent's Block Identity DID, sent by the Parent app after the wallet is verified on the phone,
//! so school administrators pick it on the Gate access tab instead of typing it.
//! Contract: `docs/PARENT_BLOCK_IDENTITY.md`.
use super::{auth, failure, repository::PARENT_ELIGIBLE, Failure};
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use neo4rs::query;
use serde::Deserialize;
use serde_json::{json, Value};

/// The presentation's challenge: binds the DID holder's signature to this Parent account, so a
/// presentation copied to another account does not verify there.
pub const CHALLENGE_PREFIX: &str = "digital-school:parent-did:";
const MAX_PRESENTATION: usize = 16_384;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Link {
    pub did: String,
    /// The VP JSON as the wallet signed it (a string, so its bytes stay exactly as signed).
    pub verifiable_presentation: String,
}

/// Ed25519 `did:key` as Block Identity issues them.
pub fn is_did_key(did: &str) -> bool {
    did.strip_prefix("did:key:z6Mk").is_some_and(|rest| {
        (40..=60).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() && !b"0OIl".contains(&b))
    })
}

/// Shape checks only. The Ed25519Signature2020 signature needs JSON-LD canonicalisation, which this
/// service does not carry: every reader verifies the stored presentation before using the DID
/// (the admin portal does, with the same code that signs its Block Identity presentations).
pub fn valid(link: &Link, sub: &str) -> bool {
    if !is_did_key(&link.did) || link.verifiable_presentation.len() > MAX_PRESENTATION {
        return false;
    }
    let Ok(vp) = serde_json::from_str::<Value>(&link.verifiable_presentation) else {
        return false;
    };
    let proof = &vp["proof"];
    let method = proof["verificationMethod"].as_str().unwrap_or_default();
    // No credentials: the school needs the DID, never the parent's eKYC data.
    let no_credentials = match &vp["verifiableCredential"] {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        _ => false,
    };
    vp["holder"] == link.did.as_str()
        && proof["type"] == "Ed25519Signature2020"
        && method
            .split_once('#')
            .is_some_and(|(did, _)| did == link.did)
        && proof["challenge"] == format!("{CHALLENGE_PREFIX}{sub}").as_str()
        && proof["proofValue"]
            .as_str()
            .is_some_and(|v| v.starts_with('z'))
        && no_credentials
}

/// `POST /api/leads/v1/me/school/block-identity`: replaces the account's DID. Only a User id token
/// (not a Lead link token) of an eligible Parent with at least one child may link one.
pub async fn link(
    State(s): State<AppState>,
    h: HeaderMap,
    input: Result<Json<Link>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&s.config, &h)?;
    let Json(v) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !valid(&v, &sub) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let q = query(&format!("MATCH(u:User {{id:$sub}})-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(:Student) WHERE {PARENT_ELIGIBLE} WITH DISTINCT u MERGE(u)-[:HAS_BLOCK_IDENTITY]->(b:ParentBlockIdentity) SET b.did=$did,b.presentation=$presentation,b.linked_at=datetime() CREATE(:SchoolPortalAudit {{id:$audit,actor_id:u.id,kind:'block_identity_link',created_at:datetime()}}) RETURN true AS saved"))
        .param("sub", sub)
        .param("did", v.did)
        .param("presentation", v.verifiable_presentation)
        .param("audit", uuid::Uuid::new_v4().to_string());
    let saved = async {
        let mut rows = s.graph.execute(q).await?;
        let saved = rows.next().await?.is_some();
        while rows.next().await?.is_some() {}
        Ok::<_, neo4rs::Error>(saved)
    }
    .await
    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if !saved {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(super::envelope(json!({"saved":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    const DID: &str = "did:key:z6Mksk6pFzcZUxnaeXsuCv4k46FVUVFnhgYtFaFopTFJVBuB";

    fn vp(challenge: &str, credentials: Value) -> String {
        json!({"@context":["https://www.w3.org/ns/credentials/v2"],"type":["VerifiablePresentation"],"holder":DID,"verifiableCredential":credentials,
            "proof":{"type":"Ed25519Signature2020","verificationMethod":format!("{DID}#{}",&DID[8..]),"proofPurpose":"assertionMethod","challenge":challenge,"proofValue":"z2b5f"}}).to_string()
    }
    fn link(did: &str, presentation: String) -> Link {
        Link {
            did: did.into(),
            verifiable_presentation: presentation,
        }
    }

    #[test]
    fn accepts_a_credential_free_presentation_bound_to_this_account() {
        assert!(valid(
            &link(DID, vp("digital-school:parent-did:user-1", json!([]))),
            "user-1"
        ));
        let mut no_list: Value =
            serde_json::from_str(&vp("digital-school:parent-did:user-1", json!([]))).unwrap();
        no_list
            .as_object_mut()
            .unwrap()
            .remove("verifiableCredential");
        assert!(valid(&link(DID, no_list.to_string()), "user-1"));
    }

    #[test]
    fn refuses_other_accounts_other_dids_credentials_and_junk() {
        let good = vp("digital-school:parent-did:user-1", json!([]));
        assert!(!valid(&link(DID, good.clone()), "user-2"));
        assert!(!valid(
            &link(
                "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
                good.clone()
            ),
            "user-1"
        ));
        assert!(!valid(
            &link(
                DID,
                vp("digital-school:parent-did:user-1", json!([{"id":"ekyc"}]))
            ),
            "user-1"
        ));
        assert!(!valid(&link(DID, vp("user-1", json!([]))), "user-1"));
        assert!(!valid(&link(DID, "not json".into()), "user-1"));
        assert!(!valid(
            &link(DID, " ".repeat(MAX_PRESENTATION + 1)),
            "user-1"
        ));
        assert!(!valid(&link("did:web:example.com", good), "user-1"));
    }

    #[test]
    fn did_key_shape() {
        assert!(is_did_key(DID));
        assert!(!is_did_key(
            "did:key:z6Mk0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl"
        ));
        assert!(!is_did_key("did:key:z6Mkshort"));
    }

    /// Parent link then the Gate access read: owners and school admins see linked DIDs of a class
    /// member's Parents; teachers, other classes and unlinked Parents see nothing.
    #[tokio::test]
    #[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
    async fn parent_link_then_admin_read() {
        use crate::handlers::auth_helper::AdminActor;
        use crate::repositories::sis_repository::list_block_identity_parents as read;
        let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
        let g = neo4rs::Graph::new(uri, "neo4j", "parent-otp-fixture-password")
            .await
            .unwrap();
        let t = format!("bi-did-{}", uuid::Uuid::new_v4());
        g.run(query("CREATE(:School {school_id:$t+'-school',tenant_id:$t,bi_test:$t}) CREATE(sec:Section {section_id:$t+'-class',name:'7A',school_id:$t+'-school',tenant_id:$t,status:'active',year_group:'Grade 7',academic_year:'2026/2027',bi_test:$t}) CREATE(other:Section {section_id:$t+'-other',name:'7B',school_id:$t+'-school',tenant_id:$t,status:'active',year_group:'Grade 7',academic_year:'2026/2027',bi_test:$t}) FOREACH(p IN ['mum','dad','solo'] | CREATE(u:User {id:$t+'-'+p,email:p+'@'+$t+'.test',role:'parent',bi_test:$t})-[:HAS_APPLICATION]->(:Lead {lead_id:$t+'-lead-'+p,email:p+'@'+$t+'.test',parent_name:'Parent '+p,status:'verified',bi_test:$t})) WITH sec,other MATCH(mum:Lead {lead_id:$t+'-lead-mum'}),(dad:Lead {lead_id:$t+'-lead-dad'}),(solo:Lead {lead_id:$t+'-lead-solo'}) CREATE(mum)-[:HAS_STUDENT]->(child:Student {studentId:$t+'-child',fullName:'Child',bi_test:$t})-[:ENROLLED_AS]->(:EnrolledStudent {student_id:$t+'-child-p',applicant_student_id:$t+'-child',status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id,academic_year:sec.academic_year,year_group:sec.year_group,bi_test:$t})-[:ENROLLED_IN]->(sec) CREATE(dad)-[:HAS_STUDENT]->(child) CREATE(solo)-[:HAS_STUDENT]->(:Student {studentId:$t+'-peer',fullName:'Peer',bi_test:$t})-[:ENROLLED_AS]->(:EnrolledStudent {student_id:$t+'-peer-p',applicant_student_id:$t+'-peer',status:'active',school_id:other.school_id,tenant_id:other.tenant_id,academic_year:other.academic_year,year_group:other.year_group,bi_test:$t})-[:ENROLLED_IN]->(other) FOREACH(r IN ['school_admin','teacher'] | CREATE(:User {id:$t+'-'+r+'-user',staffMemberId:$t+'-'+r,bi_test:$t})-[:STAFF_MEMBER]->(:StaffMember {id:$t+'-'+r,membershipStatus:'ACTIVE',roles:[r],schoolIds:[$t+'-school'],tenantIds:[$t],teamIds:[],bi_test:$t}))").param("t", t.clone())).await.unwrap();
        let secret = "synthetic-parent-key-at-least-32-bytes";
        let state = AppState {
            graph: g.clone(),
            config: crate::config::config::AppConfig {
                jwt_secret: secret.into(),
                ..Default::default()
            },
            http_client: reqwest::Client::new(),
        };
        let send = |sub: String| {
            let token = jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &json!({"sub":sub,"exp":chrono::Utc::now().timestamp()+600}),
                &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
            )
            .unwrap();
            let mut h = HeaderMap::new();
            h.insert("authorization", format!("Bearer {token}").parse().unwrap());
            let body = Link {
                did: DID.into(),
                verifiable_presentation: vp(&format!("{CHALLENGE_PREFIX}{sub}"), json!([])),
            };
            super::link(State(state.clone()), h, Ok(Json(body)))
        };
        assert!(send(format!("{t}-mum")).await.is_ok());
        assert!(send(format!("{t}-mum")).await.is_ok(), "re-link replaces");
        assert!(send(format!("{t}-solo")).await.is_ok());
        let unknown = send(format!("{t}-nobody")).await.unwrap_err();
        assert_eq!(unknown.0, StatusCode::FORBIDDEN);
        let actor = |r: &str| AdminActor {
            subject: format!("{t}-{r}-user"),
            staff: format!("{t}-{r}"),
            expires: chrono::Utc::now().timestamp() + 600,
        };
        let (class, child) = (format!("{t}-class"), format!("{t}-child"));
        let rows = read(&g, &actor("school_admin"), &class, &child)
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "dad has no DID, solo is another child's parent"
        );
        assert_eq!(rows[0].parent_user_id, format!("{t}-mum"));
        assert_eq!(rows[0].parent_name, "Parent mum");
        assert_eq!(rows[0].did, DID);
        let links: i64 = g
            .execute(
                query("MATCH(:User {id:$u})-[:HAS_BLOCK_IDENTITY]->(b) RETURN count(b) AS n")
                    .param("u", format!("{t}-mum")),
            )
            .await
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap()
            .get("n")
            .unwrap();
        assert_eq!(links, 1);
        assert!(
            read(&g, &actor("school_admin"), &class, &format!("{t}-peer"))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(read(&g, &actor("teacher"), &class, &child).await.is_err());
        g.run(query("MATCH(n) WHERE n.bi_test=$t DETACH DELETE n").param("t", t.clone()))
            .await
            .unwrap();
        g.run(query(
            "MATCH(b:ParentBlockIdentity) WHERE NOT ()-[:HAS_BLOCK_IDENTITY]->(b) DELETE b",
        ))
        .await
        .unwrap();
    }
}
