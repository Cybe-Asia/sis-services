# School nomenclature (Istilah sekolah, slices 1–3)

Added 2026-10-29 from `digital-school/docs/admin/academic-nomenclature-spec.md`. Each school keeps one list of
bilingual labels per kind: grades, subjects, terms, titles and the rest of spec §2. Admin, Teacher,
Student/Parent and the report card read the labels; codes stay fixed, and only labels change. The defaults are the
team's proposal and apply until the school owner answers spec §6.

## Model

| Node | Key | Holds |
|---|---|---|
| `AcademicNomenclature` | `tenant\|school\|kind\|code` | `label_id`, `label_en`, `short_id?`, `short_en?`, `order`, `status` (`active`/`archived`), `version` (the kind version at which the row last changed) |
| `AcademicNomenclatureKind` | `tenant\|school\|kind` | `version` of the whole list |
| `AcademicNomenclatureAudit` | uuid | `kind`, `action` (`seed`/`save`), actor, role, `before`/`after` (JSON), `version`, `command_key` |
| `AcademicNomenclatureReceipt` | `actor\|tenant\|school\|Idempotency-Key` | fingerprint and stored answer |

Constraints and indexes: `migrations/0008_academic_nomenclature.cypher`, run at start-up.

Kinds, in this order: `unit`, `grade`, `phase`, `stage`, `academic_year`, `term`, `subject`, `section_pattern`,
`assessment_kind`, `mastery_level`, `role_title`.

- **Closed kinds** (`unit`, `grade`, `phase`, `stage`, `section_pattern`, `assessment_kind`, `mastery_level`) take
  their codes from the seed and the curriculum rules. A save may relabel, reorder and archive them, but never add a
  code.
- **Open kinds** take new codes in their own format:
  - `subject`: the LMS subject code (`[a-z0-9][a-z0-9_-]{0,63}`);
  - `academic_year`: `YYYY/YYYY`, consecutive years;
  - `term`: `[A-Z][A-Z0-9]{0,7}`;
  - `role_title`: a role template code.

## Seed (spec §2)

A school's missing kinds are seeded at version 1 under the school lock (`EducationCalendarLock`). This happens the
first time the school is read or saved, so concurrent first reads seed once. The School must exist exactly once
for its tenant; otherwise the answer is 404 and nothing is created.

- `unit`: the school id without `SCH-` (IISS, IIHS, with the spec's names). Any other school gets its own code and
  `school_name`.
- `grade`: IISS gets Y7–Y9 and IIHS gets Y10–Y12; any other school gets Y7–Y12. Labels are `Kelas VII` / `Year 7`,
  with short labels `VII` / `Y7`.
- `phase` and `stage`: only those of the school's grades (spec §4).
- `academic_year`: every `YYYY/YYYY` year of the school's classes, plus the current year (July starts the year).
  Other forms such as `2026` are left for slice 5.
- `term`: S1 and S2. `assessment_kind`, `mastery_level`: the six and five codes of spec §2.
- `subject`: the active rows of the school's subject plans, latest year first. The label comes from the plan's name;
  the English label comes from the LMS name when the plan uses the standard Indonesian name. A school with no plans
  gets an empty list.
- `section_pattern`: `default`, `Kelas {grade_short}-{letter}` / `{grade}{letter}`.
- `role_title`: the four of spec §2, plus `school_admin`, `admin_teacher` and `regular_teacher`. Admin uses them for
  every template in Akses staf.

The fixed grade mappings of spec §4 (grade to unit, phase and stage) are code, not data, and the read returns them.

## Access

- **Read:** a staff portal session of an Owner or school administrator of the school, or a teacher session of the
  school. Student/Parent BFF reads come with slice 4.
- **Save:** a staff portal session of an Owner or school administrator. It also needs the capability decision
  `academic.nomenclature.manage` at the school (ADR 0018). Owners and the `school_admin` seed hold it; see
  `AUTHORIZATION.md` for tenants with a saved `school_admin` revision.
- A save runs in one transaction, in this order:
  1. caller lock;
  2. school lock;
  3. the capability decision;
  4. Auth introspection again;
  5. the receipt;
  6. the rows, the kind version, the audit and the receipt.

  It runs detached from the request, so a client that disconnects cannot cancel a commit.

## `GET /api/v1/sis-service/academic/nomenclature?schoolId&tenantId[&includeArchived=true]`

```json
{"data":{"contractVersion":1,"schoolId":"SCH-IISS","tenantId":"…","canManage":true,"cacheSeconds":300,
 "etag":"\"n1-11-14\"",
 "kinds":[{"kind":"grade","closed":true,"version":2,
   "rows":[{"code":"Y7","labelId":"Kelas VII","labelEn":"Year 7","shortId":"VII","shortEn":"Y7","order":1,"status":"active","version":2}]}],
 "mappings":{"grade":[{"code":"Y7","unit":"IISS","phase":"D","stage":"CLS7"}]}}}
```

- Active rows only, unless `includeArchived=true` is passed (Admin's editor). Rows are ordered by `order`.
- `canManage` is the capability decision for the caller. It is a menu hint; every save is checked again.
- **Caching.** Active reads carry `Cache-Control: private, max-age=300`, `ETag` and `Vary: Authorization`.
  - `If-None-Match` with the current ETag answers 304 without reading the rows.
  - The ETag is `"n1-<kinds>-<sum of kind versions>"`, and every save raises the sum.
  - `includeArchived` reads are `no-store`.

## `PUT /api/v1/sis-service/academic/nomenclature/{kind}`

Headers: `Idempotency-Key` (8–128 of `A-Z a-z 0-9 _ . : -`). Body: the whole list of one kind.

```json
{"tenantId":"…","schoolId":"SCH-IISS","expectedVersion":2,
 "rows":[{"code":"Y7","labelId":"Kelas VII","labelEn":"Year 7","shortId":"VII","shortEn":"Y7","order":1,"status":"active"}]}
```

Rules:

- 1–100 rows, one per code and one per `order` (1–999).
- Labels must be 1–80 characters (not bytes) in both languages, trimmed, with no control characters.
- Short labels are optional and at most 24 characters.
- `section_pattern` labels must contain `{letter}` and use no placeholders other than `{grade}`, `{grade_short}` and
  `{letter}`.
- `unit` and `section_pattern` keep at least one active row.

On success it answers `{kind, closed, version, updatedAt, rows}`. A row keeps its version when only other rows
changed.

| Status | Code | When |
|---|---|---|
| 400 | `INVALID_INPUT` | unknown kind or field, bad id, missing key, a row breaking the rules above |
| 401 / 403 | `UNAUTHORIZED` / `FORBIDDEN` | no session; not an Owner or admin of the school; no `academic.nomenclature.manage` |
| 404 | `NOT_FOUND` | the School does not exist once for the tenant |
| 409 | `REVISION_CONFLICT` (`version`) | `expectedVersion` is not the stored version; `version` is the current one |
| 409 | `CODE_REMOVED` (`codes`) | a stored code is missing from the list (archive instead) |
| 422 | `CODE_NOT_ALLOWED` (`codes`) | a new code in a closed kind |
| 409 | `IDEMPOTENCY_KEY_REUSED` | the same key with another body; the same key and body returns the stored answer |

## Consumers

- **Admin, slices 2–3:**
  - Akademik → Istilah sekolah edits the lists.
  - The class list, class detail, create-class form, master mapel (grade tabs, subject names, semester tabs) and
    Akses staf (role titles, subject and grade names in scopes) read the labels.
  - Each falls back to today's string when a label is missing or SIS does not answer.
- **Slices 4–5** (not built): learning-web and the Student/Parent client through their BFFs, and normalising
  `Section.yearGroup` and `academicYear` to codes. `model::grade_code` is the parser that normalisation will use.

## Tests

- `cargo test --bin sis-service academic_nomenclature`: seeds per unit, codes, labels, patterns, grade parsing.
- `SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223 cargo test --bin sis-service academic_nomenclature -- --ignored`, on a
  disposable Neo4j. It covers:
  - concurrent first reads seeding once;
  - ETag;
  - revisions, replay, key reuse, `CODE_REMOVED` and `CODE_NOT_ALLOWED`;
  - archive;
  - open kinds;
  - who may save;
  - a save as the school's first touch.
- The local browser check against this build is in `digital-school/docs/qa/academic-nomenclature/`.
