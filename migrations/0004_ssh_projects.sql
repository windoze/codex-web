-- Add SSH project support.
-- Projects can now be either "local" (existing behavior) or "ssh" (remote).

ALTER TABLE projects
ADD COLUMN kind TEXT NOT NULL DEFAULT 'local';

ALTER TABLE projects
ADD COLUMN ssh_target TEXT NULL;

ALTER TABLE projects
ADD COLUMN ssh_port INTEGER NULL;

ALTER TABLE projects
ADD COLUMN remote_root_path TEXT NULL;

ALTER TABLE projects
ADD COLUMN ssh_identity_file TEXT NULL;

ALTER TABLE projects
ADD COLUMN ssh_known_hosts_policy TEXT NULL;
