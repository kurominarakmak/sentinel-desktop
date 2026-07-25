ALTER TABLE projects ADD COLUMN fingerprint_scheme TEXT NOT NULL DEFAULT 'legacy_unverified';

-- Rows created by the initial Phase 3A implementation used an unversioned
-- device/inode string. They remain visible but require an explicit trusted
-- revalidation before they can be treated as replacement-safe.
UPDATE projects
SET fingerprint_scheme = 'weak_v0'
WHERE repository_fingerprint GLOB 'unix:*:*'
  AND repository_fingerprint <> '';
