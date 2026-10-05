MATCH (u:User {id:$payer})-[:HAS_APPLICATION]->(l:Lead {lead_id:$lead,tenant_id:$tenant})-[:HAS_STUDENT]->(s:Student {studentId:$student})
WHERE coalesce(u.role,'parent')='parent'
  AND coalesce(u.staffMemberId,'')=''
  AND coalesce(u.staff_member_id,'')=''
  AND all(role IN coalesce(u.roles,[]) WHERE role='parent')
  AND all(role IN coalesce(u.marketingRoles,[]) WHERE role='parent')
  AND NOT (u)-[:STAFF_PROFILE]->() AND NOT (u)-[:STAFF_MEMBER]->()
  AND NOT EXISTS { MATCH (staff:StaffMember) WHERE toLower(staff.email)=toLower(u.email) }
  AND coalesce(u.email,'')<>'' AND toLower(u.email)=toLower(l.email)
  AND (l.email_otp_verified_at IS NOT NULL OR coalesce(l.otp_verified,false)=true
       OR EXISTS { MATCH (proof:ParentLoginChallenge {purpose:'parent_login',userId:u.id,leadId:l.lead_id,email:toLower(u.email),channel:'email',isUsed:true,delivered:true}) })
  AND NOT EXISTS { MATCH (other_user:User {id:$payer}) WHERE other_user<>u }
  AND NOT EXISTS { MATCH (other_student:Student {studentId:$student}) WHERE other_student<>s }
WITH collect(DISTINCT {u:u,l:l,s:s}) AS families WHERE size(families)=1
WITH head(families) AS family
WITH family.u AS u,family.l AS l,family.s AS s
