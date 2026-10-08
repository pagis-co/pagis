-- A Connection records no auth mode (ADR-0012). Every OAuth Connection
-- holds a credential that the Installation OAuth Client brokered, and the
-- kind of the provider's catalog entry says what a Connection holds.
--
-- The Postgres migration of the same number holds the same change.

ALTER TABLE connections DROP COLUMN auth_mode;
