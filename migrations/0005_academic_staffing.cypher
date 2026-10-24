CREATE CONSTRAINT academic_subject_plan_key IF NOT EXISTS FOR (n:AcademicSubjectPlan) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT teaching_assignment_key IF NOT EXISTS FOR (n:TeachingAssignment) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT teaching_assignment_id IF NOT EXISTS FOR (n:TeachingAssignment) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT academic_staffing_audit_key IF NOT EXISTS FOR (n:AcademicStaffingAudit) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT academic_staffing_receipt_key IF NOT EXISTS FOR (n:AcademicStaffingReceipt) REQUIRE n.key IS UNIQUE;
CREATE INDEX academic_subject_plan_scope IF NOT EXISTS FOR (n:AcademicSubjectPlan) ON (n.school_id, n.tenant_id, n.academic_year);
CREATE INDEX teaching_assignment_scope IF NOT EXISTS FOR (n:TeachingAssignment) ON (n.school_id, n.tenant_id, n.academic_year);
CREATE INDEX teaching_assignment_holder IF NOT EXISTS FOR (n:TeachingAssignment) ON (n.staff_member_id, n.section_id)
