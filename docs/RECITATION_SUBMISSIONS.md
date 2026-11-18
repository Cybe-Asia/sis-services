# Recitation submissions (tahfiz)

Owner: sis-services, module `src/extra_curricular/recitation`. Decision record: digital-school
`docs/adr/0009-tahfiz-recitation-assessment.md`. Contract version 1, additive.

A Student enrolled in a published activity submits one recording per Qur'an verse after the AI
pre-check; a new take replaces the previous one. The coach (ustadz) of the activity, or the school
Owner, lists the roster's submissions, plays a clip and grades it against the activity's criteria.
Clips live in the shared object store (`MINIO_*`, prefix `sis/recitations/`), never in the graph.
Nothing here grades automatically: the verdict the Student sends is a hint for the coach.

## Routes

All under `/api/v1/sis-service/activities/recitations`, `Authorization: Bearer` as for activities,
`:role` is `student` or `teacher` (an Owner on the teacher path acts as coach of every activity).
Responses are `{"data":…}` or `{"error":{"code":…}}` with `INVALID_INPUT`, `UNAUTHORIZED`,
`FORBIDDEN`, `REVISION_CONFLICT`, `NOT_FOUND`, `TOO_LARGE`, `DEPENDENCY_UNAVAILABLE`.

- `GET /:role?schoolId&tenantId&academicYear&activityId[&studentId][&verse][&status][&after]`
  Student: own submissions. Coach: the roster's; `studentId` narrows, `status` is `submitted`
  (waiting) or `reviewed`. Items: `studentId`, `studentName` (coach only), `verse`, `revision`,
  `status`, `stars`, `attempts`, `submittedAt`, `audio` (false once the clip was removed), `verdict`,
  `review` (`revision` graded, `reviewerId`, `skills`, `note`, `decision`, `reviewedAt`) or null.
  200 per page; `nextCursor` goes back as `after`.
- `PUT /student` JSON `{scope, activityId, verse, mime, audioBase64, verdict}` (3 MB body; `mime`
  one of webm/ogg/mp4/wav/mpeg; the bytes must begin like that type). Replaces the verse's
  submission; the previous clip is deleted. Answer: `{verse, revision, attempts, status, submittedAt}`.
- `GET /:role/audio?…&studentId&verse` the clip as stored (`Content-Type` = its `mime`, at most 4.4 MB),
  `Cache-Control: no-store`. Student: own only.
- `POST /teacher/review` JSON `{scope, activityId, studentId, verse, revision, skills:[{criterionId,
  level}], note?, decision: "pass"|"retry"}`. Every criterion of the activity needs a level; `revision`
  is the submission revision the coach listened to (409 when the student sent a newer take).

## Playable everywhere (AAC)

The iPhone app (`AVAudioPlayer`) cannot play WebM or Ogg, which Chrome, Android and Firefox record. After
the caller is authorised, a WebM or Ogg clip (Opus or Vorbis) is converted to AAC-LC in MP4, mono, 64 kbps,
and stored as `audio/mp4` (`.m4a`). MP4, MP3 and WAV are stored as sent. The size limit and the
"begins like its type" check still apply to what was sent. If the conversion fails (ffmpeg missing, a
broken clip, over 30 s of work, output over 4.4 MB), the clip is stored as sent. The log line is
`recitation clip kept as sent` with the reason and type only. Code: `src/extra_curricular/recitation/transcode.rs`.

The image carries a minimal ffmpeg built from the release tarball (Dockerfile stage `ffmpeg`: version and
sha256 pinned, matroska/ogg demuxers, opus/vorbis decoders, aac encoder, mp4 muxer, file protocol only). It
adds about 2 MB. Local tests need `ffmpeg` on PATH for the ignored conversion test (`brew install ffmpeg`).

The student recorder (client `web/src/presentation/tahfiz/use-recorder.ts`) asks for
`audio/mp4;codecs=mp4a.40.2` first, so Safari, and Chrome where it can record AAC, send AAC and need no
conversion.

### Backfill of stored WebM/Ogg clips

`sis-service transcode-recitations` counts the clips still stored as WebM or Ogg and prints
`{"found":N,"converted":0,"kept":0}`. It writes nothing. With `--apply`, it stores each clip's AAC
rendition under the same key with `.m4a`, then moves the record (`object_key`, `mime`, `size`) only if it
still points at the old object, then deletes the old object. A clip ffmpeg refuses is left as it is
(`kept`). Re-running is safe: converted clips are no longer WebM or Ogg. Run it in the sis-service pod
(it needs Neo4j and `MINIO_*`):
`kubectl -n <ns> exec deploy/sis-service -- ./sis-service transcode-recitations` (count), then `… --apply`.
Read-only count straight in Neo4j: `MATCH(r:ExtraCurricularRecitation) WHERE r.object_key IS NOT NULL AND
r.mime IN ['audio/webm','audio/ogg'] RETURN r.mime, count(*)`.

## Retention

A clip nobody reviewed is removed 180 days after it was sent; a reviewed one 90 days after the
review; a replaced one at once. The record (verdict, grade) stays as history with `audio:false`.
The sweep runs in-process every six hours when `MINIO_*` is set. A family's deletion request is
an ad hoc delete of the student's prefix plus the records (`MATCH(r:ExtraCurricularRecitation
{student_id:…})`).

## Graph

`ExtraCurricularRecitation {key: activity_key|student|verse, activity_key, student_id, verse,
revision, attempts, status, stars, object_key, mime, size, verdict, submitted_at, review,
reviewed_at, reviewer_id, delete_after, purged_at}`; audit rows in `ExtraCurricularAudit` with a
summary request (never the clip).
