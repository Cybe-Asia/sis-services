CREATE CONSTRAINT academic_grade_weights_key IF NOT EXISTS FOR (n:AcademicGradeWeights) REQUIRE n.key IS UNIQUE;
CREATE INDEX academic_grade_weights_scope IF NOT EXISTS FOR (n:AcademicGradeWeights) ON (n.school_id, n.tenant_id, n.academic_year)
