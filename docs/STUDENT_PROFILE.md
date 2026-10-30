# Student Profile contract

Added 2026-10-20 for the Student web Profile (Figma Student `yZgETbhThx5MuOqR8sWwYD`, node `868:13754`).

## `GET /api/v1/sis-service/learning/owner/student/profile?school_id&tenant_id`

Student bearer only (same actor resolution and `STUDENT` binding query as `student/school`). The caller names no student: `class_id`, `student_id` and `access` are refused with 400; any non-student role gets 403; an account bound to more than one student gets 403. 5 s deadline, 503 on any graph failure.

```json
{"data":{"contractVersion":1,"actor":{"id":"…","role":"student"},"schoolId":"…","tenantId":"…","studentId":"…",
 "profile":{"fullName":"…","email":null,"nisn":null,"birthPlace":null,"dateOfBirth":null,"gender":null,"emergencyContact":null},
 "school":{"id":"…","name":null},
 "classes":[{"id":"…","name":"…","grade":null,"academicYear":null,"gradeCode":null,"academicYearCode":null}],
 "guardians":[{"name":"…","relationship":null,"phones":[],"email":null}]}}
```

Sources and null rule (null means "not recorded", the client shows a dash):

| Field | Graph source | Accepted value |
|---|---|---|
| `email` | `User.email` | one `@`, no spaces, ≤254 |
| `nisn` | `Student.nisn` | exactly 10 digits |
| `birthPlace` | `Student.birthPlace` | ≤128 chars |
| `dateOfBirth` | `Student.dateOfBirth` (admissions) | real `YYYY-MM-DD` |
| `gender` | `Student.gender` | `male`/`female` (also `L`/`P`, `laki-laki`/`perempuan`) |
| `emergencyContact` | `Student.emergencyContact` | 6–15 digits, `+` only first |
| `school.name` | `School.name` | ≤256 |
| `classes[].grade` / `academicYear` | `Section.year_group` / `Section.academic_year` | ≤64 / ≤32 |
| `classes[].gradeCode` / `academicYearCode` | `Section.grade_code` / `Section.academic_year_code` (nomenclature slice 5; null until coded) | ≤8 / ≤16 |
| `guardians[]` | the admissions `Lead` (`parent_name`, `relationship`, `mobile`, `whatsapp`, `email`), then `(Student)-[:HAS_GUARDIAN]->(Guardian {full_name, relationship, phone_primary, phone_secondary, email})` | max 4; a row without a name is dropped; `relationship` ∈ `father`/`mother`/`guardian` (also `ayah`/`ibu`/`wali`); duplicate phone dropped |

`nisn`, `birthPlace`, `gender`, `emergencyContact`, `Lead.relationship` and `Guardian` have no writer yet; Admin entry is separate work.

## Student preferences: notification categories

`GET/POST /api/v1/sis-service/learning/owner/student/preferences` (unchanged route) gains an optional `notificationCategories` object with exactly six booleans: `assignments`, `grades`, `classSchedule`, `announcements`, `messages`, `learningReminders`, stored on the `User` as `student_notify_*`.

- A body without it keeps the old meaning: every category follows `notifications`.
- A body with it stores the six and sets `notifications` to their OR.
- Responses always include it; saved categories that disagree with the single switch (an older release switched everything off) report every category equal to the switch.

Release order: the Learning web gateway and the Student client must accept the optional field before this SIS image rolls out.
