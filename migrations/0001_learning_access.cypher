CREATE CONSTRAINT learning_teaching_key IF NOT EXISTS FOR (g:LearningTeachingGrant) REQUIRE g.key IS UNIQUE;
CREATE CONSTRAINT learning_student_user IF NOT EXISTS FOR (g:LearningStudentBinding) REQUIRE g.user_id IS UNIQUE;
CREATE CONSTRAINT learning_student_identity IF NOT EXISTS FOR (g:LearningStudentBinding) REQUIRE g.student_id IS UNIQUE;
CREATE CONSTRAINT learning_access_audit IF NOT EXISTS FOR (a:LearningAccessAudit) REQUIRE a.id IS UNIQUE;
CREATE CONSTRAINT learning_binding_lock IF NOT EXISTS FOR (g:LearningBindingLock) REQUIRE g.key IS UNIQUE;
