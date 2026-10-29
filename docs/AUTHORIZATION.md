# Staff authorization: capabilities, role templates and assignments (ADR 0018)

Added 2026-10-28. This is slice 1 of `digital-school/docs/admin/rbac-spec.md` §7 and
`digital-school/docs/adr/0018-capability-based-access-control-for-school-staff.md`.

Code checks **capabilities**. A **role template** is a versioned set of capabilities. An **assignment** gives
one person one template at one scope. People also hold access **derived** from today's records, which are not
stored twice.

**No service enforces capabilities yet.** Every existing role check stays as it is until that service moves
(spec §7 slices 5–6). This slice only answers questions and records assignments.

## Catalogue

The catalogue is `src/authorization/capabilities.json`, a reviewed file that is loaded and checked at start-up.
A bad file stops the service. Admin cannot add a capability. Each entry has:

- `code`: `<area>.<resource>.<action>` or `<area>.<action>`;
- `service`: the service that will enforce it;
- `area`: `learning`, `academic`, `school`, `staff`, `student`, `billing` or `authorization`;
- `sensitivity`: `normal` or `sensitive`;
- `classWide`;
- `description`: `{id, en}`.

`classWide` marks a capability that applies to a class as a whole: `learning.class.teach` and
`learning.class.homeroom`. Holding it for one subject of a class holds it for the class, as a teaching grant per
class does today.

Sensitive capabilities are `student.biodata.manage`, `billing.payment.manage` and `authorization.manage`. An
unknown code counts as sensitive.

## Seed templates

The seeds are in `src/authorization/templates.json`. They are defaults the school may edit.

- Until a tenant saves a seed, it reads as version 1 with `seeded: true`.
- The first save of a seed stores the seed as version 1 and the change as version 2.
- No migration Job is needed.

| Code | Allowed scopes | Capabilities |
|---|---|---|
| `regular_teacher` | `SECTION_SUBJECT`, `SECTION`, `SCHOOL_SUBJECT` | `learning.class.teach`, `learning.module.write`, `learning.module.publish`, `learning.curriculum.read`, `learning.objective_tag.write`, `learning.mastery.read`, `learning.gradebook.read` |
| `admin_teacher` | `SCHOOL`, `SCHOOL_SUBJECT`, `SECTION` | Regular teacher + `learning.module.review`, `learning.module.manage_any`, `academic.staffing.read` |
| `subject_lead` | `SCHOOL_SUBJECT` | `learning.module.review`, `learning.module.manage_any`, `learning.plan.manage`, `learning.question_bank.manage` |
| `curriculum_coordinator` | `SCHOOL` | `learning.curriculum.manage`, `learning.plan.manage`, `learning.module.review`, `learning.module.manage_any`, `learning.mastery.read` |
| `school_admin` | `SCHOOL` | `academic.staffing.read`, `academic.staffing.manage`, `academic.subject_plan.manage`, `academic.calendar.manage`, `academic.class_access.manage`, `academic.extracurricular.manage`, `school.content.manage`, `staff.directory.manage`, `student.biodata.manage`, `authorization.read` |
| `principal` | `SCHOOL` | `authorization.manage`, `authorization.read`, `academic.staffing.read`, `learning.curriculum.read`, `learning.mastery.read` |

Two seeds go further than spec §3, so that day-one access equals today:

- **Regular teacher** adds `learning.class.teach`. That is what a teaching grant gives today.
- **School admin** lists everything a `school_admin` may do in SIS today: calendar, rooms and timetable, class
  Learning access, extracurricular, articles and student biodata. It also adds `authorization.read`, so a school
  admin can see access in their own schools.

**Principal** (Kepala Sekolah, added 2026-10-29 for spec §10 #8.3) is the one seed that carries
`authorization.manage`, at `SCHOOL` only, so one person per unit can manage access without waiting for the
foundation. Only an Owner may give it (`PRINCIPAL_NEEDS_OWNER` below), whatever the template carries after a
revision. The catalogue check refuses any other seed that carries `authorization.manage`, and a Principal seed
with another scope. A Principal manages access in their school under the same guardrails as everyone else: they
revoke and change validity, but grant only capabilities they hold themselves, so with this seed they cannot
hand out the Learning templates (`CAPABILITY_NOT_HELD`). Widening the seed is a product decision.

## Scopes

`{"type": …, "schoolId"?, "sectionId"?, "subject"?, "gradeFrom"?, "gradeTo"?}`. The tenant is always the
request's `tenantId`.

| Type | Fields | Covers |
|---|---|---|
| `TENANT` | none | everything in the tenant |
| `SCHOOL` | `schoolId` | the school, its subjects and its classes |
| `SCHOOL_SUBJECT` | `schoolId`, `subject`, optional `gradeFrom`/`gradeTo` | that subject (in that grade range) and that subject in each class of a grade in range; a class-wide capability also covers those classes |
| `SECTION` | `schoolId`, `sectionId` | the class and every subject in it |
| `SECTION_SUBJECT` | `schoolId`, `sectionId`, `subject` | that subject in that class; a class-wide capability also covers the class |

Grades compare by their first number ("Year 7" → 7). A class with no grade is outside any range. A check on a
school that is not unique in the tenant is denied (`SCOPE_NOT_FOUND`), and so is a check on a class that is not
an active class of that school.

## Derived holdings (day one equals today)

The order is the order in which a decision looks:

1. **Owner.** An active Owner without teams holds every capability:
   - unscoped: at `TENANT`;
   - scoped: at `SCHOOL` for each school in its Auth scope.

   This is `staff_capability::owner`. An Owner holds nothing else: explicit assignments do not add to an Owner,
   because today's checks never combine Owner with another role.
2. **Explicit assignments.** An assignment counts only while all of these hold:
   - it is active and today (UTC+7) falls between `validFrom` and `validUntil`;
   - its template is active;
   - the person is still in that school in Auth;
   - its class is still an active class of the school, and of the assignment's `academicYear` when it names
     one.
3. **Auth role `school_admin`.** The person gets *School admin* at `SCHOOL` for each school. This is
   `staff_capability::authority(…'school_admin'…)`. An explicit `school_admin` assignment at that school
   replaces this row.
4. **Teaching records.** These follow the `TEACHER` projection of `/learning/owner/context`. They require an
   active `teacher`, not an Owner, of the class's school, and an active class:
   - **Teaching assignment** (`teaching_assignment`): *Regular teacher* at `SECTION_SUBJECT`, while the
     teacher's grant to the class is `ACTIVE`. The grant stays the gate, so a grant revoked by hand also ends the
     access of an assignment that is still open, as today.
   - **Grant without an assignment** (`teaching_grant`): *Regular teacher* at `SECTION`.
   - **Homeroom** (`homeroom`): *Regular teacher* plus `learning.class.homeroom` at `SECTION`. A homeroom teacher
     teaches their class today, and only they have homeroom access (ADR 0015).

Derived rows use the **current version** of `regular_teacher` and `school_admin`. Editing those templates
changes derived access, and the impact read says for how many people. Neither template can be archived.

**One deliberate difference from today:** a staff member id with two `StaffMember` nodes holds nothing. Today
the Owner and school-admin checks already refuse such an id, but the teacher projection does not. This is
corrupt data, and failing closed is the safe answer.

The graph test `authorization_graph_day_one_equals_todays_checks` runs today's Cypher next to the new decision
for 12 people × 5 classes × {teach, homeroom} and 12 people × 2 schools × 3 school-admin capabilities. The
people include:

- Owners (unscoped, scoped, with a team);
- school admins (own school, other school, disabled);
- a homeroom teacher, an assigned teacher and a grant-only teacher;
- a teacher with an open assignment and a revoked grant;
- a disabled teacher and a teacher of another school.

All 192 answers match.

## Access to these routes

The caller is any active staff portal session (Auth `active`, a staff member id and at least one role), so that a
Principal who is a `teacher` in Auth reaches these routes; every other SIS Admin route still needs an `owner` or a
`school_admin`. The session must cover the tenant: an unscoped Owner, or `tenantId` in its tenants. What the caller
reads or changes is then decided from their own holdings only, as below; a teacher without `authorization.read`
or `.manage` gets 403 on every read but their own `effective`. Audit entries name a teacher's role `staff`.

- **Reads:** only schools where the caller holds `authorization.read` or `authorization.manage`, and that their
  session covers. The caller needs at least one such school, or 403.
  - A tenant-wide holding is shown as `TENANT` only to someone who reads the whole tenant. Others see it once
    per readable school.
  - People outside the readable schools are `404 NOT_FOUND`.
- **Writes** need `authorization.manage` (Owners by default) and pass every guardrail below.
- Every response is `Cache-Control: no-store`.

## Guardrails (all enforced in SIS)

| Rule | Error |
|---|---|
| A template change needs `authorization.manage` for the whole tenant (at `TENANT`, or at every school of it) | 403 `FORBIDDEN` |
| An assignment change needs `authorization.manage` at its scope | 403 `FORBIDDEN` (`index`) |
| Nobody grants a capability they do not hold at the assignment's scope (or, for a template, across the tenant) | 403 `CAPABILITY_NOT_HELD` (`index`, `capability`) |
| No scope wider than one's own: holding at a school never covers another school or `TENANT` | 403 `FORBIDDEN` |
| A sensitive capability is granted only by an Owner of the scope (of the whole tenant for templates) | 403 `SENSITIVE_NEEDS_OWNER` |
| The `principal` template is assigned (or its dates changed) by an Owner of the school only | 403 `PRINCIPAL_NEEDS_OWNER` (`index`) |
| A school that had a holder of `authorization.manage` keeps one after the change | 409 `LAST_AUTHORIZATION_MANAGER` (`schoolId`) |
| A school admin acts only in their schools | follows from the rows above |

## Writes: revisions, `Idempotency-Key`, audit

These follow the same rules as `docs/TEACHING_ASSIGNMENTS.md`:

- **`Idempotency-Key`:** 8–128 characters. The same key and body replays the stored answer. The same key with
  another body is 409 `IDEMPOTENCY_KEY_REUSED`.
- **Revisions:** `expectedVersion` (0 creates). Any other version is 409 `REVISION_CONFLICT` (`index`).
- **Batches:** 1–20 commands, all or nothing. A command error names its `index`.

One transaction runs these steps in order:

1. lock the caller's `StaffMember`;
2. lock the tenant (`AuthorizationTenantState`);
3. check Auth introspection again;
4. check the receipt;
5. read the tenant's facts under the locks;
6. check the guardrails;
7. write the changes and their audit;
8. run the last-manager check;
9. raise `authorizationVersion` by one;
10. store the receipt.

Writes run detached from the HTTP request.

Every change writes an `AuthorizationAudit` with:

- `kind` (`template` or `assignment`), `action` and `record_key`;
- `actor_id`, `actor_role`, tenant and school;
- `template_code` and `template_version`;
- `before` and `after` as JSON;
- `authorized_by`, the holding that allowed it (for example the Owner source, or the assignment id and template
  version);
- `command_key` and `created_at`.

No personal data is logged. An unknown capability in a check is logged by its code only.

## Caching

Read answers carry `authorizationVersion` (the tenant's counter) and `cacheSeconds: 60`. A caller may keep an
answer for at most 60 seconds, and drops it sooner when it sees a higher version. A revocation therefore takes
effect at the next request after the cache expires or the version changes.

## Endpoints

All under `/api/v1/sis-service/authorization`. Bodies are `{"data": …}` or `{"error": {"code": …}}`.

### `GET /capabilities?tenantId`

`{"version":1,"capabilities":[{"code","service","area","sensitivity","classWide","description":{"id","en"}}]}`

### `GET /templates?tenantId`

`{"templates":[{"code","name":{"id","en"},"version","capabilities","allowedScopes","status","seeded","updatedAt","people"}],"authorizationVersion","cacheSeconds"}`.
`people` counts the people who hold the template now, explicitly or derived. `canEditTemplates` (manage across the whole tenant) and
`canGrantSensitive` (an Owner of every school) are hints for Admin menus. Every write checks again.

### `GET /templates/history?tenantId&code`

`{"code","versions":[{"version","name","capabilities","allowedScopes","status","seed","actorName","at"}]}`, newest
first. Admin computes the diff between two versions.

### `GET /templates/impact?tenantId&code`

`{"code","version","people","sources":{"assignment":1,"teaching_grant":2,"homeroom":1,…},"schools"}`. This is
whom a change to the template reaches today. Admin shows it before saving.

### `POST /templates/commands`

```json
{"tenantId":"TENANT-001","commands":[
  {"action":"revise","code":"admin_teacher","name":{"id":"Guru admin","en":"Admin teacher"},
   "capabilities":["learning.class.teach","learning.module.review"],"allowedScopes":["SCHOOL","SECTION"],"expectedVersion":1},
  {"action":"create","code":"exam_officer","name":{"id":"Petugas ujian","en":"Exam officer"},
   "capabilities":["learning.question_bank.manage"],"allowedScopes":["SCHOOL"],"expectedVersion":0},
  {"action":"archive","code":"subject_lead","expectedVersion":1}]}
```

The answer is `{"templates":[…the saved template…],"authorizationVersion":7}`. Errors besides the guardrails:

| Status | Code | When |
|---|---|---|
| 409 | `TEMPLATE_REQUIRED` | archiving `regular_teacher` or `school_admin` |
| 409 | `TEMPLATE_IN_USE` | archiving with active assignments |
| 409 | `TEMPLATE_ARCHIVED` | revising an archived template |
| 409 | `SCOPE_IN_USE` | removing an allowed scope that an active assignment uses |
| 422 | `UNKNOWN_CAPABILITY` | a code that is not in the catalogue (`capability`) |

Codes are 2–48 characters of `a-z 0-9 _`, starting with a letter. `owner`, `student`, `parent` and `homeroom`
are reserved.

### `GET /assignments?tenantId[&schoolId&templateCode&staffMemberId&sectionId&subject&status&includeDerived]`

The answer has:

- `assignments`: explicit rows (`id`, `staffMemberId`, `name`, `templateCode`, `templateName`,
  `templateVersion`, `scope`, `scopeLabel`, `academicYear`, `validFrom`, `validUntil`, `status`, `version`,
  `current`, `derived:false`) and derived rows (`source`, `scope`, `scopeLabel`, `templateName`,
  `capabilities`, `staffMemberId`, `name`, `derived:true`);
- `people`, `schools`, `sections` and `templates`, for the pickers;
- `tenantWide`;
- `manageSchools` and `ownerSchools`: the readable schools where the caller may change assignments, and where
  they are an Owner (sensitive capabilities). These are menu hints; every write checks again.

`status` is `active` (the default), `revoked` or `all`. Derived rows are always active. `includeDerived`
defaults to `true`.

### `POST /assignments/commands`

```json
{"tenantId":"TENANT-001","commands":[
  {"action":"assign","staffMemberId":"staff-1","templateCode":"subject_lead",
   "scope":{"type":"SCHOOL_SUBJECT","schoolId":"SCH-IISS","subject":"mathematics","gradeFrom":"Year 7","gradeTo":"Year 9"},
   "academicYear":"2026/2027","validFrom":"2026-07-01","validUntil":"2027-06-30","expectedVersion":0},
  {"action":"change_validity","assignmentId":"…","validUntil":"2026-12-31","expectedVersion":1},
  {"action":"revoke","assignmentId":"…","expectedVersion":2}]}
```

| Action | Applies when | Result |
|---|---|---|
| `assign` | no record for this person, template, scope and year (`expectedVersion` 0), or a revoked one at its version (reopens it) | `active` |
| `change_validity` | an `active` record at `expectedVersion` | new dates; the grant checks run again |
| `revoke` | an `active` record at `expectedVersion` | `revoked`, kept for audit |

Errors besides the guardrails:

| Status | Code | When |
|---|---|---|
| 404 | `NOT_FOUND` | `assignmentId` is not in the tenant |
| 422 | `TEMPLATE_NOT_FOUND` | unknown or archived template |
| 422 | `SCOPE_NOT_ALLOWED` | the template does not allow the scope type |
| 422 | `SCOPE_NOT_FOUND` | no unique school, no active class of the school, or a class of another `academicYear` |
| 422 | `TARGET_NOT_ELIGIBLE` | not one active Staff Member of that school (or tenant), or an Owner |

Dates are `YYYY-MM-DD`, with `validFrom` ≤ `validUntil`.

**Default end (spec §10 #8.4).** An `assign` or `change_validity` without `validUntil` ends with the academic
year, except templates `principal` and `curriculum_coordinator`, which run until revoked. The academic year is the
one the command names (`academicYear`, when it is `YYYY/YYYY` and not already over); otherwise the year of the
school's active classes (`Section.academic_year`, `YYYY/YYYY` or `YYYY-YYYY`) that holds `validFrom` (or today,
UTC+7); otherwise the calendar year from 1 July. An academic year runs 1 July to 30 June; a per-school setting
may replace this later. A tenant-wide assignment looks at the classes of every school. Every `assign` and
`change_validity` row in the answer says which rule applied:

```json
"validity":{"rule":"academic_year_end","academicYear":"2026/2027","source":"classes"}
```

`rule` is `explicit` (the command named the end), `until_revoked` or `academic_year_end`; `source` is
`command`, `classes` or `calendar`. The saved record carries only the dates.

### `GET /assignments/expiring?tenantId[&days&schoolId]`

Active explicit assignments in the readable schools that are in force today and end within `days` (1–366,
default 30), soonest first: the rows of `GET /assignments` plus `daysLeft`, `renewUntil` and
`renewAcademicYear` (the end of the academic year that follows, by the rule above) and `canRenew` (the reader
manages access in that school; a tenant-wide row needs `authorization.manage` across the tenant). Also `today`
and `days`. Same session and read rules as the other reads; renewing is a `change_validity` command.

### `POST /check`

```json
{"tenantId":"TENANT-001","actor":"staff-1","capability":"learning.module.review",
 "scope":{"type":"SECTION_SUBJECT","schoolId":"SCH-IISS","sectionId":"SEC-7A","subject":"mathematics"}}
```

`actor` defaults to the caller. Asking about someone else needs read access in the scope's school. The answer:

```json
{"allowed":true,"reason":"ALLOWED","actor":"staff-1","capability":"learning.module.review","scope":{…},
 "scopeLabel":{"schoolName":"IISS","sectionName":"7A","grade":"Year 7","academicYear":"2026/2027"},
 "source":{"kind":"assignment","id":"…","templateCode":"subject_lead","templateVersion":2},"templateVersion":2,
 "heldAt":[…holdings with this capability elsewhere…],"authorizationVersion":7,"cacheSeconds":60}
```

`reason` is one of:

- `ALLOWED`;
- `UNKNOWN_CAPABILITY`: a deny, with a log line naming the code;
- `SCOPE_NOT_FOUND`;
- `NO_ACCESS`: the person holds nothing;
- `NOT_IN_SCOPE`: they hold something, not this here; `heldAt` says where they do hold it.

### `GET /effective?tenantId&staffMemberId`

```json
{"person":{"staffMemberId","name","active","roles","owner"},
 "holdings":[{"source","scope","scopeLabel","templateName","capabilities","derived"}],
 "capabilities":{"learning.class.teach":[{scope},…]},"authorizationVersion","cacheSeconds"}
```

This is for the Admin Access tab. When `staffMemberId` is the caller, the answer needs no read capability: it
lists the caller's own holdings in the schools their session covers (tenant-wide ones as `TENANT`), and a disabled
or unknown caller gets 403. Admin builds its gate and menus from this self-read.

### `GET /audit?tenantId`

The newest 200 entries in the readable schools, with `before` and `after` parsed. Template entries are visible
to every reader. A tenant-wide assignment entry is visible only to tenant-wide readers.

## `/learning/owner/authorize` (existing)

The contract is unchanged (`contractVersion` 1, `allowed`). For a `teacher` or `owner` actor the answer adds
`capabilities`: the `learning.*` codes the person holds in that class. learning-api's `OwnerData` does not deny
unknown fields, so the current learning-api still parses it. Nothing reads it until learning-api moves (spec §7
slice 5).

## `GET /learning/owner/capabilities?school_id&tenant_id` (ADR 0018 slice 5)

This lets Learning check school and subject roles, for example a curriculum coordinator or subject lead who in Auth
is an ordinary `teacher`. `/learning/owner/authorize` answers per class only, and `/authorization/check` takes only
Admin sessions.

- **Caller:** authenticated and pinned to the school and tenant exactly as for `/learning/owner/authorize`: the
  Teacher or Owner session, with the school and tenant in its lists.
  - Students and Parents get 403.
  - There is no `actor` parameter: the answer is always about the caller.
- **Content:** only `learning.*` codes, and only holdings in the pinned school. `TENANT` holdings count; another
  school's holdings never appear.
- **Sources:** the derived holdings count as everywhere else: Owner, `school_admin`, teaching assignments while the
  grant is active, grants and homeroom.
- **Holding nothing:** an active Staff Member who holds nothing gets `{"capabilities":{}}`.
- **Refusals:** a Staff Member who is not one active node gets 403, as from authorize.
- **Headers and logs:** the response is `Cache-Control: no-store`, and nothing personal is logged.

For a subject lead at mathematics Year 7–9 who also has a teaching grant to 9A, the answer is:

```json
{"data":{"contractVersion":1,"authorizationVersion":1,"cacheSeconds":60,"capabilities":{
  "learning.class.teach":[{"type":"SECTION","schoolId":"school","sectionId":"9A"}],
  "learning.module.review":[{"type":"SCHOOL_SUBJECT","schoolId":"school","subject":"mathematics","gradeFrom":"Year 7","gradeTo":"Year 9"}],
  "learning.plan.manage":[{"type":"SCHOOL_SUBJECT","schoolId":"school","subject":"mathematics","gradeFrom":"Year 7","gradeTo":"Year 9"}],
  "…":[]}}}
```

Each code maps to the scopes where it is held, each scope once. Containment works as in "Scopes" above: a
`SCHOOL_SUBJECT` holding covers that subject in each class of a grade in range, and a class-wide capability held at
`SECTION_SUBJECT` covers the class. Callers may keep the answer for `cacheSeconds`, or until `authorizationVersion`
changes.

The graph test `authorization_graph_learning_scopes_per_school` covers:

- an Owner (every `learning.*` at `TENANT`);
- a coordinator at `SCHOOL`, next to their homeroom rows;
- a subject lead at `SCHOOL_SUBJECT`;
- a plain teacher (`SECTION_SUBJECT` only);
- another school's coordinator (`{}` here, their own school answers);
- disabled and unknown people (refused);
- a teacher who holds nothing (`{}`).

## `GET /learning/owner/mastery-roster?school_id&tenant_id[&subject&grade]` (ADR 0018 remainder)

The classes and students the caller may read mastery for, so Learning can show the Mastery panel to a
curriculum coordinator or subject lead who teaches no class (`/learning/owner/context` lists taught classes only).

- **Caller:** authenticated and pinned exactly as `/learning/owner/capabilities`: a Teacher or Owner session with
  the school and tenant in its lists; Students and Parents get 403; the answer is always about the caller.
- **Decision:** a class is listed when the caller holds `learning.mastery.read` for it, through any holding:
  Owner, `SCHOOL` (a coordinator or Principal), `SCHOOL_SUBJECT` (a subject role, for the classes of its grades),
  `SECTION` (a grant or homeroom) or `SECTION_SUBJECT` (a teaching assignment). Explicit and derived holdings count
  alike; another school's holdings never do.
- **`subjects`:** `null` when the caller reads every subject of the class; otherwise the subject codes the
  holding covers. With `?subject=`, only classes where that subject is covered are listed, with `subjects:
  [subject]`. `?grade=` keeps one grade, compared by its number ("Year 7" or "7").
- **Students:** the class's active enrolments (`EnrolledStudent {status:'active'}`), id and name.
- **Refusals:** a Staff Member who is not one active node gets 403. A subject lead whose seed carries no
  `learning.mastery.read` gets `classes: []` until the school revises that template.
- **Headers and logs:** `Cache-Control: no-store`; nothing personal is logged. Callers may keep the answer
  `cacheSeconds`, or until `authorizationVersion` changes.

```json
{"data":{"contractVersion":1,"schoolId":"school","tenantId":"tenant","authorizationVersion":5,"cacheSeconds":60,
 "classes":[{"id":"7B","name":"7B","grade":"Year 7","academicYear":"2026/2027","subjects":["mathematics"],
             "students":[{"id":"st-1","name":"Aisha"},{"id":"st-2","name":"Bilal"}]}]}}
```

Bounds: 5000 active classes of the school and 20000 roster rows; past a bound the answer is 503.

The graph test `authorization_graph_principal_and_mastery_roster` covers the Principal guardrails (school admin
refused, `SCHOOL` only, Principal cannot name a Principal, hold-what-you-grant, revoke and reopen) and the roster
for an Owner, a Principal, a teaching assignment (one subject), a grant (whole class), a revoked grant, another
school's teacher, disabled and unknown people, a coordinator, a subject lead before and after a template revision,
and the grade and subject filters.

## Services that check capabilities (spec §7 slice 6)

`authorization::reads::permits(db, tenant, school, staff, capability)` answers "does this person hold the
capability for the whole school now" with the same decision as `/authorization/check` at `SCHOOL`; inside a write
transaction it reads the facts under the caller's locks, so an assignment or template revoked meanwhile denies.

| Routes (`academic_staffing`) | Capability |
|---|---|
| `GET /academic/subject-plans`, `GET /academic/teaching-assignments`, `GET /academic/grade-weights` | `academic.staffing.read` |
| `PUT /academic/subject-plans`, `POST /academic/subject-plans/copy`, `PUT /academic/grade-weights` | `academic.subject_plan.manage` |
| `POST /academic/teaching-assignments/commands` | `academic.staffing.manage` |

The Auth session gate is unchanged: these routes still take only Owner and `school_admin` staff portal sessions
pinned to the school, so on day one the people who pass are exactly those who passed the role fragment. What
changes is that a revision of the School admin template, or an explicit assignment, now reaches these routes
(graph test `staffing_routes_decide_by_capability`). `GET /learning/grade-weights` (teachers' read) keeps its role
fragment.

## Schema

Applied at start-up from `migrations/0007_authorization.cypher` (`authorization::init`). The statements are
idempotent.

- **Uniqueness constraints:**
  - `AuthorizationRoleTemplate.key` (`tenant|code`);
  - `AuthorizationRoleTemplateVersion.key` (`tenant|code|version`);
  - `AuthorizationAssignment.key` and `.id`;
  - `AuthorizationAudit.key`;
  - `AuthorizationReceipt.key`;
  - `AuthorizationTenantState.tenant_id`.
- **Indexes:** templates by tenant, versions by tenant and code, assignments by holder, class and status, and
  audit by tenant and time.

There is no backfill. **Rollback:** an image without this module ignores these labels, and every service still
checks roles as before.

## Bounds

Per tenant:

- 3000 staff, 500 schools and 5000 active classes;
- 10000 active grants and 10000 active teaching assignments;
- 5000 explicit assignments, 200 templates, 5000 listed rows and 200 audit entries.

Past a bound the answer is 503, never a partial decision.

## Tests

- `cargo test --bin sis-service authorization_` covers:
  - the catalogue and seeds, including that no seed holds `authorization.manage`;
  - scope validation and the containment matrix;
  - day-one holdings for every derived source;
  - dates, archived templates, leaving a school, and Owners not combining;
  - edited templates reaching derived access;
  - tenant coverage and readable schools;
  - command validation.
- **Graph tests** (disposable Neo4j only):
  `SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223 cargo test --bin sis-service authorization_graph -- --ignored --test-threads=1`.
  - The day-one equality matrix above, and the Learning authorize capabilities.
  - The Principal guardrails and the mastery roster (`authorization_graph_principal_and_mastery_roster`).
  - The staffing routes deciding by capability (`academic_staffing::database_tests::staffing_routes_decide_by_capability`).
  - Replay, key reuse and stale revisions.
  - Every 422 with its index, and that nothing of a refused batch is written.
  - Each guardrail: school admin without manage, wider scope, capability not held, sensitive needs Owner, last
    manager.
  - Validity change, revoke and reopen.
  - The default end per academic year and the expiring read
    (`authorization_graph_validity_default_and_expiring`).
  - The self-read behind Admin's gate, for a teacher who becomes Principal of one school
    (`authorization_graph_own_holdings_for_admin_gate`).
  - Template revise of a seed with history, the impact read, every template 409/422, and archive.
  - Reads limited to the reader's schools.
  - Audit counts with before, after and `authorized_by`, and the tenant version.
