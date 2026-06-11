-- meridian — normalises screenpipe activity into structured app sessions

-- Remove all subtasks from pm_tasks, keeping only top-level tasks and features.
-- Subtasks will be filtered out at fetch time with type != Subtask in the JQL.
DELETE FROM pm_task_embeddings
WHERE task_key IN (
    SELECT task_key FROM pm_tasks WHERE issue_type = 'Subtask'
);

DELETE FROM pm_tasks WHERE issue_type = 'Subtask';
