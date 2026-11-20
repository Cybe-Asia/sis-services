# Student push (Student app)

2026-11-20. Push notifications for the Student app (Compose Multiplatform). Plan and decisions:
`digital-school/docs/notifications/student-push.md`. SIS owns who a Student is, which class they are in and their
notification settings; notification-services stores devices and sends (FCM); Learning tells SIS what was published.

## Device registration (Student app)

Header: `Authorization: Bearer <Student access token>` (auth-services `POST /api/v1/auth-service/login`). The
Student is checked as for Student preferences: signed HS256 `role=student` token plus the live Auth Student
session, then an `ACTIVE` `LearningStudentBinding` to a Student enrolled in an active class of the school.

### `POST /api/v1/sis-service/learning/owner/student/push-devices?schoolId=…&tenantId=…`

```json
{"installationId":"<UUID made once per install>","token":"<FCM registration token>","platform":"android|ios","appVersion":"1.0.0+3","locale":"id"}
```

`200 {"saved":true}`. Call after sign-in, on every app start and when FCM rotates the token. Stored in
notification-services (`PUT /api/push/v1/devices`) as recipient `student:<User.id>`; the body cannot name the
recipient (unknown fields are 400). An install another Student used moves to the caller.

### `DELETE` the same path, body `{"installationId":"…"}`

`200 {"removed":true}`. Call on sign-out, before the session token is dropped. Removes the install only if it is
the caller's.

Rules from the Parent app apply: `installationId` is a random UUID, not a hardware id; on iOS send the Firebase
token, not the APNs token; never log the token. notification-services keeps the newest 10 devices per recipient and
prunes a device idle for 60 days.

## Class push (Learning)

### `POST /api/v1/sis-service/learning/student-push?schoolId=…&tenantId=…`

Header: the Teacher's or Owner's own bearer, as Learning already forwards it to `learning/*`.

```json
{"kind":"assignment.published","ref":"<assignment id>","classId":"<section id>"}
{"kind":"assessment.published","ref":"<assessment id>","classId":"<section id>"}
{"kind":"report_card.published","ref":"<publication id>","classId":"<section id>","studentIds":["<Student.studentId>"]}
```

`202 {"data":{"recipients":N,"duplicate":false}}`; the push runs after the answer.

- The caller must pass `class_actor` for the class (homeroom, teaching grant, or Owner scope) with role Teacher or
  Owner. Students and Parents get 403. Every `studentIds` entry (1–60, unique) must be enrolled in the class, else 403.
- Recipients: Student accounts with an `ACTIVE` binding to an active enrolment in the class. A Student is pushed only
  when `student_notifications` is on and the kind's category is on (`assignments` for assignments and quizzes,
  `grades` for report cards; same rule as `StudentNotificationCategories::stored`).
- Once per fact: `(:StudentPush {key:"<tenant>|<school>|<kind>|<class>|<ref>"})`, unique. A repeat answers
  `duplicate:true` and pushes nothing.
- At most 20 new facts per class per day (Asia/Jakarta): 429 `RATE_LIMITED`.
- 21:00–06:00 WIB: the claim is recorded and nobody is pushed (skipped, not delayed).
- Lock screen text is fixed by kind and `student_locale` (`id…` Indonesian, otherwise English), never a title,
  name, subject or score. `url`: `/lessons?assignment=<ref>`, `/tests?assessment=<ref>`, `/report-cards`.
- Outcome on the claim: `status sending→sent`, `recipients`, `pushed`, `push_failed`, `skipped`.

Errors use `{"error":{"code":…}}`: 400 `INVALID_INPUT`, 401 `UNAUTHORIZED`, 403 `FORBIDDEN`, 429 `RATE_LIMITED`,
503 `DEPENDENCY_UNAVAILABLE` (graph, Auth, or notification-services not configured/unreachable).

## Configuration

No new settings: `NOTIFICATION_SERVICE_URL` and `NOTIFICATION_INTERNAL_TOKEN` (already on staging for broadcasts),
`AUTH_STUDENT_SESSION_URL`, `JWT_SECRET`. On start SIS creates `student_push_key` (IF NOT EXISTS, new label only).

## Tests

- Unit (`student_push::tests`): fixed text per kind and language, kind → category, quiet hours, body validation
  (unknown kind, extra fields, `recipientId`, bad `studentIds`).
- Disposable graph (`student_push::database_tests`, ignored; `SIS_STUDENT_TEST_NEO4J_URI=bolt://127.0.0.1:3385`
  plus `_USER`/`_PASSWORD`): recipients by binding, class, switch and category; `studentIds` outside the class; claim
  fresh/duplicate/capped; register/remove through synthetic Auth and notification-services (recipient from the
  token, 401 without bearer, 403 revoked binding); a Student token cannot ask for a class push; the fan-out sends
  only to the willing Student and records `pushed`/`skipped`.
- Not covered by a test here: a real Teacher (RS256 staff SSO) through `class_actor`; that path is the existing,
  separately tested Learning class access.

## Rollback

Revert the image. Data can stay; to remove it: `MATCH(p:StudentPush) DELETE p`,
`DROP CONSTRAINT student_push_key IF EXISTS`. Student devices are notification-services `PushDevice` rows with
`recipientId STARTS WITH 'student:'`.
