#!/usr/bin/env bash
# Pre-commit/pre-push safety check for git-sync.
# Usage: bash .claude/skills/git-sync/preflight.sh [remote]
# Exit codes: 0 = safe to commit/push, 1 = blocked (conflicts, risky files), 2 = setup error.
set -u

remote=${1:-origin}
root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "ERROR: not a git repository"; exit 2; }
cd "$root" || exit 2
git remote get-url "$remote" >/dev/null 2>&1 || { echo "ERROR: remote '$remote' not configured"; exit 2; }

gitdir=$(git rev-parse --git-dir)
blocked=0
block() { echo "BLOCKED: $*"; blocked=1; }

branch=$(git symbolic-ref --short -q HEAD) || { echo "ERROR: detached HEAD"; exit 2; }
echo "BRANCH: $branch"

# 1. A merge/rebase/cherry-pick left half-done.
for f in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD; do
  [ -e "$gitdir/$f" ] && block "operation in progress ($f)"
done
{ [ -d "$gitdir/rebase-merge" ] || [ -d "$gitdir/rebase-apply" ]; } && block "rebase in progress"

# 2. Unmerged paths in the index.
unmerged=$(git diff --name-only --diff-filter=U)
[ -n "$unmerged" ] && { block "unmerged files:"; echo "$unmerged" | sed 's/^/  /'; }

# 3. Leftover conflict markers in files that would be committed.
changed=$( { git diff --name-only HEAD 2>/dev/null || git ls-files; git ls-files -o --exclude-standard; } | sort -u)
while IFS= read -r f; do
  [ -f "$f" ] || continue
  hits=$(grep -InE '^(<{7}|>{7})( |$)' -- "$f" 2>/dev/null | head -3)
  [ -n "$hits" ] && { block "conflict markers in $f"; echo "$hits" | sed 's/^/  /'; }
done <<< "$changed"

# 4. Large or secret-looking files about to be added (GitHub rejects files > 100 MB).
while IFS= read -r f; do
  [ -f "$f" ] || continue
  size=$(wc -c < "$f")
  if [ "$size" -gt 95000000 ]; then
    block "file too large for GitHub ($((size / 1000000)) MB): $f"
  elif [ "$size" -gt 20000000 ]; then
    echo "WARNING: large file ($((size / 1000000)) MB): $f"
  fi
  case "$f" in
    *.pfx|*.p12|*.pem|*.key|*.env|.env*|*secret*|*credential*) echo "WARNING: possible secret: $f" ;;
  esac
done <<< "$changed"

# 5. Compare with the remote and predict merge conflicts without touching the working tree.
if ! git fetch --prune "$remote" 2>&1 | sed 's/^/  fetch: /'; then
  echo "ERROR: fetch from '$remote' failed"; exit 2
fi

upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null)
if [ -z "$upstream" ] && git rev-parse -q --verify "refs/remotes/$remote/$branch" >/dev/null; then
  upstream="$remote/$branch"
fi

if [ -z "$upstream" ]; then
  echo "REMOTE: $remote/$branch does not exist yet (first push, use: git push -u $remote $branch)"
elif ! git rev-parse -q --verify HEAD >/dev/null; then
  block "remote branch $upstream exists but local repo has no commits; clone or pull it first"
else
  read -r ahead behind < <(git rev-list --left-right --count "HEAD...$upstream")
  echo "UPSTREAM: $upstream (ahead $ahead, behind $behind)"
  if [ "$behind" -gt 0 ]; then
    if [ "$ahead" -gt 0 ]; then
      if out=$(git merge-tree --write-tree --name-only HEAD "$upstream" 2>&1); then
        echo "SYNC: diverged, no conflicts predicted (git pull --rebase is safe)"
      else
        block "committed changes conflict with $upstream in:"
        echo "$out" | sed -n '2,/^$/p' | sed '/^$/d; s/^/  /'
      fi
    else
      echo "SYNC: behind only, fast-forward needed (git pull --rebase)"
    fi
    # Uncommitted edits to files that also changed upstream may still conflict once committed.
    incoming=$(git diff --name-only "HEAD...$upstream")
    overlap=$(comm -12 <(echo "$incoming" | sort -u) <(git diff --name-only HEAD | sort -u))
    [ -n "$overlap" ] && { echo "WARNING: uncommitted edits also changed upstream (re-run after committing):"; echo "$overlap" | sed 's/^/  /'; }
    untracked_clash=$(comm -12 <(echo "$incoming" | sort -u) <(git ls-files -o --exclude-standard | sort -u))
    [ -n "$untracked_clash" ] && { block "untracked files would be overwritten by upstream:"; echo "$untracked_clash" | sed 's/^/  /'; }
  else
    echo "SYNC: up to date with $upstream"
  fi
fi

if [ "$blocked" -eq 1 ]; then
  echo "RESULT: BLOCKED"
  exit 1
fi
echo "RESULT: OK"
