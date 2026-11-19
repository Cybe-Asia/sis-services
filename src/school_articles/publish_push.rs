//! The push families get when the school publishes an article or webinar: once per article (a
//! re-publish after an unpublish pushes nothing), to the Parents of the audience the article names,
//! with a fixed lock-screen text and a link that opens the article.
use super::{model::Article, repository};
use crate::school_broadcasts::{
    self,
    repository::{recipients, Recipient},
    Audience,
};
use crate::AppState;
use serde_json::{json, Value};

/// The broadcast audience an article's own audience reaches.
pub(crate) fn audience(article: &Article) -> Audience {
    match article.audience.as_str() {
        "sections" => Audience::Class {
            ids: article.section_ids.clone(),
        },
        _ => Audience::School {},
    }
}
/// Lock screens are not private: a fixed text in the reader's language, never the title. The link is
/// the web's article link (`/parent?article=<id>`), which the app opens as the article page too.
pub(crate) fn push_payload(r: &Recipient, article: &Article) -> Value {
    let id = r.locale.starts_with("id");
    let (title, body) = match (article.kind.as_str(), id) {
        ("webinar", true) => ("Webinar sekolah", "Ada webinar baru dari sekolah."),
        ("webinar", false) => ("School webinar", "There is a new webinar from the school."),
        (_, true) => ("Artikel sekolah", "Ada artikel baru dari sekolah."),
        (_, false) => ("School article", "There is a new article from the school."),
    };
    // `Article::valid` keeps the id to `[A-Za-z0-9_.:-]`, so it needs no escaping in a query.
    json!({"recipientId": r.id, "title": title, "body": body, "url": format!("/parent?article={}", article.id)})
}
/// Runs after a successful publish, off the request. Failures are logged and recorded, never retried.
// shortcut: like school broadcasts, the fan-out lives in this process; a restart mid-way leaves the
// rest unpushed and the record `sending`. Move both to a durable outbox if that matters.
pub(crate) async fn announce(s: AppState, article: Article) {
    let key = format!("{}|{}|{}", article.tenant_id, article.school_id, article.id);
    let everyone = match recipients(
        &s.graph,
        &article.school_id,
        &article.tenant_id,
        &audience(&article),
    )
    .await
    {
        Ok(found) => found,
        Err(_) => {
            tracing::warn!("article {key}: recipients not read, nothing pushed");
            return;
        }
    };
    match repository::claim_push(&s.graph, &key, everyone.len() as i64).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(_) => {
            tracing::warn!("article {key}: push not claimed, nothing pushed");
            return;
        }
    }
    let wanted: Vec<Recipient> = everyone.into_iter().filter(|r| r.notify).collect();
    let (pushed, failed) =
        match school_broadcasts::deliver(&s, &wanted, |r| push_payload(r, &article)).await {
            Some(outcome) => outcome,
            None => {
                tracing::warn!(
                    "article {key}: notification-services is not configured, nothing pushed"
                );
                (0, wanted.len() as i64)
            }
        };
    if repository::finish_push(&s.graph, &key, pushed, failed)
        .await
        .is_err()
    {
        tracing::warn!("article {key}: push outcome not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn article(kind: &str, audience: &str, sections: &[&str]) -> Article {
        serde_json::from_value(json!({"id":"f33125bb-ccd1","schoolId":"school","tenantId":"tenant","kind":kind,"category":"wellbeing","title":"Health: private title","body":"B","audience":audience,"sectionIds":sections,"date":null,"start":null,"end":null,"joinUrl":null})).unwrap()
    }
    fn reader(locale: &str) -> Recipient {
        Recipient {
            id: "parent:u1".into(),
            user: Some("u1".into()),
            locale: locale.into(),
            notify: true,
        }
    }
    #[test]
    fn the_lock_screen_names_the_kind_never_the_title_and_links_the_article() {
        let a = article("article", "school", &[]);
        let id = push_payload(&reader("id"), &a);
        assert_eq!(id["title"], "Artikel sekolah");
        assert_eq!(id["url"], "/parent?article=f33125bb-ccd1");
        assert_eq!(id["recipientId"], "parent:u1");
        let w = article("webinar", "school", &[]);
        assert_eq!(push_payload(&reader("en"), &w)["title"], "School webinar");
        assert_eq!(
            push_payload(&reader("id"), &w)["body"],
            "Ada webinar baru dari sekolah."
        );
        for v in [
            push_payload(&reader("id"), &a),
            push_payload(&reader("en"), &w),
        ] {
            assert!(!v.to_string().contains("private title"), "{v}");
        }
    }
    #[test]
    fn the_article_audience_becomes_the_broadcast_audience() {
        assert_eq!(
            audience(&article("article", "school", &[])),
            Audience::School {}
        );
        assert_eq!(
            audience(&article("article", "sections", &["7a", "7b"])),
            Audience::Class {
                ids: vec!["7a".into(), "7b".into()]
            }
        );
    }
}
