# School broadcasts

A school administrator sends one announcement to an audience in **their own school**. Parents get it
as a notice on the school board (`/parent`, "Papan sekolah", and the app's board) and as a push to
their app devices and subscribed browsers. Staff get the push only.

Code: `src/school_broadcasts.rs`, `src/school_broadcasts/repository.rs`.

## Who may send

Staff session (same as school articles): `school_staff_actor` for the school, then the
`school.content.manage` capability (school administrators and Owners on day one). The graph
statement that claims the broadcast checks the caller's StaffMember node again. Teachers, other
schools' administrators and parents get 403. The endpoints are reached through the admin portal BFF
(`/api/admin/sis/school-broadcasts`) only; they are not on the public ingress.

## `POST /api/v1/sis-service/school-broadcasts`

```json
{
  "tenantId": "…", "schoolId": "…",
  "id": "4f0c…",                        // client-made, once per announcement; a retry reuses it
  "audience": {"type": "class", "ids": ["7a-…"]},
  "title": "Libur sekolah",             // 1–120 chars, shown in the app only
  "body": "Sekolah libur 25 Desember.", // 1–2000 chars, shown in the app only
  "dryRun": true
}
```

`audience` has exactly one shape per type. Unknown fields are refused.

| type | ids | reaches |
|---|---|---|
| `school` | none | parents of every active enrolled student of the school |
| `grade` | grade codes (`Section.grade_code`, else `year_group`) | parents of students in active classes of those grades |
| `class` | `Section.section_id` | parents of students in those classes |
| `students` | `Student.studentId` | those students' parents |
| `staff` | none | active staff members of the school (`staff:<id>`) |

`ids`: 1–500, distinct, `[A-Za-z0-9_.:-]` (a grade may be free text up to 128 chars, e.g. `Year 8`). Every id must exist **in this school**. One foreign or
unknown id refuses the whole request (422 `UNKNOWN_AUDIENCE`). Parents are the usual eligible
Parent rule (verified/paid lead, not staff).

Answers:

- dry run: `200 {"data":{"dryRun":true,"audience":"class","recipients":12}}`. Nothing is written or sent.
- first send: `200 {"data":{"id":"…","recipients":12,"duplicate":false,"status":"sending"}}`. Notices
  are posted with the claim. The push runs in the background.
- same `id` again: `200 {"data":{…,"duplicate":true,"status":"sending"|"sent"}}`. Nothing is posted or sent again.
- `id` already used by another school: 409 `ID_IN_USE`.
- 400 `INVALID_INPUT`, 401, 403 `FORBIDDEN`, 503 `DEPENDENCY_UNAVAILABLE`.

## `GET /api/v1/sis-service/school-broadcasts/audiences?tenantId=&schoolId=`

Data for the admin picker:
`{"data":{"grades":[{"id":"Y7"}],"classes":[{"id","name","grade"}],"students":[{"id","name","classId"}]}}`.
Limits: 100 grades, 500 classes, 3000 student rows.

## Push

For each recipient, SIS calls notification-services `POST /api/push/v1/send` (`X-Internal-Token`),
5 at a time. Stale app tokens and expired browser subscriptions are removed there.

- Lock-screen text never contains the announcement. `id*` locale (`User.school_locale`) gets
  "Pengumuman sekolah" / "Ada pengumuman baru dari sekolah.". Any other locale gets "School announcement" /
  "There is a new announcement from the school.". Staff get Indonesian.
- `url`: `/parent` for parents, `/teacher` for staff.
- A Parent who turned off notifications or announcement notifications still gets the board notice
  but no push.
- The outcome is stored on `(:SchoolBroadcast {id})`: `status` `sending` → `sent`, `pushed`, `push_failed`.

Known limit: the fan-out runs in the SIS process. If the pod restarts mid-way, the rest is not pushed and
the broadcast stays `sending`. The notices are already posted. A retry with the same id does not resume it.
Staff have no board to read the text yet.

## Graph

- `(:SchoolBroadcast {id, school_id, tenant_id, actor_id, audience, audience_ids, title, body, recipients, status, created_at, pushed, push_failed, finished_at})`
  has a unique constraint `school_broadcast_id`, created on start.
- `(:User)-[:HAS_SCHOOL_NOTICE]->(:SchoolNotice {id:'broadcast:<id>:<User.id>', title, body, is_read:false})`.

## Environment

The env vars already exist for safety alerts: `NOTIFICATION_SERVICE_URL` and `NOTIFICATION_INTERNAL_TOKEN`
(must match notification-services). Without them, notices are still posted and the push is skipped
(logged, `push_failed` = all).

## Runbook: send or test

1. Admin portal → Artikel sekolah → "Kirim pengumuman". Pick the audience, check the count, confirm.
2. Without the UI, on a host port-forward to sis-service with an admin staff bearer:
   ```bash
   curl -sS -X POST "$SIS/api/v1/sis-service/school-broadcasts" -H "Authorization: Bearer $STAFF" -H 'Content-Type: application/json' \
     -d '{"tenantId":"…","schoolId":"…","id":"'"$(uuidgen)"'","audience":{"type":"students","ids":["<one test child>"]},"title":"Tes","body":"Tes pengumuman","dryRun":true}'
   ```
   Check `recipients`, then send the same body with `"dryRun":false`. Test with a `students` audience of a
   test child, never `school`, on a shared environment.
3. Check: `MATCH(b:SchoolBroadcast {id:$id}) RETURN b.status,b.recipients,b.pushed,b.push_failed`.
4. Undo a test notice: `MATCH(n:SchoolNotice) WHERE n.id STARTS WITH 'broadcast:'+$id DETACH DELETE n`.

Tests: `cargo test --bin sis-service school_broadcasts`. The graph tests need a disposable Neo4j:
`docker run --rm -d -p 127.0.0.1:3223:7687 -e NEO4J_AUTH=neo4j/parent-otp-fixture-password neo4j:5.26-community`, then
`SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223 cargo test --bin sis-service school_broadcasts -- --ignored --test-threads=1`.
