-- The Home Exit of a Person (ADR-0029): the one Host of the Workspace
-- through which the Person's Computers on a Server reach the internet.
-- NULL while the Person has chosen none. The store writes a Host of the
-- same Workspace alone.
--
-- The Postgres migration of the same number holds the same column.

ALTER TABLE workspaces ADD COLUMN home_exit_host_id TEXT REFERENCES hosts (id);
