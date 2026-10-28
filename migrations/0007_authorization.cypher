CREATE CONSTRAINT authorization_template_key IF NOT EXISTS FOR (n:AuthorizationRoleTemplate) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT authorization_template_version_key IF NOT EXISTS FOR (n:AuthorizationRoleTemplateVersion) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT authorization_assignment_key IF NOT EXISTS FOR (n:AuthorizationAssignment) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT authorization_assignment_id IF NOT EXISTS FOR (n:AuthorizationAssignment) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT authorization_audit_key IF NOT EXISTS FOR (n:AuthorizationAudit) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT authorization_receipt_key IF NOT EXISTS FOR (n:AuthorizationReceipt) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT authorization_tenant_state IF NOT EXISTS FOR (n:AuthorizationTenantState) REQUIRE n.tenant_id IS UNIQUE;
CREATE INDEX authorization_template_tenant IF NOT EXISTS FOR (n:AuthorizationRoleTemplate) ON (n.tenant_id);
CREATE INDEX authorization_template_version_scope IF NOT EXISTS FOR (n:AuthorizationRoleTemplateVersion) ON (n.tenant_id, n.code);
CREATE INDEX authorization_assignment_holder IF NOT EXISTS FOR (n:AuthorizationAssignment) ON (n.tenant_id, n.staff_member_id);
CREATE INDEX authorization_assignment_section IF NOT EXISTS FOR (n:AuthorizationAssignment) ON (n.tenant_id, n.section_id);
CREATE INDEX authorization_assignment_status IF NOT EXISTS FOR (n:AuthorizationAssignment) ON (n.tenant_id, n.status);
CREATE INDEX authorization_audit_tenant IF NOT EXISTS FOR (n:AuthorizationAudit) ON (n.tenant_id, n.created_at)
