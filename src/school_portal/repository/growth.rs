use super::{Error, PARENT, PARENT_SECTION};
use neo4rs::{query, Graph};
use serde_json::{json, Value};
/** Latest publication for exactly the owned child. History is not bundled with Home. */
pub async fn parent_growth(
    graph: &Graph,
    sub: &str,
    student: &str,
) -> Result<Option<Value>, Error> {
    let mut rows = graph.execute(query(&format!("{PARENT} WITH DISTINCT u,s WHERE s.studentId=$student {PARENT_SECTION} WITH DISTINCT s,sec OPTIONAL MATCH (r:SchoolPublishedRecord {{kind:'growth',student_id:s.studentId}})-[:FOR_SECTION]->(sec) RETURN s.studentId AS student,r.payload AS payload,toString(r.created_at) AS created ORDER BY CASE WHEN r IS NULL THEN 1 ELSE 0 END ASC,created DESC LIMIT 1")).param("sub",sub).param("student",student)).await?;
    let Some(row) = rows.next().await? else {
        // An owned admissions child may not be enrolled yet. Return an empty
        // publication while keeping every school record restricted to active enrollment.
        let mut owned=graph.execute(query(&format!("{PARENT} WITH DISTINCT s WHERE s.studentId=$student RETURN count(s)=1 AS owned")).param("sub",sub).param("student",student)).await?;
        let allowed = owned
            .next()
            .await?
            .is_some_and(|r| r.get::<bool>("owned").unwrap_or(false));
        return Ok(allowed.then_some(Value::Null));
    };
    let content = row.get::<Option<String>>("payload")?;
    Ok(Some(match content {
        Some(raw) => {
            json!({"studentId":row.get::<String>("student")?,"content":serde_json::from_str::<Value>(&raw)?,"createdAt":row.get::<String>("created")?})
        }
        None => Value::Null,
    }))
}
#[cfg(test)]
mod tests;
