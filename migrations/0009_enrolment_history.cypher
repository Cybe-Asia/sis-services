// One record per Student, school and academic year (Learning ADR 0017 slice 5, docs/LEARNING_STUDENT_HISTORY.md).
CREATE CONSTRAINT sis_enrolment_period_key IF NOT EXISTS FOR (p:SISEnrolmentPeriod) REQUIRE p.key IS UNIQUE;
CREATE INDEX sis_enrolment_period_student IF NOT EXISTS FOR (p:SISEnrolmentPeriod) ON (p.tenant_id, p.student_id);
CREATE INDEX sis_enrolment_period_backfill IF NOT EXISTS FOR (p:SISEnrolmentPeriod) ON (p.backfill_run)
