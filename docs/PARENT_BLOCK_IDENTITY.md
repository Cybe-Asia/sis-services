# Parent Block Identity DID

The Parent app sends the parent's Block Identity DID once the wallet on the phone is verified, so the admin
portal's Gate access tab offers it instead of a typed DID. Code: `src/school_portal/block_identity.rs`,
`sis_repository::list_block_identity_parents`.

## Proof of control

The DID is accepted only with a presentation signed by it: an `Ed25519Signature2020` VP with no credentials,
`holder` = the DID, `proof.verificationMethod` = `<did>#…`, and `proof.challenge` =
`digital-school:parent-did:<sub>` (`sub` = the Parent access token subject, the User id). The challenge binds
the signature to this account, so a copied presentation does not verify for another account; re-sending it
for the same account is harmless (same DID, same owner).

SIS checks the shape only and stores the presentation as sent. It has no JSON-LD canonicalisation, so
**every reader verifies the presentation (signature, holder, challenge for the returned `parentUserId`)
before using the DID**. The admin portal does, with the canonicalisation it already uses to sign its Block
Identity presentations. An unverifiable link is simply not offered.

## Parent: `POST /api/leads/v1/me/school/block-identity`

`Authorization: Bearer <Parent access token>` (User id subject; Lead link tokens and scoped tokens are refused).

```json
{ "did": "did:key:z6Mk…", "verifiablePresentation": "<VP JSON as a string, ≤ 16 KB>" }
```

- 200 `{"responseCode":200,"data":{"saved":true}}`: replaces the account's DID (one per account).
- 400 `INVALID_INPUT`: unknown fields, not a `did:key:z6Mk…`, presentation shape or challenge wrong, or it carries credentials.
- 401 `UNAUTHORIZED`; 403 `FORBIDDEN`: not an eligible Parent with at least one child; 503 `DEPENDENCY_UNAVAILABLE`.

Graph: `(:User)-[:HAS_BLOCK_IDENTITY]->(:ParentBlockIdentity {did, presentation, linked_at})`, plus a
`SchoolPortalAudit {kind:'block_identity_link'}`. The path sits under `/me/school*`, which ingress already
routes to SIS. Nothing is logged.

## Admin: `GET /api/leads/v1/admin/sis/sections/{sectionId}/students/{studentId}/block-identity-parents`

Typed staff credential. Owners and school admins of the class's school only (by role; no capability grants
it), and only while the student is an active member of the class. Eligible Parents of that student who
linked a DID, at most 10:

```json
{ "data": { "items": [ { "parentUserId": "…", "parentName": "…", "did": "did:key:z6Mk…", "verifiablePresentation": "…" } ] } }
```

400 invalid ids, 401, 403 (teacher, other school), 404 unknown class, 503. Other Parents never see a DID.

## Not done

- A Parent who deletes the wallet keeps the stored DID until the app links a new one (the DID is useless
  without the key; the admin still decides whether to link it).
