use super::repository::Error;
use crate::school_portal::model::{date, identifier};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Window {
    pub from: String,
    pub to: String,
    pub after: Option<String>,
    pub student_id: Option<String>,
}
impl Window {
    pub fn valid(&self) -> bool {
        date(&self.from)
            && date(&self.to)
            && self.from <= self.to
            && chrono::NaiveDate::parse_from_str(&self.to, "%Y-%m-%d")
                .ok()
                .zip(chrono::NaiveDate::parse_from_str(&self.from, "%Y-%m-%d").ok())
                .is_some_and(|(e, s)| (e - s).num_days() <= 62)
            && self.student_id.as_deref().is_none_or(identifier)
            && self.cursor().is_ok()
    }
    pub fn cursor(&self) -> Result<String, Error> {
        match &self.after {
            None => Ok(String::new()),
            Some(v) if v.len() <= 768 => {
                let bytes = URL_SAFE_NO_PAD.decode(v)?;
                let value = String::from_utf8(bytes)?;
                let parts = value.split('|').collect::<Vec<_>>();
                if parts.len() != 3 || !parts.iter().all(|s| identifier(s)) {
                    return Err("invalid calendar cursor".into());
                }
                Ok(value)
            }
            _ => Err("invalid calendar cursor".into()),
        }
    }
    pub fn clause(&self) -> &'static str {
        "AND r.start_date<=$to AND r.end_date>=$from AND r.key>$after RETURN DISTINCT r.payload AS payload,r.version AS version,r.key AS key ORDER BY key LIMIT 101"
    }
}
pub async fn page(graph: &neo4rs::Graph, q: neo4rs::Query) -> Result<serde_json::Value, Error> {
    let mut rows = graph.execute(q).await?;
    let mut items = vec![];
    let mut bytes = 0;
    let mut next = None;
    let mut last = String::new();
    while let Some(r) = rows.next().await? {
        let entry: super::model::Entry = serde_json::from_str(&r.get::<String>("payload")?)?;
        if !entry.valid() {
            return Err("invalid calendar record".into());
        }
        let item =
            serde_json::json!({"entry":entry,"version":r.get::<i64>("version")?,"studentId":null});
        let size = serde_json::to_vec(&item)?.len() + 1;
        if items.len() == 100 || bytes + size > 350_000 {
            next = Some(URL_SAFE_NO_PAD.encode(&last));
            break;
        }
        last = r.get::<String>("key")?;
        bytes += size;
        items.push(item);
    }
    Ok(serde_json::json!({"items":items,"hasMore":next.is_some(),"next":next}))
}
