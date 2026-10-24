# sis-services

Student Information System microservice for the Digital School platform.

## Scope

Owns the post-enrolment side of the domain:

- **Sections** — homeroom groupings per year-group per school
- **Enrolments** — which `EnrolledStudent` is assigned to which `Section`
- **Attendance** — daily per-student attendance records
- **Grades** — per-subject, per-term grade entries

Sibling service to [admission-services](https://github.com/Cybe-Asia/admission-services), which owns the admissions-funnel side (Lead, Application, ApplicantStudent, Test, Document, Offer).

## Neo4j label ownership

Shared Neo4j cluster. Label ownership by service:

| Owned by admission-services | Owned by sis-services |
| --- | --- |
| `Lead`, `Application`, `:Student` (ApplicantStudent), `TestSchedule`, `TestSession`, `TestResult`, `DocumentRequest`, `DocumentArtifact`, `Offer`, `OfferAcceptance`, `AdmissionDecision` | `Section`, `AttendanceRecord`, `GradeEntry` |
| `EnrolledStudent` (created at offer acceptance) | `(EnrolledStudent)-[:ENROLLED_IN]->(Section)` edges |

sis-services reads `(Lead)`, `(User)`, `(:Student)`, `(EnrolledStudent)` but never writes them. All writes to those labels go through admission-services.

## Endpoints

### Admin (requires `ADMIN_EMAILS` allowlist)

- `POST   /api/leads/v1/admin/sis/sections` — create a Section
- `GET    /api/leads/v1/admin/sis/sections` — list with filters
- `GET    /api/leads/v1/admin/sis/sections/{id}` — detail + members
- `POST   /api/leads/v1/admin/sis/sections/{id}/assign` — assign students
- `POST   /api/leads/v1/admin/sis/sections/{id}/homeroom` — set homeroom teacher
- `POST   /api/leads/v1/admin/sis/sections/{id}/status` — active/archived
- `GET    /api/leads/v1/admin/sis/sections/{id}/attendance` — daily roster
- `POST   /api/leads/v1/admin/sis/sections/{id}/attendance` — bulk upsert
- `GET    /api/leads/v1/admin/sis/sections/{id}/grades` — per-subject/term roster
- `POST   /api/leads/v1/admin/sis/sections/{id}/grades` — bulk upsert

### Admin teacher list (staff portal session, school admin or Owner)

- `GET    /api/v1/sis-service/academic/teachers/context` — schools and academic years the caller administers
- `GET    /api/v1/sis-service/academic/teachers` — teachers, scheduled load and warnings for one school year and week
- `GET    /api/v1/sis-service/academic/teachers/{staffMemberId}` — one teacher with the week's schedule

Read-only, from published lessons, homerooms, grants and activity coaches; see `docs/TEACHER_LIST.md`.

### Parent (requires valid session)

- `GET    /api/leads/v1/me/grades` — all grades across all kids
- `GET    /api/leads/v1/me/attendance` — 14-day window attendance
- `GET    /api/leads/v1/me/sections` — sections each kid is assigned to

#### Parent school preferences

- `GET /api/leads/v1/me/school`: the snapshot's `preferences` is
  `{ version, locale, theme, notifications, notificationCategories }`, where
  `notificationCategories` is `{ classSchedule, announcements, messages }`
  (booleans) and is always present. A Parent who never saved categories gets
  three copies of `notifications`. `notifications` stays in the response for
  clients that predate the categories.
- `POST /api/leads/v1/me/school/preferences`: body
  `{ version, locale, theme, notifications, notificationCategories? }`.
  Unknown fields are rejected (`400 INVALID_INPUT`), also inside the object.
  - Without `notificationCategories` (or with `null`) the save behaves as
    before and sets all three categories to the value of `notifications`.
  - With it, all three booleans are required; they are stored as sent and
    `notifications` is stored as their OR, whatever the body said.
  - Unchanged: compare-and-set on `version` (`409 REVISION_CONFLICT` when
    stale), response `{ "responseCode": 200, "data": { "saved": true } }`,
    and one `SchoolPortalAudit` of kind `preferences` (ids only) per save.
- Storage: `school_notify_class_schedule`, `school_notify_announcements` and
  `school_notify_messages` on the Parent's `User`, next to the existing
  `school_notifications`. No constraint, index or migration. A release without
  the categories ignores the three properties; if it then changes
  `notifications`, the snapshot reports three copies of that newer value until
  the next save.
- The Student preferences endpoint
  (`/api/v1/sis-service/learning/owner/student/preferences`) is unchanged and
  does not accept `notificationCategories`.

#### Published grade detail

- `POST /api/leads/v1/teacher/school/classes/{section}/records` with
  `kind: "grade"` accepts an optional `detail` object:
  `{ assignment?, midterm?, final_exam?, subject_group?, subject_group_summary?,
  learning_outcome?, top_performer? }` (Tugas, PTS, PAS, rumpun and its
  description, Capaian Pembelajaran, the teacher's top-performer mark).
  Components are on the grade's own `maximum`; `subject_group` ≤ 128,
  `subject_group_summary` ≤ 256 and only with a group, `learning_outcome` ≤ 2000
  characters; `top_performer` defaults to `false`. Unknown fields are rejected.
- The detail is stored in the record payload only. `GradeEntry` (and
  `GET /me/grades`) keeps subject, term and score; the Parent reads the detail
  from the `grade` records in `GET /me/school`. A grade without `detail` is
  stored exactly as before, so earlier clients are unaffected.

URL prefix `/api/leads/v1/*` is inherited from the admissions convention; Traefik routes `…/sis/…` and the three `me/{grades,attendance,sections}` paths to this service. See `digital-school-gitops/lab/*/ingress.yaml`.

## Architecture

Layered, same as admission-services:

```
handlers/    HTTP surface (SIS admin + parent endpoints, auth helper)
routes/      Axum Router builders
repositories/  Cypher — one file per node label or cross-cutting concern
models/      Domain structs + status constants
utils/       JWT, response wrapper
config/      AppConfig from env
database/    neo4rs init
```

## Local dev

```bash
cp .env.example .env.local
cargo run
curl http://127.0.0.1:8081/api/v1/sis-service/health
```

Point `NEO4J_URI` at a local neo4j or port-forward `neo4j` from the cluster:

```bash
kubectl -n school-test port-forward svc/neo4j 7687:7687
```

## Deployment

Image: `ghcr.io/cybe-asia/sis-services:sha-<gitsha>`

Branch → overlay mapping:

- `main` → `lab/dev`
- `qa/*` → `lab/test`
- `staging/*` → `lab/staging` (manual promote to prod)
- `release-*` → `lab/staging` → auto-promote to prod

See `.github/workflows/deploy.yml` for the CI pipeline.

## Recitation submissions

Tahfiz recordings a Student submits after the AI pre-check are kept in the shared object store (`MINIO_*`, prefix `sis/recitations/`) and removed after their retention period by an in-process sweep; see `docs/RECITATION_SUBMISSIONS.md`.
