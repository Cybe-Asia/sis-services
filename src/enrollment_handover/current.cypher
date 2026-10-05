MATCH (staff_user:User {id:$subject})-[staff_link:STAFF_MEMBER]->(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'})
WHERE any(role IN coalesce(actor.roles,[]) WHERE role IN ['owner','school_admin','admissions_admin'])
  AND size(coalesce(actor.teamIds,[]))=0
  AND (('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0)
       OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])))
  AND datetime.realtime().epochSeconds<$expires
  AND NOT EXISTS { MATCH(other:User {id:$subject}) WHERE other<>staff_user }
  AND NOT EXISTS { MATCH(other:StaffMember {id:$actor}) WHERE other<>actor }
  AND NOT EXISTS { MATCH(staff_user)-[other:STAFF_MEMBER]->(other_actor) WHERE other<>staff_link }
  AND NOT EXISTS { MATCH(other:User)-[:STAFF_MEMBER]->(actor) WHERE other<>staff_user }
WITH u,l,s,actor
MATCH (u)-[:OWNS_APPLICATION]->(app:Application {application_id:$application,tenant_id:$tenant})<-[:CONVERTED_TO]-(l)
MATCH (app)-[:INCLUDES_STUDENT]->(s)
MATCH (s)-[:HAS_OFFER]->(o:Offer {offer_id:$offer,applicant_student_id:$student,tenant_id:$tenant,target_school_id:$school,status:'accepted',payment_status:'paid',revision:$revision,pricing_snapshot_hash:$hash})
MATCH (o)-[:ACCEPTED_VIA]->(acceptance:OfferAcceptance {offer_id:$offer,status:'accepted',responded_by_lead_id:$lead,offer_revision:$revision,pricing_snapshot_hash:$hash})
MATCH (o)-[:PAID_VIA]->(p:Payment {payment_id:$payment,tenant_id:$tenant,lead_id:$lead,status:'paid',payment_type:'offer_due_now',offer_revision:$revision,pricing_snapshot_hash:$hash})
MATCH (sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant,status:'active',year_group:$year,academic_year:$academic})
MATCH (school:School {school_id:$school,tenant_id:$tenant})
WHERE s.applicantStatus IN ['enrolment_paid','handed_to_sis']
  AND o.target_year_group=$year AND o.academic_year=$academic
  AND coalesce(o.terms_hash,'')<>'' AND acceptance.terms_hash=o.terms_hash
  AND coalesce(o.pricing_snapshot_json,'')<>'' AND p.pricing_snapshot_json=o.pricing_snapshot_json
  AND p.payment_method IN ['doku','manual_transfer'] AND p.currency='IDR' AND p.amount>0
  AND p.paid_at IS NOT NULL
  AND (p.payment_method='doku' OR coalesce(p.amount_verified,0)>=p.amount)
  AND EXISTS { MATCH(s)-[:REQUIRES_DOCUMENT]->(:DocumentRequest {request_type:'application_document_pack',status:'approved'}) }
  AND NOT EXISTS { MATCH(other:Application {application_id:$application}) WHERE other<>app }
  AND NOT EXISTS { MATCH(other:Offer {offer_id:$offer}) WHERE other<>o }
  AND NOT EXISTS { MATCH(other:Payment {payment_id:$payment}) WHERE other<>p }
  AND NOT EXISTS { MATCH(o)-[:ACCEPTED_VIA]->(other:OfferAcceptance) WHERE other<>acceptance }
  AND NOT EXISTS { MATCH(other:Section {section_id:$section}) WHERE other<>sec }
  AND NOT EXISTS { MATCH(other:School {school_id:$school,tenant_id:$tenant}) WHERE other<>school }
WITH DISTINCT u,l,s,app,o,p,sec,school,actor
