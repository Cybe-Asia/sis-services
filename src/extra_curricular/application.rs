//! Transactional activity use case. The persistence port owns current access checks and locks.
use super::{model::*, policy, ports::*};
pub async fn execute<S: ChangeStore>(
    store: &mut S,
    actor: &Actor,
    input: &Change,
) -> Result<Option<Receipt>, S::Error> {
    if !input.valid() {
        return Ok(None);
    }
    if !store.authorized().await? {
        return Ok(None);
    }
    let current = store.current().await?;
    if current.is_none() && !matches!(input.command, Command::Save { .. })
        || matches!(input.command, Command::Save { .. } | Command::Archive {})
            && !matches!(actor.role.as_str(), "admin" | "owner")
    {
        return Ok(None);
    }
    match store.replay().await? {
        Replay::Match(r) => return Ok(Some(r)),
        Replay::Conflict => return Ok(None),
        Replay::None => {}
    }
    let result = match &input.command {
        Command::Save { activity } => {
            let occupied = store.enrollment_count(true).await?;
            let history = store.enrollment_count(false).await?;
            if !policy::catalog(
                current.as_ref(),
                activity,
                input.revision,
                occupied,
                history,
            ) || !store.catalog_references(activity).await?
            {
                return Ok(None);
            }
            let revision = input.revision + 1;
            store.save_catalog(activity, revision).await?;
            Receipt::revision(revision)
        }
        Command::Archive {} => {
            if current
                .as_ref()
                .is_none_or(|c| c.revision != input.revision)
            {
                return Ok(None);
            }
            let revision = input.revision + 1;
            store.archive(revision).await?;
            Receipt::revision(revision)
        }
        Command::Enroll {} | Command::Approve {} | Command::Reject {} | Command::Withdraw {} => {
            let Some(c) = current.as_ref() else {
                return Ok(None);
            };
            let Some(student) = c.student_id.as_deref() else {
                return Ok(None);
            };
            let old = store.enrollment(student).await?;
            let occupied = store.enrollment_count(true).await?;
            let Some(status) = policy::enrollment(
                &actor.role,
                c,
                old.as_ref(),
                &input.command,
                input.revision,
                occupied,
            ) else {
                return Ok(None);
            };
            let e = Enrollment {
                student_id: student.into(),
                revision: old.map_or(1, |e| e.revision + 1),
                status,
            };
            store.save_enrollment(&e).await?;
            Receipt {
                revision: e.revision,
                student_id: Some(e.student_id),
                status: Some(e.status),
                released: None,
            }
        }
        Command::Attendance { meeting_id, marks } => {
            let Some(c) = current.as_ref() else {
                return Ok(None);
            };
            if !matches!(actor.role.as_str(), "teacher" | "owner")
                || !c.activity.meetings.iter().any(|m| &m.id == meeting_id)
                || store.meeting_revision(meeting_id).await? != input.revision
                || !store
                    .roster_authorized(
                        &marks
                            .iter()
                            .map(|m| m.student_id.clone())
                            .collect::<Vec<_>>(),
                    )
                    .await?
            {
                return Ok(None);
            }
            let revision = input.revision + 1;
            store.save_attendance(meeting_id, revision, marks).await?;
            Receipt::revision(revision)
        }
        Command::Outcome {
            skills,
            feedback,
            achievements,
        } => {
            let Some(c) = current.as_ref() else {
                return Ok(None);
            };
            let Some(student) = input.scope.student_id.as_deref() else {
                return Ok(None);
            };
            if !matches!(actor.role.as_str(), "teacher" | "owner")
                || !policy::skills(&c.activity, skills)
                || !store.roster_authorized(&[student.into()]).await?
            {
                return Ok(None);
            }
            let old = store.outcome(student).await?;
            if old.as_ref().map_or(0, |o| o.revision) != input.revision {
                return Ok(None);
            }
            let revision = input.revision + 1;
            store
                .save_outcome(&Outcome {
                    student_id: student.into(),
                    revision,
                    released: false,
                    skills: skills.clone(),
                    feedback: feedback.clone(),
                    achievements: achievements.clone(),
                })
                .await?;
            let mut receipt = Receipt::revision(revision);
            receipt.released = Some(false);
            receipt
        }
        Command::Release {} => {
            let Some(student) = input.scope.student_id.as_deref() else {
                return Ok(None);
            };
            if !matches!(actor.role.as_str(), "teacher" | "owner")
                || !store.roster_authorized(&[student.into()]).await?
            {
                return Ok(None);
            }
            let Some(mut outcome) = store.outcome(student).await? else {
                return Ok(None);
            };
            if outcome.revision != input.revision {
                return Ok(None);
            }
            outcome.revision += 1;
            outcome.released = true;
            store.save_outcome(&outcome).await?;
            let mut receipt = Receipt::revision(outcome.revision);
            receipt.released = Some(true);
            receipt
        }
    };
    store.audit(&result).await?;
    Ok(Some(result))
}

pub async fn context<S: ContextReader>(
    store: &S,
    actor: &Actor,
    student: Option<String>,
    schools: Vec<String>,
    tenants: Vec<String>,
) -> Result<Option<CatalogContext>, S::Error> {
    if !matches!(
        actor.role.as_str(),
        "admin" | "teacher" | "owner" | "parent" | "student"
    ) || (actor.role == "parent") != student.is_some()
    {
        return Ok(None);
    }
    let context = store
        .read(actor, student, schools.clone(), tenants.clone())
        .await?;
    if matches!(actor.role.as_str(), "parent" | "student") && context.classes.is_empty() {
        return Ok(None);
    }
    if matches!(actor.role.as_str(), "admin" | "teacher" | "owner")
        && (context
            .classes
            .iter()
            .any(|c| !schools.contains(&c.school_id) || !tenants.contains(&c.tenant_id))
            || context.staff.iter().any(|s| {
                s.school_ids.iter().any(|id| !schools.contains(id))
                    || s.tenant_ids.iter().any(|id| !tenants.contains(id))
            }))
    {
        return Ok(None);
    }
    Ok(Some(context))
}
