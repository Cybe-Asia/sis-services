CREATE CONSTRAINT academic_nomenclature_key IF NOT EXISTS FOR (n:AcademicNomenclature) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT academic_nomenclature_kind_key IF NOT EXISTS FOR (n:AcademicNomenclatureKind) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT academic_nomenclature_audit_key IF NOT EXISTS FOR (n:AcademicNomenclatureAudit) REQUIRE n.key IS UNIQUE;
CREATE CONSTRAINT academic_nomenclature_receipt_key IF NOT EXISTS FOR (n:AcademicNomenclatureReceipt) REQUIRE n.key IS UNIQUE;
CREATE INDEX academic_nomenclature_scope IF NOT EXISTS FOR (n:AcademicNomenclature) ON (n.tenant_id, n.school_id);
CREATE INDEX academic_nomenclature_kind_scope IF NOT EXISTS FOR (n:AcademicNomenclatureKind) ON (n.tenant_id, n.school_id)
