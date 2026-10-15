---
name: git-sync
description: Commit and push NovaMix changes to GitHub (origin = github.com/VictorCavalcant/NovaMix) automatically, checking for merge conflicts, leftover conflict markers, half-finished merges/rebases, oversized files and possible secrets before committing, and syncing with the remote before pushing. Use when the user asks to commit, push, sync, "subir", "enviar pro git/GitHub" or save the work to the repository.
---

# Commit and push (with conflict checks)

Goal: get the local work committed and pushed to `origin` on the current branch, without
ever losing remote work. Run it end to end without asking, **unless a step below says to stop**.

Never: `git push --force` / `--force-with-lease`, `git reset --hard`, `git checkout -- .`,
`git clean`, `--no-verify`, or resolving a conflict by discarding one side. If you get to a
point that seems to need one of these, stop and ask the user.

## 0. Rebuild rule

If the changes touch the program (`app/`, `crates/`, `Cargo.toml`/`Cargo.lock`, `build.rs`,
docs or packaging) and the `rebuild-exe` skill has not been run on the final code in this
session, run it first (CLAUDE.md rule). `dist/` and `target/` are git-ignored, so the build
output itself is never committed.

## 1. Preflight

```bash
bash .claude/skills/git-sync/preflight.sh
```

It fetches `origin` and prints one line per finding, ending with `RESULT: OK` (exit 0) or
`RESULT: BLOCKED` (exit 1); exit 2 is a setup error (no repo, no remote, detached HEAD,
fetch failed — e.g. not logged in to GitHub).

| Line | Meaning / what to do |
| --- | --- |
| `BLOCKED: operation in progress` / `rebase in progress` | A merge/rebase was left half-done. **Stop**, show `git status`, ask the user whether to finish or abort it. |
| `BLOCKED: unmerged files` / `conflict markers in <file>` | Unresolved conflict. **Stop** and show the files/lines. Resolve only if the user asks, keeping both sides' intent. |
| `BLOCKED: committed changes conflict with origin/...` | Local commits and remote commits edit the same lines. **Stop**, list the files, show both versions (`git diff HEAD...origin/<branch> -- <file>` and `git diff origin/<branch>...HEAD -- <file>`) and ask how to resolve. |
| `BLOCKED: untracked files would be overwritten` | Remote added files you also have untracked locally. **Stop** and ask. |
| `BLOCKED: file too large for GitHub` | Add it to `.gitignore` (build output) or ask the user (Git LFS). |
| `WARNING: possible secret` / `large file` | Open/inspect it. Certificates, keys, `.env`, tokens must not be committed: add to `.gitignore` and tell the user. Proceed only if clearly harmless. |
| `WARNING: uncommitted edits also changed upstream` | Possible conflict; re-run preflight after step 2 (it then checks exactly). |
| `SYNC: ...` / `REMOTE: ... does not exist yet` | Informational; drives step 3. |

## 2. Commit

1. `git status --short` and `git diff --stat` (plus `git diff` of anything unclear) to see
   exactly what changed. Do not commit files that are clearly not meant for the repo (build
   output, logs, editor junk) — add them to `.gitignore` instead.
2. `git add -A` (or specific paths if the user asked to commit only part of the work).
3. If nothing is staged and nothing to push (`ahead 0`), tell the user there is nothing to do.
4. Commit message in English, imperative, ≤ 72-char subject, body explaining *why* when not
   obvious, ending with the attribution trailer from the system reminder, e.g.:

```bash
git commit -F - <<'EOF'
Add input gain metering to strip panel

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

If a commit hook fails, fix the cause and create the commit again (never `--no-verify`).

## 3. Sync with the remote

Re-run the preflight now that the work is committed (it predicts conflicts exactly with
`git merge-tree`, without touching the working tree):

```bash
bash .claude/skills/git-sync/preflight.sh
```

- `BLOCKED` → stop as in step 1. The local commit stays; nothing was pushed.
- `SYNC: up to date` or `REMOTE: ... does not exist yet` → go to step 4.
- `SYNC: behind only` or `SYNC: diverged, no conflicts predicted` →

```bash
git pull --rebase --autostash origin <branch>
```

If the rebase still stops on a conflict: `git rebase --abort` (restores the pre-pull state),
then report the conflicting files to the user and ask how to resolve.

## 4. Push

```bash
git push -u origin <branch>
```

- Rejected as non-fast-forward (someone pushed meanwhile) → go back to step 3 once, then retry.
- Authentication error → tell the user to sign in (`gh auth login` or Git Credential Manager);
  don't try to work around it.

## 5. Verify and report

```bash
git status -sb
git log --oneline -3
```

`git status -sb` must show `## <branch>...origin/<branch>` with no `ahead`/`behind`.
Report to the user in pt-BR: commit hash + subject, branch, link
`https://github.com/VictorCavalcant/NovaMix/commit/<hash>`, and any warnings found.
