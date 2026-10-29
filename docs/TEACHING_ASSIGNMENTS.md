# Subject plans and teaching assignments (teacher assignments, slice 2)

Added 2026-10-24. This is slice 2 of `digital-school/docs/admin/teacher-assignments-spec.md` §8, built on the
defaults the user approved for §7 #4–#6:

- team teaching is allowed, with the periods split between the teachers;
- each school's academic admin fills its own subject plans, and a plan is copied into the next year;
- a subject is the LMS subject code plus a framework label (`merdeka`, `cambridge`, `iiec`).

Nothing official is seeded. The Cambridge and IIEC subject names and codes still come from the school's
academic team, and they are data in these plans, not code.

The read model of slice 1 (`docs/TEACHER_LIST.md`) now adds planned periods and the remaining warnings of §5.

## Access

The caller is a staff portal session of an active `school_admin` of the school, or of an `owner` whose scope
covers it (spec §7 #7). Teachers and other roles get 403. Since 2026-10-29 (ADR 0018 slice 6) the graph check is a
capability decision, not a role fragment: reads need `academic.staffing.read`, plan and weight writes
`academic.subject_plan.manage`, assignment commands `academic.staffing.manage`, all at the school. Owners and
school administrators hold these on day one; see `docs/AUTHORIZATION.md`, "Services that check capabilities".

- **Reads:** session, the capability decision from the graph, the read, then the session again.
- **Writes:** in one transaction, in this order:
  1. a lock on the caller's `StaffMember`;
  2. the school lock (`EducationCalendarLock`, shared with rooms and the calendar);
  3. the capability decision from the graph, under the locks;
  4. Auth introspection again;
  5. the idempotency receipt;
  6. the change, its audit, grant sync and the receipt.

  A role revoked while the request waited gets 403 and nothing is written. Writes run detached from the HTTP
  request, so a client that disconnects cannot cancel a commit it then never hears about.

Every response is `Cache-Control: no-store`.

## Writes: revisions and `Idempotency-Key`

Every write needs an `Idempotency-Key` header (8–128 characters of `A-Z a-z 0-9 _ . : -`). The receipt is
per caller and school.

- **Same key and same body:** the stored answer is returned and nothing is written again.
- **Same key and another body:** 409 `IDEMPOTENCY_KEY_REUSED`.
- **Revisions:** every record carries a `version`. A write names the version it read (`expectedVersion`, 0
  creates). Any other version gives 409 `REVISION_CONFLICT`.

## Errors

The body is always `{"error":{"code":…}}`. Command errors also carry `index`, the failing command.

| Status | Code | When |
|---|---|---|
| 400 | `INVALID_INPUT` | Unknown fields, bad ids, a missing or short `Idempotency-Key`, rows that break the rules below |
| 401 / 403 | `UNAUTHORIZED` / `FORBIDDEN` | No session, or not an admin or Owner of the school |
| 404 | `NOT_FOUND` | Copy from a year whose plan for the grade has no active row |
| 409 | `REVISION_CONFLICT` | Stale `expectedVersion`, or the assignment is not in a state the command applies to |
| 409 | `SUBJECT_IN_USE` (`subjects`) | A plan save removes a subject that still has an active assignment in that grade |
| 409 | `IDEMPOTENCY_KEY_REUSED` | See above |
| 409 | `GRANT_CONFLICT` (`sectionId`) | The grant record of the teacher and class belongs to another school (corrupt data) |
| 422 | `CLASS_NOT_ACTIVE` | `assign` to a class that is not an active class of the school and year, or whose id is not unique |
| 422 | `SUBJECT_NOT_IN_PLAN` | `assign` a subject that is not an active row of the class's grade plan |
| 422 | `TEACHER_NOT_ELIGIBLE` | `assign` someone who is not an active teacher of the school, or whose id is duplicated |
| 503 | `DEPENDENCY_UNAVAILABLE` | Graph or Auth error, a timeout, or a bound exceeded |

## Subject plans

One plan per school, academic year and grade (`Section.year_group`, for example `Year 7`). Stored as
`AcademicSubjectPlan {key: tenant|school|year|grade}`.

A plan row:

```json
{"subject":"mathematics","name":"Matematika","framework":"merdeka","periodsPerWeek":5,"required":true,"status":"active"}
```

- **`subject`:** the LMS subject code: 1–64 characters, lower case letters, digits, `_` and `-`, starting with
  a letter or a digit. A code appears once per grade, so a code has one framework within a grade. Lessons and
  assignments record only the code.
- **`name`:** the display name, 1–120 characters, trimmed.
- **`framework`:** `merdeka`, `cambridge` or `iiec`.
- **`periodsPerWeek`:** 1–40 lesson periods.
- **`required`:** whether a missing teacher is a `SUBJECT_UNASSIGNED` warning.
- **`status`:** `active` or `archived`. An archived row is kept so that its old assignments still read, but it
  takes no new assignment and raises no warning.

A plan holds at most 40 rows. A row cannot be removed while the subject has an active assignment in a class of
that grade. Archive it instead, or end the assignments first.

### `GET /api/v1/sis-service/academic/subject-plans?schoolId&tenantId&academicYear`

Grades with active classes this year, and any grade that already has a plan. They are in natural order
(`Year 7` before `Year 10`). `sources` lists the other years with active rows to copy from, newest first.

```json
{"data":{"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027",
  "grades":[{"grade":"Year 7","classes":2,"plan":{"version":2,"updatedAt":"2026-10-24T09:12:03Z","rows":[…]}},
            {"grade":"Year 8","classes":1,"plan":null}],
  "sources":[{"academicYear":"2025/2026","grades":["Year 7","Year 8"]}]}}
```

A class without a `year_group` has no grade and cannot get a plan.

### `PUT /api/v1/sis-service/academic/subject-plans`

```json
{"tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027","grade":"Year 7",
 "expectedVersion":1,"rows":[…]}
```

The rows replace the plan. Response: `{"data":{"grade","version","rows","updatedAt"}}`.

### `POST /api/v1/sis-service/academic/subject-plans/copy`

```json
{"tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027","grade":"Year 7",
 "fromYear":"2025/2026","expectedVersion":0}
```

Copies the **active** rows of the same grade from `fromYear` and stores them like a `PUT`. The response is the
same as for `PUT`.

## Teaching assignments

One record per school year, class, subject and teacher. It is stored as
`TeachingAssignment {key: tenant|school|year|section|subject|staff}` with `periods`, a `status` (`active` or
`ended`) and a `version`. Team teaching is two or more records for the same class and subject; their periods
are meant to add up to the plan's `periodsPerWeek` (`PLAN_HOURS_MISMATCH` otherwise). An ended record is kept,
and assigning the same teacher again reopens it at its version.

### `GET /api/v1/sis-service/academic/teaching-assignments?schoolId&tenantId&academicYear[&sectionId|&staffMemberId]`

Everything the assignment matrix needs:

- the year's active classes with their homeroom;
- the year's plans;
- the teachers who can be assigned (active teachers of the school), plus anyone who holds an assignment, with
  `active`;
- every assignment of the year, active and ended, with its version.

`sectionId` or `staffMemberId` narrows the assignments only.

```json
{"data":{"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027",
  "sections":[{"sectionId":"SEC-7A","name":"7A","yearGroup":"Year 7",
               "homeroom":{"staffMemberId":"staff-1","name":"Siti Rahmawati","active":true}}],
  "plans":[{"grade":"Year 7","version":2,"rows":[…]}],
  "teachers":[{"staffMemberId":"staff-1","name":"Siti Rahmawati","active":true}],
  "assignments":[{"sectionId":"SEC-7A","subject":"mathematics","staffMemberId":"staff-1","periods":4,
                  "status":"active","version":1,"updatedAt":"2026-10-24T09:20:11Z"}]}}
```

Bounds: 200 classes, 1000 people, 4000 assignments. Past those the answer is 503.

### `POST /api/v1/sis-service/academic/teaching-assignments/commands`

```json
{"tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027","commands":[
  {"action":"end","sectionId":"SEC-7A","subject":"science","staffMemberId":"staff-1","expectedVersion":2},
  {"action":"assign","sectionId":"SEC-7A","subject":"science","staffMemberId":"staff-2","periods":3,"expectedVersion":0},
  {"action":"assign","sectionId":"SEC-7A","subject":"science","staffMemberId":"staff-3","periods":1,"expectedVersion":0}]}
```

The request carries 1–20 commands, at most one per class, subject and teacher. They apply together or not at
all.

| Action | `periods` | Applies when | Result |
|---|---|---|---|
| `assign` | 1–40 | no record and `expectedVersion` 0, or an `ended` record at `expectedVersion` | `active` |
| `change_hours` | 1–40 | an `active` record at `expectedVersion` | `active`, new periods |
| `end` | absent | an `active` record at `expectedVersion` | `ended`, periods kept |

`assign` also checks the class, the plan and the teacher (the 422 codes above). Answer:

```json
{"data":{"assignments":[{"sectionId":"SEC-7A","subject":"science","staffMemberId":"staff-2","periods":3,
                         "status":"active","version":1,"updatedAt":"…"}],
         "grants":[{"staffMemberId":"staff-2","sectionId":"SEC-7A","status":"ACTIVE"},
                   {"staffMemberId":"staff-1","sectionId":"SEC-7A","status":"REVOKED"}]}}
```

### Grant sync

After the commands, every teacher and class they touched is brought in line, in the same transaction:

- **While the teacher has any active assignment in the class:** their `LearningTeachingGrant`
  (`key: staff|section`) is `ACTIVE`. It is created or reactivated if needed, with `version + 1`.
- **When this request ended the teacher's last assignment in the class:** the grant is `REVOKED`.

Each change writes a `LearningAccessAudit` with `source: 'teaching_assignment'` and takes the section lock
that the Learning access grant command takes. `grants` in the answer lists only grants that changed.

The grant stays per class, as before. The subject a teacher may mark is the assignment itself, which is what
the gradebook will read.

Grants made before this feature are left alone: they keep working until assignments are filled in. A grant
without an assignment is reported as `GRANT_WITHOUT_ASSIGNMENT` once the class's grade has a plan. Ending
an assignment revokes the grant even if it was first made by hand, as spec §3.2 says. The manual Learning access
grant command (`POST /api/v1/sis-service/learning/teaching-grants`) still exists. Retiring it is a later step,
after schools have filled their assignments.

### Audit

Every plan save or copy and every command writes one `AcademicStaffingAudit`:

- `kind`: `subject_plan` or `teaching_assignment`;
- `action`: `save`, `copy`, `assign`, `change_hours` or `end`;
- `record_key`, `actor_id`, `actor_role`, school, tenant and year;
- `before` and `after` as JSON;
- `command_key`, the `Idempotency-Key`;
- `created_at`.

## Warnings and planned periods

### `GET /api/v1/sis-service/academic/staffing-warnings?schoolId&tenantId&academicYear[&week]`

Takes the same parameters, checks and week rule as the teacher list. It returns every warning of §5 with what
it is about, and counts by level and by code:

```json
{"data":{"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027",
  "week":{"from":"2026-10-26","to":"2026-10-30","lessons":42},"limits":{…},
  "warnings":[
    {"code":"NO_HOMEROOM","level":"critical","sectionId":"SEC-8A","name":"8A","yearGroup":"Year 8"},
    {"code":"SUBJECT_UNASSIGNED","level":"critical","sectionId":"SEC-7B","name":"7B","yearGroup":"Year 7",
     "subject":"mathematics","plannedPeriods":4,"assignedPeriods":0},
    {"code":"HOURS_MISMATCH","level":"attention","staffMemberId":"staff-2","teacherName":"Budi Santoso",
     "sectionId":"SEC-7A","name":"7A","yearGroup":"Year 7","subject":"science","plannedPeriods":3,"scheduledPeriods":1.0},
    {"code":"UNDERLOAD","level":"attention","staffMemberId":"staff-2","teacherName":"Budi Santoso","scheduledPeriods":6.0}],
  "summary":{"levels":{"critical":2,"attention":2,"info":0},"codes":{"HOURS_MISMATCH":1,…}}}}
```

| Code | Level | Subject | When |
|---|---|---|---|
| `NO_HOMEROOM` | critical | class | As in slice 1 |
| `SUBJECT_UNASSIGNED` | critical | class + subject | A **required** active row of the grade's plan has no active assignment in the class |
| `PLAN_HOURS_MISMATCH` | attention | class + subject | The class has assignments for an active plan row, but their periods do not add up to `periodsPerWeek` (spec §3.1 "jam kurang") |
| `TEACHER_CLASH` | critical | teacher (`count`) | As in slice 1 |
| `HOURS_MISMATCH` | attention | teacher + class + subject | The week's scheduled periods of the teacher in that class and subject differ from the assigned periods by more than 0.25 |
| `OVERLOAD` / `UNDERLOAD` | attention | teacher | As in slice 1 (scheduled periods against 18–24) |
| `SCHEDULE_WITHOUT_ASSIGNMENT` | attention | teacher + class + subject | The teacher has lessons in the class and subject this week but no active assignment for them |
| `GRANT_WITHOUT_ASSIGNMENT` | info | teacher + class | An active grant to the class with no active assignment of that teacher in it |
| `INACTIVE_TEACHER` | critical | teacher | As in slice 1; holding an active assignment now also counts |

- **Week rule:** in a week without any lesson in the school, the timetable is not compared with the plan, as
  for load, so `HOURS_MISMATCH` is quiet.
- **Grade rule:** `SCHEDULE_WITHOUT_ASSIGNMENT` and `GRANT_WITHOUT_ASSIGNMENT` appear only in classes whose
  grade has a plan this year. A school that has not started on assignments sees no flood of warnings, and a
  grade starts being checked once its plan is saved.
- **No blocking:** warnings never block a write. Publishing a timetable still refuses clashes as before.

### Teacher list additions (`GET …/academic/teachers` and `…/teachers/:id`)

Added to each teacher:

- `assignments`: `[{sectionId,name,yearGroup,subject,periods,scheduledPeriods}]`, active only;
- `plannedPeriods`: the sum of their periods;
- `subjects`: now also includes assigned subjects;
- `warnings`: may now carry `HOURS_MISMATCH`, `SCHEDULE_WITHOUT_ASSIGNMENT` and `GRANT_WITHOUT_ASSIGNMENT`, each
  with `count`.

The list adds `subjectWarnings` (`SUBJECT_UNASSIGNED` and `PLAN_HOURS_MISMATCH`, with `subject`,
`plannedPeriods` and `assignedPeriods`). `summary` adds `subjectUnassigned`, `planHoursMismatch`,
`hoursMismatch`, `scheduleWithoutAssignment` and `grantWithoutAssignment`.

`classWarnings` still holds only `NO_HOMEROOM`, so the slice 1 Admin, which drops unknown fields and codes and
caps that list at 200, keeps working against this SIS. That is why SIS can be released before Admin.

## Schema

Applied at start-up from `migrations/0005_academic_staffing.cypher` (`academic_staffing::init`). The statements
are idempotent.

- **Uniqueness constraints:** `AcademicSubjectPlan.key`, `TeachingAssignment.key`, `TeachingAssignment.id`,
  `AcademicStaffingAudit.key` and `AcademicStaffingReceipt.key`.
- **Indexes:** plans and assignments by school, tenant and year, and assignments by teacher and class.

There is no backfill. Existing grants stay as they are.

## Tests

- `cargo test --bin sis-service academic_`: row and command validation, revision rules, batch bounds, grade
  order, planned periods and every new warning, levels and the wire shapes.
- **Graph test** (ignored by default; disposable Neo4j only):
  `SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223 cargo test --bin sis-service academic_ -- --ignored --test-threads=1`.
  Both graph tests wipe the database, so they run one at a time.

  It covers:
  - plan create, replay, key reuse, stale revision, foreign admin and teacher refused;
  - copy, both from a year with no plan and with archived rows left out;
  - team teaching and the grants it creates;
  - each 422 with its index, and that nothing of a refused batch is written;
  - changing hours;
  - ending one of two subjects, which keeps the grant, and ending the last, which revokes it;
  - reopening an assignment;
  - `SUBJECT_IN_USE` against archiving;
  - audit counts with before and after;
  - the reads, and the teacher list snapshot.

  Auth introspection is not part of the graph test. It is covered by the `learning_access` introspection tests,
  and the write path checks it again with the locks held.
