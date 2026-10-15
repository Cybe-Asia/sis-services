//! Recitation submissions of a tahfiz activity: what a Student sends after the AI check, what the
//! coach (ustadz) sends back, and the bounds on both. The clip itself is base64 in the submission;
//! it is kept in the object store this deployment already runs, never in the graph.
use crate::extra_curricular::model::{Scope, Skill};
use crate::school_portal::model::identifier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MIMES: [&str; 5] = [
    "audio/webm",
    "audio/ogg",
    "audio/mp4",
    "audio/wav",
    "audio/mpeg",
];
/// Base64 of at most about 2 MB of audio: the same bound as the AI check, so one take fits both.
pub const MAX_AUDIO_BASE64: usize = 2_800_000;
pub const MAX_AUDIO_BYTES: usize = 2_200_000;
pub const BODY_LIMIT: usize = 3_000_000;
/// A clip nobody reviewed is kept this long after it was sent; a reviewed one this long after the review.
pub const UNREVIEWED_RETENTION_DAYS: i64 = 180;
pub const REVIEWED_RETENTION_DAYS: i64 = 90;
const STATUSES: [&str; 4] = ["ok", "unsure", "wrong", "missing"];

/// `surah:ayah`, both numbers of at most three digits.
pub fn verse(s: &str) -> bool {
    let Some((surah, ayah)) = s.split_once(':') else {
        return false;
    };
    let number = |v: &str| (1..=3).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()) && v != "0";
    number(surah) && number(ayah)
}
pub fn extension(mime: &str) -> &'static str {
    match mime {
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mp4" => "m4a",
        "audio/wav" => "wav",
        _ => "mp3",
    }
}
/// Whether the bytes begin the way a clip of this type does; a mislabelled body is refused early.
pub fn looks_like(mime: &str, bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    match mime {
        "audio/webm" => bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]),
        "audio/ogg" => bytes.starts_with(b"OggS"),
        "audio/mp4" => &bytes[4..8] == b"ftyp",
        "audio/wav" => bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE",
        "audio/mpeg" => bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0),
        _ => false,
    }
}
fn plain(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}
fn unique<'a>(ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    ids.into_iter().all(|id| identifier(id) && seen.insert(id))
}

/// One word of the AI check, as the Student screen showed it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerdictWord {
    pub index: u32,
    pub expected: String,
    pub heard: Option<String>,
    pub status: String,
}
/// The AI check of the take being submitted: a hint for the ustadz, never a grade.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verdict {
    pub stars: u8,
    pub words: Vec<VerdictWord>,
    pub heard_text: String,
    pub duration_seconds: f64,
}
impl Verdict {
    pub fn valid(&self) -> bool {
        (1..=3).contains(&self.stars)
            && !self.words.is_empty()
            && self.words.len() <= 200
            && self.words.iter().enumerate().all(|(at, w)| {
                w.index as usize == at
                    && !w.expected.is_empty()
                    && plain(&w.expected, 64)
                    && w.heard.as_deref().is_none_or(|h| plain(h, 64))
                    && STATUSES.contains(&w.status.as_str())
            })
            && plain(&self.heard_text, 600)
            && self.duration_seconds.is_finite()
            && (0.0..=600.0).contains(&self.duration_seconds)
    }
}
/// What the Student screen sends after a check in tadarus mode. It replaces the verse's previous
/// submission; the Student's identity comes from the session, never from the body.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submission {
    pub scope: Scope,
    pub activity_id: String,
    pub verse: String,
    pub mime: String,
    pub audio_base64: String,
    pub verdict: Verdict,
}
impl Submission {
    pub fn valid(&self) -> bool {
        self.scope.valid()
            && self.scope.student_id.is_none()
            && self.scope.after.is_none()
            && identifier(&self.activity_id)
            && verse(&self.verse)
            && MIMES.contains(&self.mime.as_str())
            && self.audio_base64.len() >= 16
            && self.audio_base64.len() <= MAX_AUDIO_BASE64
            && self
                .audio_base64
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            && self.verdict.valid()
    }
}
/// The ustadz's grade of one submission: a level per criterion of the activity, a note, and
/// whether the student passes the verse or records it again.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub scope: Scope,
    pub activity_id: String,
    pub student_id: String,
    pub verse: String,
    pub revision: u32,
    pub skills: Vec<Skill>,
    pub note: Option<String>,
    pub decision: String,
}
impl Review {
    pub fn valid(&self) -> bool {
        self.scope.valid()
            && self.scope.student_id.is_none()
            && self.scope.after.is_none()
            && identifier(&self.activity_id)
            && identifier(&self.student_id)
            && verse(&self.verse)
            && self.revision < i32::MAX as u32
            && !self.skills.is_empty()
            && self.skills.len() <= 30
            && unique(self.skills.iter().map(|s| s.criterion_id.as_str()))
            && self.note.as_deref().is_none_or(|n| plain(n, 1000))
            && matches!(self.decision.as_str(), "pass" | "retry")
    }
}
/// The query of a list or a clip. `after` is the cursor the previous page returned.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub school_id: String,
    pub tenant_id: String,
    pub academic_year: String,
    pub activity_id: String,
    pub student_id: Option<String>,
    pub verse: Option<String>,
    pub status: Option<String>,
    pub after: Option<String>,
}
impl Query {
    pub fn scope(&self) -> Scope {
        Scope {
            school_id: self.school_id.clone(),
            tenant_id: self.tenant_id.clone(),
            academic_year: self.academic_year.clone(),
            student_id: None,
            after: None,
        }
    }
    pub fn valid(&self) -> bool {
        self.scope().valid()
            && identifier(&self.activity_id)
            && self.student_id.as_deref().is_none_or(identifier)
            && self.verse.as_deref().is_none_or(verse)
            && self
                .status
                .as_deref()
                .is_none_or(|s| matches!(s, "submitted" | "reviewed"))
            && self.after.as_deref().is_none_or(|a| {
                (1..=512).contains(&a.len())
                    && a.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
    }
}
/// The grade as stored and listed. `revision` is the submission revision that was graded: a newer
/// take makes the grade history, not a verdict on what the student last sent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grade {
    pub revision: u32,
    pub reviewer_id: String,
    pub skills: Vec<Skill>,
    pub note: Option<String>,
    pub decision: String,
    pub reviewed_at: String,
}
