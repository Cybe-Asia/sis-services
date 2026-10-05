CREATE CONSTRAINT learning_timetable_draft IF NOT EXISTS FOR (n:LearningTimetableDraft) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT learning_timetable_operation IF NOT EXISTS FOR (n:LearningTimetableOperation) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT learning_timetable_teacher_lock IF NOT EXISTS FOR (n:LearningTimetableTeacherLock) REQUIRE n.key IS UNIQUE;
