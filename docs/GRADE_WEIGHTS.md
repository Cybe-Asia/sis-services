# Grade weights (gradebook, slice 2)

Added 2026-10-26 for slice 2 of `digital-school/docs/learning/gradebook-spec.md`, on the defaults the user
approved for §7 (#1 weights 20/20/20/15/25, #4 KKM 75 per subject, #6 only Admin changes weights).

SIS owns the weights. Learning reads them to compute the final grade in the Teacher gradebook; it stores no
copy. Code: `src/academic_staffing/weights.rs` (pure rules) and `src/academic_staffing/repository/weights.rs`
(graph). Schema: `migrations/0006_grade_weights.cypher`, applied at start-up by `academic_staffing::init`.

## Model

`AcademicGradeWeights {key}`, one record per:

- **school default** of an academic year: key `tenant|school|year|school`;
- **override** for one grade, subject and term: key `tenant|school|year|subject|grade|subject|term`.

A record holds `weights` (whole percents of `chapterQuiz` (Kuis bab), `quiz` (Kuis, pop quizzes included),
`assignment` (Tugas), `pts` and `pas`, adding up to 100), `passMark` (KKM, 0–100), a `version` and a `status`
(`active` or `cleared`). A cleared override keeps its version and no weights; saving it again names that version.

Terms are `semester_1` and `semester_2`. An override names a subject that is an active row of the grade's subject
plan for that year (`docs/TEACHING_ASSIGNMENTS.md`).

**Resolution** for a class, subject and term: the active override of the class's grade and year, else the active
school default of that year, else the built-in default (20/20/20/15/25, KKM 75, `source: "default"`, version 0).

Every save and clear writes an `AcademicStaffingAudit` (`kind: grade_weights`, `before`, `after`, actor, role,
command key) in the same transaction, behind the same locks, authority check, Auth re-check and
`Idempotency-Key` receipt as the subject plans.

## Admin: `GET /api/v1/sis-service/academic/grade-weights?schoolId&tenantId&academicYear`

Same access as the subject plans: an active `school_admin` of the school, or an Owner whose scope covers it.
Teachers get 403.

```json
{"data":{"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027",
  "builtin":{"weights":{"chapterQuiz":20,"quiz":20,"assignment":20,"pts":15,"pas":25},"passMark":75},
  "school":{"version":1,"status":"active","weights":{…},"passMark":75,"updatedAt":"…"},
  "subjects":[{"grade":"Year 7","subject":"mathematics","term":"semester_1","version":2,"status":"active",
               "weights":{…},"passMark":80,"updatedAt":"…"}]}}
```

`school` is `null` until a default is saved.

## Admin: `PUT /api/v1/sis-service/academic/grade-weights`

Needs `Idempotency-Key`. Leave out `grade`, `subject` and `term` for the school default; give all three for an
override.

```json
{"tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027",
 "grade":"Year 7","subject":"mathematics","term":"semester_1","expectedVersion":0,
 "set":{"weights":{"chapterQuiz":20,"quiz":20,"assignment":20,"pts":15,"pas":25},"passMark":80}}
```

`{"expectedVersion":2,"clear":true}` (with the same target) removes an override or the school default. The answer
is the saved record as in the list, plus `grade`, `subject` and `term`.

| Status | Code | When |
|---|---|---|
| 400 | `INVALID_INPUT` | Unknown fields, a partial target, weights that do not add up to 100, a KKM above 100, `clear` with a set or at version 0 |
| 403 | `FORBIDDEN` | Not an admin or Owner of the school |
| 409 | `REVISION_CONFLICT` | Stale `expectedVersion`, or clearing a record that is not active |
| 409 | `IDEMPOTENCY_KEY_REUSED` | Same key, another body |
| 422 | `SUBJECT_NOT_IN_PLAN` | The subject is not an active row of the grade's plan that year |
| 503 | `DEPENDENCY_UNAVAILABLE` | Graph or Auth error, a timeout |

## Learning: `GET /api/v1/sis-service/learning/grade-weights?schoolId&tenantId&sectionId&subject&term`

The weights that apply to one class (section), subject code and term. Learning calls it with the signed-in staff
member's bearer, the same way it reads the prework policy. Active teachers and administrators of the school and
Owners whose scope covers it may read; nobody writes here.

```json
{"data":{"schoolId":"SCH-IISS","tenantId":"TENANT-001","sectionId":"SEC-7A","academicYear":"2026/2027",
  "grade":"Year 7","subject":"mathematics","term":"semester_1","source":"subject","version":2,
  "weights":{"chapterQuiz":20,"quiz":20,"assignment":20,"pts":15,"pas":25},"passMark":80}}
```

404 when the section is not one record of the school with an academic year. A class without a `year_group` gets
the school default.

## Rollback

Additive: an older SIS ignores the records, and Learning then shows the gradebook without a final grade.
