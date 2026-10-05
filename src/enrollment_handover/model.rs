use serde::{Deserialize, Serialize};

pub fn identifier(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn label(v: &str) -> bool {
    !v.trim().is_empty() && v.len() <= 32 && !v.chars().any(char::is_control)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub idempotency_key: String,
    pub student_id: String,
    pub payer_user_id: String,
    pub admission_id: String,
    pub application_id: String,
    pub offer_id: String,
    pub payment_id: String,
    pub offer_revision: u32,
    pub pricing_snapshot_hash: String,
    pub school_id: String,
    pub tenant_id: String,
    pub section_id: String,
    pub year_group: String,
    pub academic_year: String,
    pub expected_version: u32,
}
impl Command {
    pub fn valid(&self) -> bool {
        [
            &self.idempotency_key,
            &self.student_id,
            &self.payer_user_id,
            &self.admission_id,
            &self.application_id,
            &self.offer_id,
            &self.payment_id,
            &self.school_id,
            &self.tenant_id,
            &self.section_id,
        ]
        .iter()
        .all(|v| identifier(v))
            && self.offer_revision > 0
            && self.expected_version == 0
            && self.pricing_snapshot_hash.len() == 64
            && self
                .pricing_snapshot_hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            && label(&self.year_group)
            && label(&self.academic_year)
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub contract_version: u32,
    pub version: u32,
    pub student_id: String,
    pub enrolled_student_id: String,
    pub student_number: String,
    pub school_id: String,
    pub tenant_id: String,
    pub section_id: String,
    pub year_group: String,
    pub academic_year: String,
    pub enrolment_date: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_rejects_guessed_permanent_id_unknown_fields_and_missing_scope() {
        let body = serde_json::json!({"idempotencyKey":"request-1","studentId":"applicant","payerUserId":"parent","admissionId":"lead","applicationId":"app","offerId":"offer","paymentId":"payment","offerRevision":1,"pricingSnapshotHash":"a".repeat(64),"schoolId":"school","tenantId":"tenant","sectionId":"section","yearGroup":"Grade 7","academicYear":"2026/2027","expectedVersion":0});
        assert!(serde_json::from_value::<Command>(body.clone())
            .unwrap()
            .valid());
        for (key, value) in [
            ("enrolledStudentId", serde_json::json!("legacy")),
            ("tenantId", serde_json::json!("")),
            ("expectedVersion", serde_json::json!(1)),
            ("academicYear", serde_json::json!("")),
        ] {
            let mut bad = body.clone();
            bad[key] = value;
            assert!(!serde_json::from_value::<Command>(bad).is_ok_and(|v| v.valid()));
        }
    }
}
