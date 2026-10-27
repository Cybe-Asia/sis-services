# School classes for Learning class assignments

Learning ADR 0016 lets a module written for one class be assigned to the other classes of the same grade and academic
year. Learning reads the school's classes from SIS to offer and check those classes.

`GET /api/v1/sis-service/learning/owner/classes?school_id=…&tenant_id=…` with the staff member's bearer.

```json
{"data":{"contractVersion":1,"databaseId":"…","actor":{"id":"staff-1","role":"teacher"},
  "schoolId":"…","tenantId":"…",
  "classes":[{"id":"SEC-7A","name":"7A","yearGroup":"Year 7","academicYear":"2026/2027","taughtSubjects":["mathematics"]}]}}
```

- Active classes of the school only (`Section.status = 'active'`), at most 200, ordered by id. A class whose id is held
  by two `Section` nodes is left out, as in the Teacher list.
- `yearGroup` and `academicYear` are `null` when the class has none.
- `taughtSubjects`: the subject codes of the caller's active `TeachingAssignment`s in that class for the class's academic
  year ([TEACHING_ASSIGNMENTS.md](TEACHING_ASSIGNMENTS.md)). Learning ignores it for an Owner, who may assign to any
  class.
- Teacher (active, of this school and tenant) or Owner of the school; anything else is 403. No student data is returned.
