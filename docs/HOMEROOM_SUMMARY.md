# Homeroom summary and the `homeroom` access kind (ADR 0015 layer B)

Added 2026-10-22 for learning monitoring. A homeroom teacher (or an Owner) sees each Student of
their class across subjects. SIS provides two pieces: the attendance and official grades of the
class, and the answer to whether a caller may read Learning's cross-subject view of that class.
Both are reads: nothing is written and no audit row is added. Responses are `Cache-Control: no-store`.

## `GET /api/leads/v1/teacher/school/classes/:section/homeroom-summary?from&to[&term][&schoolId&tenantId]`

- **Access:** the same staff session and live checks as the other teacher-school routes
  (`teacher_query` with `homeroom_only`). The section's `homeroom_staff_member_id` must be the
  caller, or the caller must be an Owner whose scope covers the school. A teacher who only has a
  `LearningTeachingGrant` for the class gets 403, and so does a foreign or inactive class.
- **Input:** `from` and `to` are exact `YYYY-MM-DD` dates with `from <= to`, covering fewer than
  200 days. `term` is optional: trimmed text of at most 64 characters, matched exactly against
  `GradeEntry.term`. Without `term`, every Student's `grades` is empty. Anything else is 400
  (`INVALID_INPUT`).
- **Output:** the actively enrolled Students of the class (at most 200), ordered by name:

```json
{"responseCode":200,"data":{"sectionId":"SEC-7A","from":"2026-07-14","to":"2026-10-22","term":"Semester 1",
 "students":[{"studentId":"…","name":"…",
   "attendance":{"present":60,"late":2,"absent":0,"excused":2,"recordedDays":64},
   "grades":[{"subject":"Math","score":78.0,"maxScore":100.0}]}]}}
```

`attendance` counts the `AttendanceRecord`s of the class in the range, one per day and Student.
`recordedDays` includes a stored status that is none of the four named ones. Sick leave arrives as
`excused`. `grades` are the term's `GradeEntry` rows as stored; their subjects are free text, and
Learning does not join them to its own subjects.

## `homeroom` in `GET /api/v1/sis-service/learning/owner/authorize`

`access=homeroom&class_id=…` (no `student_id`) is allowed for:

- a `teacher` whose class appears in the Learning projection **and** who is that Section's
  `homeroom_staff_member_id`, matched on exactly one active Section of the school and tenant;
- an `owner` by the same live scope as `teach`.

`teach` is unchanged: a homeroom teacher or a teaching grant still qualifies. Learning uses
`homeroom` for its read-only cross-subject view, so a subject teacher sees only their own courses.

## Tests

- `cargo test --bin sis-service homeroom`: range, term and attendance tally.
- The graph test is ignored by default. Run it against a disposable Neo4j only:
  `SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223 cargo test --bin sis-service learning_access::owner -- --ignored`.
  It covers homeroom, grant-only teacher, Owner, foreign class, range and term.
