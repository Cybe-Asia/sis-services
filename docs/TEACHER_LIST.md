# Admin teacher list (teacher assignments, slice 1)

Added 2026-10-24. This is slice 1 of `digital-school/docs/admin/teacher-assignments-spec.md` §8. It is a
read-only list of who teaches what, built from data SIS already stores. There is no new label, property,
constraint or migration, and nothing is written. Responses are `Cache-Control: no-store`.

Slice 1 read "who teaches what" from **published lessons** (`LearningClassMeeting`) only, and the only
hours were **scheduled hours**. Since slice 2 (`docs/TEACHING_ASSIGNMENTS.md`) each teacher also carries
their teaching assignments and planned periods, and the remaining warnings of spec §5. The additions are
listed under "Teacher list additions" there. Everything below still holds.

## Access

The caller is a staff portal session (`Authorization: Bearer`, Auth introspection) of an active
`school_admin` of the school, or of an `owner` whose scope covers it. This is the same rule as the Admin
timetable routes. The rule is checked three times:

1. on the introspected session;
2. in the graph (`StaffMember` active, one node per id, exactly one `School` node);
3. on the session again after the read. A role revoked while the read ran gets 403, not the data.

Admins and Owners get the same projection. Teachers and other roles get 403.

## `GET /api/v1/sis-service/academic/teachers/context`

The schools the caller administers, with the academic years that have active classes, newest first. An
Owner without a school scope sees every school that has exactly one `School` node. A school
administrator sees only the schools in their own session lists.

```json
{"data":{"role":"owner","schools":[{"schoolId":"SCH-IISS","tenantId":"TENANT-001",
  "academicYears":[{"academicYear":"2026/2027","classes":2}]}]}}
```

This route needs no `schoolId`, so an Owner does not hit the 400 that `school_portal` `select` gives an
Owner who names no school.

## `GET /api/v1/sis-service/academic/teachers?schoolId&tenantId&academicYear[&week]`

- **`academicYear`:** compared exactly with `Section.academic_year`, for example `2026/2027`.
- **`week`:** optional `YYYY-MM-DD`. The response covers the Monday-to-Friday school week that contains
  that date. A Saturday or Sunday means the next week. Without `week`, the week is today's in WIB.
- **Errors:** unknown parameters, a malformed id, a blank or overlong year, or a bad date give 400
  `INVALID_INPUT`. A school outside the caller's scope gives 403. A graph error, a timeout (8 s) or a
  bound exceeded gives 503 `DEPENDENCY_UNAVAILABLE`.

```json
{"data":{
  "schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027",
  "week":{"from":"2026-10-26","to":"2026-10-30","lessons":42},
  "limits":{"lowerPeriods":18.0,"upperPeriods":24.0,"juniorPeriodMinutes":40,"seniorPeriodMinutes":45},
  "teachers":[{
    "staffMemberId":"…","name":"Siti Rahma","active":true,
    "subjects":["mathematics"],
    "classes":[{"sectionId":"SEC-7A","name":"7A","yearGroup":"Year 7","subjects":["mathematics"],
                "lessons":5,"minutes":200,"periods":5.0}],
    "homeroomOf":[{"sectionId":"SEC-7A","name":"7A","yearGroup":"Year 7"}],
    "coachOf":[{"activityId":"…","title":{"en":"Football","id":"Sepak bola"}}],
    "learningAccess":[{"sectionId":"SEC-7A","name":"7A","yearGroup":"Year 7"}],
    "lessons":5,"scheduledMinutes":200,"scheduledPeriods":5.0,
    "warnings":[{"code":"UNDERLOAD"},{"code":"TEACHER_CLASH","count":1}]
  }],
  "classWarnings":[{"code":"NO_HOMEROOM","sectionId":"SEC-8A","name":"8A","yearGroup":"Year 8"}],
  "summary":{"teachers":12,"withWarnings":4,"noHomeroom":1,"clashes":1,"overloaded":0,"underloaded":3,"inactive":0}
}}
```

**Who is listed:**
- every active teacher of the school (`'teacher'` in `roles`, school and tenant in the member's lists,
  `membershipStatus` `ACTIVE`, one node per id);
- anyone else, active or not, who holds something in this school year: a lesson in the week, a homeroom
  of an active class, an active `LearningTeachingGrant` on an active class, or a non-archived
  extracurricular activity as `coach_id`.

The list is ordered by name and holds at most 300 people; more gives 503.

**Field sources:**

| Field | Source |
|---|---|
| `subjects`, `classes`, `lessons`, `scheduledMinutes` | Published `LearningClassMeeting`s of the school's active classes in this academic year that overlap the week. `subject` and `title` come from the meeting payload. |
| `homeroomOf` | `Section.homeroom_staff_member_id` |
| `learningAccess` | Active `LearningTeachingGrant`s |
| `coachOf` | `ExtraCurricularActivity.coach_id` of the year, archived excluded |

**Periods:** one lesson period is 40 minutes in a class whose `year_group` number is below 10 (Years
7–9, SMP), and 45 minutes from Year 10 (SMA). A class without a number counts as 40 minutes. Each lesson
is divided by its own class's period length. Values are rounded to one decimal.

## Warnings

| Code | Where | When |
|---|---|---|
| `NO_HOMEROOM` | `classWarnings` | An active class has no `homeroom_staff_member_id`, or it names an id with no `StaffMember`. |
| `TEACHER_CLASH` (`count` = pairs) | teacher | Two lessons of the teacher in different classes overlap. Lessons that only touch end to start do not clash. The schedule marks each lesson in a clash with `clash: true`. |
| `OVERLOAD` / `UNDERLOAD` | teacher | An active teacher's `scheduledPeriods` is above 24 or below 18. Exactly 18 or 24 is within the limits. Not reported in a week without any lesson in the school, for example a holiday. |
| `INACTIVE_TEACHER` | teacher | The person holds something listed above but is not an active teacher of the school: account not `ACTIVE`, teacher role or school removed, no `StaffMember`, or a duplicated id. |

The limits are the defaults proposed in spec §7 #1–#3. Homeroom and coaching do not count as hours.
Warnings never block anything. Publishing still refuses teacher clashes as before, so a clash here comes
from an ad-hoc class started now or from older data.

## `GET /api/v1/sis-service/academic/teachers/:staffMemberId?schoolId&tenantId&academicYear[&week]`

Same parameters and checks as the list. It returns the same envelope with `teacher` (one list row) and
`schedule`, the teacher's lessons of the week ordered by day and time:

```json
"schedule":[{"id":"wk-mon-0730-math-20261026","sectionId":"SEC-7A","className":"7A","subject":"mathematics",
  "title":"Mathematics","weekday":1,"start":"07:30","end":"08:50","roomId":"r-101","clash":false}]
```

`weekday` is ISO (Monday = 1). Times are school time (WIB). If the id is not someone the list would show,
the response is 404 `NOT_FOUND`, the same as for an unknown id.

## Differences from the spec

- **No weekly slot template.** The spec (§2, §8) speaks of stored weekly slots. SIS stores none: the
  weekly timetable is published as dated `LearningClassMeeting`s over at most 31 days per call, and the
  slot set only survives inside the operation receipt. "Scheduled hours" is therefore the published
  lessons of one school week, and `week` lets Admin look at another week, for example after a holiday.
- **Ad-hoc lessons are counted.** A meeting does not record whether it was started now, so such a
  lesson counts as a scheduled lesson in its week and may show as a clash.
- **Path and parameters.** The spec's `…/academic/teachers?school&year` became
  `/api/v1/sis-service/academic/teachers?schoolId&tenantId&academicYear`, plus a `context` route and a
  per-teacher route, following the other portal-session SIS routes.
- **Teaching in two schools.** Load and clashes are counted per school. A teacher of both IISS and IIHS
  is listed in each school with that school's lessons only.
- **No photograph.** ADR 0007 is still proposed, so SIS has no teacher photograph to return.

## Tests

- `cargo test --bin sis-service academic_teachers`: week window, period length, load limits, clashes,
  inactive holders, `NO_HOMEROOM`, bounds and parameter parsing.
- The graph test is ignored by default. Run it against a disposable Neo4j only:
  `SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223 cargo test --bin sis-service academic_teachers -- --ignored`
  (for example `docker run --rm -p 127.0.0.1:3223:7687 -e NEO4J_AUTH=neo4j/parent-otp-fixture-password neo4j:5.26-community`).
  It covers the access rule for admin, Owner, another school and a suspended admin, the week window,
  archived classes and meetings, other years, revoked grants, archived activities, a duplicated
  `StaffMember`, and the context for each role.
