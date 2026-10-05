MATCH(h:SISEnrollmentHandover {student_id:$student})
OPTIONAL MATCH(existing:EnrolledStudent) WHERE existing.applicant_student_id=$student OR EXISTS {MATCH(s)-[:ENROLLED_AS]->(existing)}
WITH u,l,s,app,o,p,sec,school,actor,h,collect(DISTINCT existing) AS enrollments
WHERE (h.version=0 AND size(enrollments)=0 AND s.applicantStatus='enrolment_paid')
   OR (h.version=1 AND h.request_json=$request AND h.actor_user_id=$subject AND size(enrollments)=1
       AND head(enrollments).student_id=h.enrolled_student_id AND head(enrollments).handover_id=h.id
       AND head(enrollments).status='active' AND head(enrollments).school_id=$school AND head(enrollments).tenant_id=$tenant
       AND head(enrollments).applicant_student_id=$student
       AND EXISTS {MATCH(s)-[:ENROLLED_AS]->(head_enrollment:EnrolledStudent)-[:ENROLLED_IN]->(sec) WHERE head_enrollment=head(enrollments)}
       AND NOT EXISTS {MATCH(head_enrollment:EnrolledStudent)-[:ENROLLED_IN]->(other:Section) WHERE head_enrollment=head(enrollments) AND other<>sec})
FOREACH(ignore IN CASE WHEN h.version=0 THEN [1] ELSE [] END |
  CREATE(e:EnrolledStudent {student_id:$permanent,student_number:$number,applicant_student_id:$student,
      school_id:$school,tenant_id:$tenant,year_group:$year,status:'active',enrolment_date:toString(date()),
      created_at:datetime(),updated_at:datetime(),handover_id:$handover,version:1})
  CREATE(s)-[:ENROLLED_AS]->(e)
  CREATE(e)-[:ENROLLED_IN {assigned_at:datetime()}]->(sec)
  SET s.applicantStatus='handed_to_sis',s.updatedAt=datetime(),
      h.id=$handover,h.version=1,h.enrolled_student_id=$permanent,h.student_number=$number,
      h.request_json=$request,h.actor_user_id=$subject,h.staff_member_id=$actor,
      h.school_id=$school,h.tenant_id=$tenant,h.section_id=$section,h.created_at=datetime()
  CREATE(:SISEnrollmentAudit {id:$audit,kind:'admission_handover',actor_user_id:$subject,staff_member_id:$actor,
      student_id:$student,enrolled_student_id:$permanent,school_id:$school,tenant_id:$tenant,section_id:$section,
      admission_id:$lead,application_id:$application,offer_id:$offer,offer_revision:$revision,payment_id:$payment,
      version:1,created_at:datetime()})
)
WITH h
MATCH(e:EnrolledStudent {student_id:h.enrolled_student_id,handover_id:h.id})
RETURN e.student_id AS permanent,e.student_number AS number,e.enrolment_date AS date
