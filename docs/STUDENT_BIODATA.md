# Student biodata (Admin writer)

Added 2026-10-20. Admin records the Student biodata and guardians that the Student Profile reads (`docs/STUDENT_PROFILE.md`, `GET …/learning/owner/student/profile`). Admin UI: the Enrolment tab of the Admin student detail page.

Both routes need a staff portal session (`Authorization: Bearer`, introspected like the other Admin routes) of an active `school_admin` of the Student's school, or an owner whose scope covers it (`education_calendar::repository::ADMIN`). The Student must be actively enrolled there (`(Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active'})`). Responses are `Cache-Control: no-store`; 5 s deadline; nothing is logged.

## `GET /api/v1/sis-service/admin/student-biodata?studentId`

`studentId` is the admissions `Student.studentId` (the applicant id). SIS finds the one school and tenant of the active enrolment, then checks the staff scope. An unknown Student, a Student without exactly one active enrolment, and staff of another school all get the same 404 (`NOT_FOUND`), so other schools cannot probe who is enrolled.

```json
{"data":{"contractVersion":1,"tenantId":"…","schoolId":"…","studentId":"…","fullName":"…","dateOfBirth":"2013-04-02","version":3,
 "biodata":{"nisn":"1328909120","birthPlace":"Jakarta","gender":"female","emergencyContact":"+62 812 1111 2222"},
 "admissionsGuardian":{"name":"…","relationship":"mother","phones":["0812 1111 2222"],"email":"…"},
 "guardians":[{"id":"<uuid>","fullName":"…","relationship":"father","phonePrimary":"…","phoneSecondary":null,"email":null}],
 "limits":{"guardians":3}}}
```

Stored values are returned as they are (trimmed, at most 256 characters) so Admin can correct older entries. `gender` and `relationship` are mapped from older words (`L`/`P`, `laki-laki`/`perempuan`, `ayah`/`ibu`/`wali`) and are `null` when unmapped. `admissionsGuardian` is `null` unless exactly one admissions `Lead` has the Student.

## `POST /api/v1/sis-service/admin/student-biodata`

Replaces the whole record. Every field must be present (`null` or blank clears it); unknown fields are refused.

```json
{"tenantId":"…","schoolId":"…","studentId":"…","expectedVersion":3,
 "biodata":{"nisn":"1328909120","birthPlace":"Jakarta","gender":"female","emergencyContact":"+62 812 1111 2222"},
 "admissionsGuardianRelationship":"mother",
 "guardians":[{"id":null,"fullName":"…","relationship":"father","phonePrimary":"…","phoneSecondary":null,"email":null}]}
```

| Field | Stored on | Rule |
|---|---|---|
| `nisn` | `Student.nisn` | exactly 10 digits; unique across Students (409 `NISN_IN_USE`) |
| `birthPlace` | `Student.birthPlace` | ≤128 characters, no control characters |
| `gender` | `Student.gender` | `male` / `female` |
| `emergencyContact` | `Student.emergencyContact` | 6–15 digits; spaces, `-`, `(`, `)`; `+` only first; ≤32 characters |
| `admissionsGuardianRelationship` | `Lead.relationship` | `father` / `mother` / `guardian`; must be `null` unless exactly one `Lead` |
| `guardians[]` | `(Student)-[:HAS_GUARDIAN]->(Guardian {guardian_id, full_name, relationship, phone_primary, phone_secondary, email, position, school_id, tenant_id})` | at most 3, in display order; `fullName` 1–256 characters; `relationship` as above; `phonePrimary` required, `phoneSecondary` optional and different; `email` optional (one `@`, dotted domain, ≤254) |

Guardians: `id: null` creates one (SIS assigns a UUID); an existing `id` updates it; a saved guardian left out is deleted. An `id` the record no longer has is a stale form (409 `REVISION_CONFLICT`).

Concurrency: CAS on `Student.biodata_version` (0 before the first save) under a write lock on the Student. A wrong `expectedVersion` is 409 `REVISION_CONFLICT`. A save that changes nothing writes nothing and returns the current version; otherwise the version moves by exactly one.

Audit: each saving write creates `StudentBiodataAudit {key, actor_id, actor_role, student_id, school_id, tenant_id, version, changed_fields, guardians_added, guardians_updated, guardians_removed, guardians_reordered, created_at}` in the same transaction. It names the changed fields, never their values.

Errors: 400 `INVALID_INPUT`, 401 `UNAUTHORIZED`, 403 `FORBIDDEN` (POST to a school outside the staff scope), 404 `NOT_FOUND`, 409 `REVISION_CONFLICT` / `NISN_IN_USE`, 503 `DEPENDENCY_UNAVAILABLE`.

Schema (created at start-up): unique `Guardian.guardian_id` (`guardian_id`), unique `StudentBiodataAudit.key` (`student_biodata_audit_key`).

`Lead` belongs to admission-services. SIS writes only `Lead.relationship` (and `relationship_updated_at`), and only through this route.

Tests: `cargo test student_biodata`. The graph test is ignored by default; run it against a disposable Neo4j only, with `STUDENT_BIODATA_TEST_NEO4J_URI`, `_USER` and `_PASSWORD` set and `-- --include-ignored`.
