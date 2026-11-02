-- Host path of the brain directory mounted at /brain for this worker, if any.
-- Lets hostexec map a /brain working directory back to the host for git.
ALTER TABLE worker ADD COLUMN brain_path TEXT;
