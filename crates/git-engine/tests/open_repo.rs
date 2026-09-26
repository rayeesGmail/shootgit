#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P0-09: `open_repo` finds the repository that contains a path (SPEC §4
//! Process model).
//!
//! Discovery is a file-system walk, like git's own: from the path upwards,
//! the first directory whose `.git` is a repository (a `.git` directory, or
//! a `.git` file that points at one, as in linked worktrees and submodules)
//! is the root of the working tree. Wherever git can answer too, the tests
//! check that `open_repo` agrees with `git rev-parse`.

use std::path::{Path, PathBuf};
use std::process::Command;

use git_engine::error::GitError;
use git_engine::git_binary::{resolve, ResolveOptions};
use git_engine::repo::{open_repo, Repo};
use tempfile::TempDir;

/// A temp dir to build repositories in, and a git that ignores the
/// machine's and the user's config (hooks, signing, templates).
struct Scratch {
    dir: TempDir,
    git: PathBuf,
    /// An empty file passed as `GIT_CONFIG_GLOBAL`.
    global_config: PathBuf,
}

impl Scratch {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let global_config = dir.path().join("gitconfig");
        std::fs::write(&global_config, "").unwrap();
        let git = resolve(&ResolveOptions::from_env(None)).await.unwrap().path;
        Self {
            dir,
            git,
            global_config,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    /// Runs git in `cwd` and returns its stdout; fails the test if git does.
    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new(&self.git)
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.global_config)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} in {} failed: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// A new repository at `relative`, with one commit when `commit` is set.
    fn init(&self, relative: &str, commit: bool) -> PathBuf {
        let root = self.path(relative);
        std::fs::create_dir_all(&root).unwrap();
        self.git(&root, &["init", "-q", "--initial-branch=main"]);
        if commit {
            self.git(
                &root,
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "base",
                ],
            );
        }
        root
    }

    /// What git reports for `start`: the root of the working tree, the git
    /// directory and the common directory, each canonical.
    fn git_view(&self, start: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let out = self.git(
            start,
            &[
                "rev-parse",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-common-dir",
            ],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out:?}");
        // `--git-common-dir` may be relative to the current directory; git
        // 2.30 has no `--path-format=absolute`.
        let common = start.join(lines[2]);
        (
            canonical(Path::new(lines[0])),
            canonical(Path::new(lines[1])),
            canonical(&common),
        )
    }

    /// `open_repo(start)` found the repository git finds from `start`.
    fn assert_agrees_with_git(&self, repo: &Repo, start: &Path) {
        let (workdir, git_dir, common_dir) = self.git_view(start);
        assert_eq!(repo.workdir(), workdir, "workdir from {}", start.display());
        assert_eq!(repo.git_dir(), git_dir, "git dir from {}", start.display());
        assert_eq!(
            repo.common_dir(),
            common_dir,
            "common dir from {}",
            start.display()
        );
    }
}

/// `path` with symlinks resolved (`/var` is `/private/var` on macOS), in the
/// form `open_repo` reports: no `\\?\` prefix on Windows.
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap()
}

// ---- working trees whose .git is a directory ---------------------------------

#[tokio::test]
async fn opens_a_repository_from_its_root() {
    let t = Scratch::new().await;
    let root = t.init("repo", false);

    let repo = open_repo(&t.git, &root).unwrap();

    assert_eq!(repo.git(), t.git);
    assert_eq!(repo.workdir(), canonical(&root));
    assert_eq!(repo.git_dir(), canonical(&root.join(".git")));
    assert_eq!(repo.common_dir(), repo.git_dir());
    t.assert_agrees_with_git(&repo, &root);
}

#[tokio::test]
async fn opens_a_repository_from_a_nested_subdirectory() {
    let t = Scratch::new().await;
    let root = t.init("repo", false);
    let nested = root.join("a").join("b c").join("ünïcødé");
    std::fs::create_dir_all(&nested).unwrap();

    let repo = open_repo(&t.git, &nested).unwrap();

    assert_eq!(repo.workdir(), canonical(&root));
    assert_eq!(repo.git_dir(), canonical(&root.join(".git")));
    t.assert_agrees_with_git(&repo, &nested);
}

#[tokio::test]
async fn opens_a_repository_from_a_file_inside_it() {
    let t = Scratch::new().await;
    let root = t.init("repo", false);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src").join("main.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let repo = open_repo(&t.git, &file).unwrap();

    assert_eq!(repo.workdir(), canonical(&root));
}

#[tokio::test]
async fn skips_a_dot_git_directory_that_is_not_a_repository() {
    // git keeps walking up past a `.git` directory without HEAD, objects/
    // and refs/.
    let t = Scratch::new().await;
    let outer = t.init("outer", false);
    let inner = outer.join("inner");
    std::fs::create_dir_all(inner.join(".git")).unwrap();

    let repo = open_repo(&t.git, &inner).unwrap();

    assert_eq!(repo.workdir(), canonical(&outer));
    t.assert_agrees_with_git(&repo, &inner);
}

// ---- working trees whose .git is a file ----------------------------------------

#[tokio::test]
async fn opens_a_linked_worktree_through_its_git_file() {
    let t = Scratch::new().await;
    let main = t.init("main", true);
    t.git(&main, &["worktree", "add", "-q", "../linked"]);
    let linked = t.path("linked");
    assert!(
        linked.join(".git").is_file(),
        "a linked worktree has a .git file"
    );
    let nested = linked.join("sub");
    std::fs::create_dir_all(&nested).unwrap();

    for start in [&linked, &nested] {
        let repo = open_repo(&t.git, start).unwrap();

        assert_eq!(repo.workdir(), canonical(&linked));
        assert_eq!(
            repo.git_dir(),
            canonical(&main.join(".git").join("worktrees").join("linked"))
        );
        assert_eq!(repo.common_dir(), canonical(&main.join(".git")));
        t.assert_agrees_with_git(&repo, start);
    }

    // The main working tree still opens as itself.
    let repo = open_repo(&t.git, &main).unwrap();
    assert_eq!(repo.workdir(), canonical(&main));
    assert_eq!(repo.git_dir(), canonical(&main.join(".git")));
    assert_eq!(repo.common_dir(), repo.git_dir());
}

#[tokio::test]
async fn follows_a_relative_gitdir_pointer() {
    // Submodules, and worktrees made with `worktree.useRelativePaths`, point
    // at their git directory with a path relative to the `.git` file.
    let t = Scratch::new().await;
    let work = t.path("work");
    let store = t.path("store");
    t.git(
        t.dir.path(),
        &[
            "init",
            "-q",
            "--initial-branch=main",
            "--separate-git-dir",
            store.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    // Removed first: Git for Windows marks `.git` hidden, and Windows refuses
    // to truncate a hidden file in place.
    std::fs::remove_file(work.join(".git")).unwrap();
    std::fs::write(work.join(".git"), "gitdir: ../store\n").unwrap();
    let nested = work.join("deep").join("er");
    std::fs::create_dir_all(&nested).unwrap();

    let repo = open_repo(&t.git, &nested).unwrap();

    assert_eq!(repo.workdir(), canonical(&work));
    assert_eq!(repo.git_dir(), canonical(&store));
    assert_eq!(repo.common_dir(), canonical(&store));
    t.assert_agrees_with_git(&repo, &nested);
}

#[tokio::test]
async fn a_git_file_that_does_not_point_at_a_repository_is_an_error() {
    // git stops at an invalid `.git` file instead of walking past it, so the
    // enclosing repository must not be opened either.
    let t = Scratch::new().await;
    let outer = t.init("outer", false);
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).unwrap();

    for contents in ["not a git file\n", "gitdir: ../nowhere\n", "gitdir: \n"] {
        std::fs::write(inner.join(".git"), contents).unwrap();

        let err = open_repo(&t.git, &inner).unwrap_err();

        match err {
            GitError::InvalidGitFile { ref path, .. } => {
                assert_eq!(path, &canonical(&inner).join(".git"), "{contents:?}");
            }
            other => panic!("{contents:?}: expected InvalidGitFile, got {other:?}"),
        }
    }
}

// ---- no working tree -------------------------------------------------------------

#[tokio::test]
async fn a_directory_outside_any_repository_is_not_a_repository() {
    let t = Scratch::new().await;
    let plain = t.path("plain");
    std::fs::create_dir_all(&plain).unwrap();

    let err = open_repo(&t.git, &plain).unwrap_err();

    match err {
        GitError::NotARepository { ref path } => assert_eq!(path, &plain),
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[tokio::test]
async fn a_missing_path_is_an_io_error() {
    let t = Scratch::new().await;
    let missing = t.path("missing");

    let err = open_repo(&t.git, &missing).unwrap_err();

    match err {
        GitError::Io {
            ref path,
            ref source,
        } => {
            assert_eq!(path, &missing);
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

#[tokio::test]
async fn a_bare_repository_or_a_git_directory_has_no_work_tree() {
    let t = Scratch::new().await;
    let bare = t.path("bare.git");
    t.git(
        t.dir.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    let normal = t.init("normal", true);

    for start in [
        bare.clone(),
        bare.join("refs"),
        normal.join(".git"),
        normal.join(".git").join("refs").join("heads"),
    ] {
        let err = open_repo(&t.git, &start).unwrap_err();

        assert!(
            matches!(err, GitError::NoWorkTree { .. }),
            "{}: expected NoWorkTree, got {err:?}",
            start.display()
        );
    }
}

// ---- Repo::new ---------------------------------------------------------------------

#[test]
fn repo_new_assumes_a_dot_git_directory_at_the_root() {
    let repo = Repo::new("git", "/work/tree");

    assert_eq!(repo.workdir(), Path::new("/work/tree"));
    assert_eq!(repo.git_dir(), Path::new("/work/tree/.git"));
    assert_eq!(repo.common_dir(), repo.git_dir());
}
