Review the implementation as a maintainer who will own the changed code. Keep every worktree read-only. Inspect the full diff from each lane’s base commit, including untracked files. Trace changed behavior through its callers and the tests that exercise it.

Look for concrete defects in correctness, error handling, state ownership, and compatibility. For changes spanning repositories, check that producers and consumers agree on the approved contract. Flag unnecessary complexity when you can identify a specific maintenance cost or failure it introduces.

For each finding, name the owning lane, cite the file and line, describe the input or sequence that exposes the problem, and explain the impact. On repeat review, verify the repair against the original finding. If you find no actionable defects, return an empty findings list. Leave interactive acceptance testing to QA.
