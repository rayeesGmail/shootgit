#!/usr/bin/env bash
# Fixture "basic": a repository whose `git status --porcelain=v2` shows every
# entry kind `git_engine::status` parses (P0-08, SPEC §5 Status).
#
#   kind              path(s)                                    XY
#   ordinary      1   modified.txt                               .M
#                     staged.txt                                 M.
#                     added.txt                                  A.
#                     deleted.txt                                .D
#                     source.txt                                 M.
#   renamed       2   old name.txt -> new name.txt               R.  (spaces)
#                     café.txt -> ünïcødé/日本語.txt             R.  (unicode)
#   copied        2   source.txt -> copy.txt                     C.
#   unmerged      u   both-modified.txt                          UU
#                     both-added.txt                             AA
#                     deleted-by-them.txt                        UD
#   untracked     ?   untracked.txt, untracked dir/ñested.txt
#   ignored       !   debug.log, build/ (with --ignored=matching)
#
# The repository is mid-merge (the source of the unmerged entries), is on
# `main`, and tracks `origin/main`, which it is 1 commit ahead of and 1
# behind. Copies are reported because the repository sets
# `status.renames=copies`.
#
# Usage: basic.sh <dest>
#
# <dest> must be missing or empty; the repository is created there. Needs
# bash and git >= 2.30 (Git Bash on Windows). File contents are written with
# printf, so they are LF on every OS.

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <dest>" >&2
    exit 2
fi
dest=$1

# The fixture must come out the same on every machine, so neither the
# machine's nor the user's git config may reach it (hooks, signing, rename
# settings, autocrlf on Windows). GIT_CONFIG_GLOBAL needs git 2.32; older git
# still reads ~/.gitconfig, and the settings that matter are pinned locally
# below either way.
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=Fixture
export GIT_AUTHOR_EMAIL=fixture@example.invalid
export GIT_AUTHOR_DATE='2026-01-01T00:00:00Z'
export GIT_COMMITTER_NAME=Fixture
export GIT_COMMITTER_EMAIL=fixture@example.invalid
export GIT_COMMITTER_DATE='2026-01-01T00:00:00Z'

mkdir -p "$dest"
cd "$dest"
if [ -n "$(ls -A .)" ]; then
    echo "$0: $dest is not empty" >&2
    exit 1
fi

git init -q --initial-branch=main .
git config core.autocrlf false
git config commit.gpgsign false
git config status.renames copies

# ---- history: base, then one commit on each side of the merge -------------

printf 'one\n' >modified.txt
printf 'one\n' >staged.txt
printf 'one\n' >deleted.txt
printf 'rename me\n' >'old name.txt'
printf 'rename me too\n' >'café.txt'
printf 'line %s\n' 1 2 3 4 5 6 7 8 9 10 >source.txt
printf 'base\n' >both-modified.txt
printf 'base\n' >deleted-by-them.txt
printf '*.log\nbuild/\n' >.gitignore
git add -A
git commit -q -m base

git switch -q -c other
printf 'theirs\n' >both-modified.txt
printf 'theirs\n' >both-added.txt
git rm -q deleted-by-them.txt
git add -A
git commit -q -m theirs

git switch -q main
printf 'ours\n' >both-modified.txt
printf 'ours\n' >both-added.txt
printf 'ours\n' >deleted-by-them.txt
git add -A
git commit -q -m ours

# ---- upstream: origin/main is `other`, so main is 1 ahead and 1 behind -----
# The remote is never contacted; it only supplies the fetch refspec that maps
# refs/heads/main to refs/remotes/origin/main.

git remote add origin ../origin.git
git update-ref refs/remotes/origin/main other
git branch -q --set-upstream-to=origin/main main

# ---- unmerged: UU, AA, UD --------------------------------------------------

if git merge -q --no-edit other >/dev/null 2>&1; then
    echo "$0: expected the merge of 'other' to stop on conflicts" >&2
    exit 1
fi

# ---- ordinary, renamed and copied ------------------------------------------

printf 'two\n' >>modified.txt
printf 'two\n' >>staged.txt
printf 'new\n' >added.txt
rm deleted.txt
git mv 'old name.txt' 'new name.txt'
mkdir 'ünïcødé'
git mv 'café.txt' 'ünïcødé/日本語.txt'
# A copy is only detected from a source that changed in the same diff.
cp source.txt copy.txt
printf 'line 11\n' >>source.txt
git add staged.txt added.txt source.txt copy.txt

# ---- untracked and ignored -------------------------------------------------

printf 'untracked\n' >untracked.txt
mkdir 'untracked dir'
printf 'untracked\n' >'untracked dir/ñested.txt'
printf 'log\n' >debug.log
mkdir build
printf 'build output\n' >build/out.bin
