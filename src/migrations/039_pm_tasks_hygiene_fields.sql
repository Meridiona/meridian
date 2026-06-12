-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Definition-of-Ready fields the board-hygiene triage checks for and helps the
-- dev fix in-app. These are standard "good ticket" fields (priority, estimate,
-- acceptance criteria) that we previously didn't fetch. Stored verbatim; a NULL/
-- empty value means the field is absent on the ticket. The triage only flags a
-- missing field when the board actually USES it (board-level guard), so a team
-- that doesn't track e.g. story points is never nagged about them.

ALTER TABLE pm_tasks ADD COLUMN priority            TEXT NOT NULL DEFAULT '';
ALTER TABLE pm_tasks ADD COLUMN story_points        TEXT NOT NULL DEFAULT '';
ALTER TABLE pm_tasks ADD COLUMN acceptance_criteria TEXT NOT NULL DEFAULT '';
