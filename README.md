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

### Parent (requires valid session)

- `GET    /api/leads/v1/me/grades` — all grades across all kids
- `GET    /api/leads/v1/me/attendance` — 14-day window attendance
- `GET    /api/leads/v1/me/sections` — sections each kid is assigned to

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
