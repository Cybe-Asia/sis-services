# Student enrolment history for the Learning pathway

Learning ADR 0017 §7: the pathway shows the units of each grade a Student attended or attends, across IISS (Year 7–9)
and IIHS (Year 10–12). Which grades and schools those are comes from SIS identity, never from a shared Learning scope.

`GET /api/v1/sis-service/learning/owner/students/{studentId}/history?school_id=…&tenant_id=…` with the caller's
bearer, pinned to one school exactly as `/learning/owner/authorize`.

```json
{"data":{"contractVersion":1,"databaseId":"…","actor":{"id":"user-1","role":"student"},
  "schoolId":"SCH-IIHS","tenantId":"…","studentId":"STU-1",
  "enrolments":[
    {"schoolId":"SCH-IISS","schoolName":"IISS","status":"inactive","entryYear":2025,"entryYearGroup":"Year 7",
     "graduationYear":2028,"class":{"id":"SEC-8A","name":"8A","yearGroup":"Year 8","academicYear":"2026/2027"}},
    {"schoolId":"SCH-IIHS","schoolName":"IIHS","status":"active","entryYear":2027,"entryYearGroup":"Year 10",
     "graduationYear":2030,"class":{"id":"SEC-10A","name":"10A","yearGroup":"Year 10","academicYear":"2027/2028"}}]}}
```

- Every `EnrolledStudent` of the Student in the tenant, any status, ordered by entry year; other tenants are left out.
  At most 8 (more is 503).
- `schoolName`: the School's `school_name`, else its legacy `name` (until 2026-10-30 only `name` was read, which the
  current School nodes do not carry, so it was `null`).
- `entryYear`, `entryYearGroup`, `graduationYear`: what was stored at placement (`school_years.rs`); `null` when not
  recorded.
- `class`: the class the enrolment is in now, or was in last when the enrolment ended, whatever the class status;
  `null` without one. An enrolment in two classes is 503 (the roster keeps one).
- SIS keeps no record per school year: a roster move replaces the class. Learning derives the years in between
  (one grade per academic year) and marks them as derived.
- Who may read: the Student themselves (`student_id` must be theirs), a Parent of the child, a Teacher with the Student
  in a class they teach or are homeroom of, an Owner of the school, or a staff member whose `learning.mastery.read`
  roster (ADR 0018, `mastery-roster`) includes the Student. Anyone else is 403. The check runs before and after the
  read. `Cache-Control: no-store`.
