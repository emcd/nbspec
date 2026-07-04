# OpenSpec Instructions

Workflow Guide: @openspec/AGENTS.md

Always open `openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan).
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work.
- Sounds ambiguous and you need the authoritative spec before coding.

Use `openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

When a commit completes an OpenSpec task or requirement, update the relevant OpenSpec task status in the same commit.

Treat OpenSpec proposals like code: commit proposal files to a branch, reviewers review the commit (`git show`), author amends as needed, merge when settled. No notebook draft step.
