//! Working-tree status (SPEC §5 Command mapping: Status).
//!
//! [`status`] runs
//! `git status --porcelain=v2 -z --branch --untracked-files=all` and parses
//! the result into a [`RepoInfo`] and one [`StatusEntry`] per path that is
//! not clean. Status is read through the CLI only: §4 gives gix the log,
//! blame, tree/blob and ref reads, and §5 maps Status to this command.
//!
//! The parser follows "Porcelain Format Version 2" in git-status(1). Under
//! `-z` every record ends in NUL and paths are printed verbatim, with no
//! quoting, so spaces and even newlines belong to the path. A rename or copy
//! is the one entry that spans two records: the new path, then the old one.

use std::path::PathBuf;

use crate::error::GitError;
use crate::process::{split_nul, CancellationToken};
use crate::repo::Repo;

/// The command §5 gives for Status.
const STATUS_ARGS: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--branch",
    "--untracked-files=all",
];

/// What to report beyond the fixed §5 command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusOptions {
    /// Also report ignored paths, with `--ignored=matching`: a directory that
    /// matches an ignore pattern is one entry (`build/`) rather than a list
    /// of every file in it, so `target/` or `node_modules/` stay cheap.
    pub include_ignored: bool,
}

/// What one `git status` run reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub repo: RepoInfo,
    /// In the order git printed them.
    pub entries: Vec<StatusEntry>,
}

/// The repository half of §5 `RepoInfo`, from the `# branch.*` headers.
///
/// Two §5 fields are not here yet: `id`, which names an open repository and
/// comes with the open-repository plumbing (P0-09, P0-12), and `state`
/// (merging, rebasing, ...), which porcelain v2 does not report and P2-11
/// reads from the markers in `.git`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// The root of the working tree, as the [`Repo`] names it.
    pub path: PathBuf,
    pub head: Head,
    /// The upstream branch, such as `origin/main`, when one is configured.
    pub upstream: Option<String>,
    /// How far HEAD is ahead of and behind `upstream`. `None` without an
    /// upstream, and when the upstream branch no longer exists.
    pub ahead_behind: Option<AheadBehind>,
}

/// Where HEAD points.
///
/// Branch names are converted to UTF-8 lossily; commit ids are hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// On a branch that has commits.
    Branch { name: String, oid: String },
    /// On a branch with no commits yet: a new repository, or after
    /// `git switch --orphan`.
    Unborn { name: String },
    /// On a commit rather than a branch.
    Detached { oid: String },
}

/// Commit counts between HEAD and its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    /// Commits on HEAD that the upstream lacks.
    pub ahead: u32,
    /// Commits on the upstream that HEAD lacks.
    pub behind: u32,
}

/// One path that is not clean (§5 `StatusEntry`).
///
/// `path` and `old_path` are relative to the root of the working tree and
/// `/`-separated, exactly as git prints them. On Unix they are git's bytes,
/// which need not be UTF-8; on Windows git prints UTF-8, and any invalid
/// sequence is replaced. An ignored directory keeps its trailing `/`.
///
/// `index_status` and `worktree_status` are the two letters of git's `XY`
/// code:
/// - ordinary, renamed and copied entries: the change from HEAD to the index
///   (X) and from the index to the working tree (Y);
/// - unmerged entries (`is_conflicted`): the conflict, ours then theirs:
///   `UU` both modified, `AA` both added, `DD` both deleted, `AU`/`UA` added
///   by us/them, `DU`/`UD` deleted by us/them;
/// - untracked and ignored entries: `Untracked` or `Ignored` in both, like
///   porcelain v1's `??` and `!!`.
///
/// §5 also lists `changelist_id`; it arrives with changelists (P6-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: PathBuf,
    /// Where a renamed or copied path came from.
    pub old_path: Option<PathBuf>,
    pub index_status: FileStatus,
    pub worktree_status: FileStatus,
    /// The path has unmerged index entries: a merge, rebase, cherry-pick,
    /// revert or stash apply stopped on it.
    pub is_conflicted: bool,
    /// The path is a submodule.
    pub is_submodule: bool,
}

/// One letter of git's `XY` status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileStatus {
    /// `.`
    Unmodified,
    /// `M`
    Modified,
    /// `T`: changed between file, symlink and submodule.
    TypeChanged,
    /// `A`
    Added,
    /// `D`
    Deleted,
    /// `R`
    Renamed,
    /// `C`
    Copied,
    /// `U`, only in the conflict code of an unmerged entry.
    Unmerged,
    /// An untracked path (`?` record).
    Untracked,
    /// An ignored path (`!` record).
    Ignored,
}

impl FileStatus {
    /// One letter of a porcelain v2 `XY` field.
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            b'.' => Self::Unmodified,
            b'M' => Self::Modified,
            b'T' => Self::TypeChanged,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'U' => Self::Unmerged,
            _ => return None,
        })
    }
}

/// Runs `git status` in `repo` and parses what it prints.
///
/// Cancelling `cancel` stops the read, whether it is queued for a git slot or
/// running, with [`GitError::Cancelled`]; a newer status request cancels the
/// one in flight this way (§4 Low-resource operation, rule 4). A `repo` that
/// is not a working tree fails with [`GitError::Failed`].
pub async fn status(
    repo: &Repo,
    options: &StatusOptions,
    cancel: &CancellationToken,
) -> Result<Status, GitError> {
    let mut git = repo.git_command();
    git.args(STATUS_ARGS).cancel_token(cancel.clone());
    if options.include_ignored {
        git.arg("--ignored=matching");
    }
    let output = git.output().await?;
    parse(&output.stdout, repo.workdir().to_path_buf())
}

/// Parses the output of [`STATUS_ARGS`] run in the working tree at `path`.
fn parse(output: &[u8], path: PathBuf) -> Result<Status, GitError> {
    parse_records(output, path).map_err(|reason| GitError::UnexpectedOutput {
        command: "status",
        reason,
    })
}

fn parse_records(output: &[u8], path: PathBuf) -> Result<Status, String> {
    let mut branch = BranchHeaders::default();
    let mut entries = Vec::new();
    let mut records = split_nul(output);
    while let Some(record) = records.next() {
        let entry = match record.first() {
            Some(b'#') => {
                branch.read(record)?;
                continue;
            }
            Some(b'1') => ordinary(record)?,
            Some(b'2') => {
                let old_path = records
                    .next()
                    .ok_or_else(|| describe(record, "rename or copy without its original path"))?;
                renamed_or_copied(record, old_path)?
            }
            Some(b'u') => unmerged(record)?,
            Some(b'?') => path_only(record, FileStatus::Untracked)?,
            Some(b'!') => path_only(record, FileStatus::Ignored)?,
            _ => return Err(describe(record, "unknown record type")),
        };
        entries.push(entry);
    }
    Ok(Status {
        repo: branch.into_repo_info(path)?,
        entries,
    })
}

/// The `# branch.*` headers `--branch` adds.
///
/// Other headers, such as `# stash <n>` when `status.showStash` is set, are
/// skipped, as git-status(1) tells parsers to do.
#[derive(Default)]
struct BranchHeaders {
    oid: Option<String>,
    head: Option<String>,
    upstream: Option<String>,
    ahead_behind: Option<AheadBehind>,
}

impl BranchHeaders {
    fn read(&mut self, record: &[u8]) -> Result<(), String> {
        let Some(header) = record.strip_prefix(b"# ") else {
            return Ok(());
        };
        let mut parts = header.splitn(2, |&b| b == b' ');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        match key {
            b"branch.oid" => self.oid = Some(lossy(value)),
            b"branch.head" => self.head = Some(lossy(value)),
            b"branch.upstream" => self.upstream = Some(lossy(value)),
            b"branch.ab" => {
                let counts = parse_ahead_behind(value)
                    .ok_or_else(|| describe(record, "bad ahead/behind counts"))?;
                self.ahead_behind = Some(counts);
            }
            _ => {}
        }
        Ok(())
    }

    fn into_repo_info(self, path: PathBuf) -> Result<RepoInfo, String> {
        let (Some(oid), Some(name)) = (self.oid, self.head) else {
            return Err("missing `# branch.oid` or `# branch.head` header".to_owned());
        };
        if oid.is_empty() || name.is_empty() {
            return Err("empty `# branch.oid` or `# branch.head` header".to_owned());
        }
        let head = match (oid.as_str(), name.as_str()) {
            ("(initial)", "(detached)") => {
                return Err("detached HEAD without a commit".to_owned());
            }
            (_, "(detached)") => Head::Detached { oid },
            ("(initial)", _) => Head::Unborn { name },
            _ => Head::Branch { name, oid },
        };
        Ok(RepoInfo {
            path,
            head,
            upstream: self.upstream,
            ahead_behind: self.ahead_behind,
        })
    }
}

/// `+<ahead> -<behind>`.
fn parse_ahead_behind(value: &[u8]) -> Option<AheadBehind> {
    let (ahead, behind) = std::str::from_utf8(value).ok()?.split_once(' ')?;
    Some(AheadBehind {
        ahead: ahead.strip_prefix('+')?.parse().ok()?,
        behind: behind.strip_prefix('-')?.parse().ok()?,
    })
}

/// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
fn ordinary(record: &[u8]) -> Result<StatusEntry, String> {
    let [_, xy, sub, _, _, _, _, _, path] = fields::<9>(record, b"1")?;
    tracked(record, xy, sub, path, None, false)
}

/// `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>`, then
/// `<origPath>` as the next record.
fn renamed_or_copied(record: &[u8], old_path: &[u8]) -> Result<StatusEntry, String> {
    let [_, xy, sub, _, _, _, _, _, score, path] = fields::<10>(record, b"2")?;
    let score_ok = matches!(
        score.split_first(),
        Some((b'R' | b'C', digits)) if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
    );
    if !score_ok {
        return Err(describe(record, "bad rename or copy score"));
    }
    if old_path.is_empty() {
        return Err(describe(record, "empty original path"));
    }
    tracked(record, xy, sub, path, Some(old_path), false)
}

/// `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
fn unmerged(record: &[u8]) -> Result<StatusEntry, String> {
    let [_, xy, sub, _, _, _, _, _, _, _, path] = fields::<11>(record, b"u")?;
    tracked(record, xy, sub, path, None, true)
}

/// `? <path>` or `! <path>`.
fn path_only(record: &[u8], status: FileStatus) -> Result<StatusEntry, String> {
    match record {
        [_, b' ', path @ ..] if !path.is_empty() => Ok(StatusEntry {
            path: git_path(path),
            old_path: None,
            index_status: status,
            worktree_status: status,
            is_conflicted: false,
            is_submodule: false,
        }),
        _ => Err(describe(record, "missing path")),
    }
}

/// Splits an entry into its `N` space-separated fields. The last field is
/// the path, spaces included; the first must be `kind`.
fn fields<'a, const N: usize>(record: &'a [u8], kind: &[u8]) -> Result<[&'a [u8]; N], String> {
    let mut parts = record.splitn(N, |&b| b == b' ');
    let mut fields: [&[u8]; N] = [&[]; N];
    for field in &mut fields {
        *field = parts
            .next()
            .ok_or_else(|| describe(record, "too few fields"))?;
    }
    if fields.first() != Some(&kind) {
        return Err(describe(record, "unknown record type"));
    }
    if fields.last().is_none_or(|path| path.is_empty()) {
        return Err(describe(record, "empty path"));
    }
    Ok(fields)
}

/// The entry for a tracked path, from its `XY` and `<sub>` fields.
fn tracked(
    record: &[u8],
    xy: &[u8],
    sub: &[u8],
    path: &[u8],
    old_path: Option<&[u8]>,
    is_conflicted: bool,
) -> Result<StatusEntry, String> {
    let codes = match xy {
        &[x, y] => FileStatus::from_code(x).zip(FileStatus::from_code(y)),
        _ => None,
    };
    let Some((index_status, worktree_status)) = codes else {
        return Err(describe(record, "bad XY status"));
    };
    // `N...` for a plain path, `S<c><m><u>` for a submodule.
    let is_submodule = match sub {
        b"N..." => false,
        [b'S', _, _, _] => true,
        _ => return Err(describe(record, "bad submodule field")),
    };
    Ok(StatusEntry {
        path: git_path(path),
        old_path: old_path.map(git_path),
        index_status,
        worktree_status,
        is_conflicted,
        is_submodule,
    })
}

/// A path exactly as git printed it.
#[cfg(unix)]
fn git_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

/// A path as git printed it. Git for Windows prints UTF-8 (it converts
/// paths to and from UTF-16 itself).
#[cfg(not(unix))]
fn git_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `problem`, followed by the record it was found in.
fn describe(record: &[u8], problem: &str) -> String {
    format!("{problem} in {:?}", String::from_utf8_lossy(record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const OID: &str = "b4b4f68c21fb04561c652a0389c58e65dec19590";
    const H1: &str = "5626abf0f72e58d7a153368ba57db4c673c0e171";
    const H2: &str = "814f4a422927b82f5f8a43f8fab6d3839e3983f2";
    const H0: &str = "0000000000000000000000000000000000000000";

    /// `-z` output: every record NUL-terminated.
    fn z<S: AsRef<[u8]>>(records: &[S]) -> Vec<u8> {
        let mut out = Vec::new();
        for record in records {
            out.extend_from_slice(record.as_ref());
            out.push(0);
        }
        out
    }

    /// `-z` output for `main` at `OID` without an upstream, then `entries`.
    fn on_main(entries: &[&str]) -> Vec<u8> {
        let mut records = vec![
            format!("# branch.oid {OID}"),
            "# branch.head main".to_owned(),
        ];
        records.extend(entries.iter().map(|e| (*e).to_owned()));
        z(&records)
    }

    fn parse_ok(output: &[u8]) -> Status {
        parse(output, PathBuf::from("/repo")).unwrap()
    }

    fn assert_unexpected(output: &[u8]) {
        match parse(output, PathBuf::from("/repo")) {
            Err(GitError::UnexpectedOutput {
                command: "status", ..
            }) => {}
            other => panic!("expected UnexpectedOutput, got {other:?}"),
        }
    }

    fn entry(
        path: &str,
        old_path: Option<&str>,
        index_status: FileStatus,
        worktree_status: FileStatus,
    ) -> StatusEntry {
        StatusEntry {
            path: PathBuf::from(path),
            old_path: old_path.map(PathBuf::from),
            index_status,
            worktree_status,
            is_conflicted: false,
            is_submodule: false,
        }
    }

    // ---- headers -----------------------------------------------------------

    #[test]
    fn branch_with_upstream_reports_ahead_and_behind() {
        let status = parse_ok(&z(&[
            format!("# branch.oid {OID}"),
            "# branch.head feature/x".to_owned(),
            "# branch.upstream origin/feature/x".to_owned(),
            "# branch.ab +3 -12".to_owned(),
        ]));

        assert_eq!(
            status.repo,
            RepoInfo {
                path: PathBuf::from("/repo"),
                head: Head::Branch {
                    name: "feature/x".to_owned(),
                    oid: OID.to_owned(),
                },
                upstream: Some("origin/feature/x".to_owned()),
                ahead_behind: Some(AheadBehind {
                    ahead: 3,
                    behind: 12
                }),
            }
        );
        assert_eq!(status.entries, []);
    }

    #[test]
    fn branch_without_upstream_has_no_ahead_behind() {
        let status = parse_ok(&on_main(&[]));

        assert_eq!(status.repo.upstream, None);
        assert_eq!(status.repo.ahead_behind, None);
    }

    #[test]
    fn upstream_whose_ref_is_gone_has_no_ahead_behind() {
        // git prints `branch.ab` only when the upstream ref exists.
        let status = parse_ok(&z(&[
            format!("# branch.oid {OID}"),
            "# branch.head main".to_owned(),
            "# branch.upstream origin/main".to_owned(),
        ]));

        assert_eq!(status.repo.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.repo.ahead_behind, None);
    }

    #[test]
    fn detached_head_reports_its_commit() {
        let status = parse_ok(&z(&[
            format!("# branch.oid {OID}"),
            "# branch.head (detached)".to_owned(),
        ]));

        assert_eq!(
            status.repo.head,
            Head::Detached {
                oid: OID.to_owned()
            }
        );
    }

    #[test]
    fn unborn_branch_has_no_commit() {
        let status = parse_ok(&z(&["# branch.oid (initial)", "# branch.head main"]));

        assert_eq!(
            status.repo.head,
            Head::Unborn {
                name: "main".to_owned()
            }
        );
    }

    #[test]
    fn unknown_headers_are_ignored() {
        // `status.showStash` adds `# stash <n>`; git-status(1) tells parsers
        // to skip headers they do not know.
        let status = parse_ok(&z(&[
            format!("# branch.oid {OID}"),
            "# stash 3".to_owned(),
            "# branch.head main".to_owned(),
            "# branch.something-new with a value".to_owned(),
        ]));

        assert_eq!(status.repo.head, parse_ok(&on_main(&[])).repo.head);
    }

    // ---- entries -----------------------------------------------------------

    #[test]
    fn ordinary_entries_keep_spaces_in_the_path() {
        let status = parse_ok(&on_main(&[
            &format!("1 .M N... 100644 100644 100644 {H1} {H1} dir with space/a  b.txt"),
            &format!("1 A. N... 000000 100644 100644 {H0} {H2} added.txt"),
            &format!("1 MD N... 100644 100644 000000 {H1} {H2} gone.txt"),
        ]));

        assert_eq!(
            status.entries,
            [
                entry(
                    "dir with space/a  b.txt",
                    None,
                    FileStatus::Unmodified,
                    FileStatus::Modified
                ),
                entry("added.txt", None, FileStatus::Added, FileStatus::Unmodified),
                entry("gone.txt", None, FileStatus::Modified, FileStatus::Deleted),
            ]
        );
    }

    #[test]
    fn type_changes_and_submodules_are_recognised() {
        let status = parse_ok(&on_main(&[
            &format!("1 .T N... 100644 100644 120000 {H1} {H1} link"),
            &format!("1 .M SC.U 160000 160000 160000 {H1} {H1} vendor/lib"),
        ]));

        assert_eq!(
            status.entries[0],
            entry(
                "link",
                None,
                FileStatus::Unmodified,
                FileStatus::TypeChanged
            )
        );
        assert_eq!(
            status.entries[1],
            StatusEntry {
                is_submodule: true,
                ..entry(
                    "vendor/lib",
                    None,
                    FileStatus::Unmodified,
                    FileStatus::Modified
                )
            }
        );
    }

    #[test]
    fn a_rename_takes_the_next_field_as_its_old_path() {
        let status = parse_ok(&on_main(&[
            &format!("2 R. N... 100644 100644 100644 {H1} {H1} R100 new name.txt"),
            "old name.txt",
            &format!("2 RM N... 100644 100644 100644 {H1} {H1} R87 ünïcødé/日本語.txt"),
            "café.txt",
            &format!("2 C. N... 100644 100644 100644 {H1} {H1} C75 copy.txt"),
            "source.txt",
            "? after.txt",
        ]));

        assert_eq!(
            status.entries,
            [
                entry(
                    "new name.txt",
                    Some("old name.txt"),
                    FileStatus::Renamed,
                    FileStatus::Unmodified
                ),
                entry(
                    "ünïcødé/日本語.txt",
                    Some("café.txt"),
                    FileStatus::Renamed,
                    FileStatus::Modified
                ),
                entry(
                    "copy.txt",
                    Some("source.txt"),
                    FileStatus::Copied,
                    FileStatus::Unmodified
                ),
                entry(
                    "after.txt",
                    None,
                    FileStatus::Untracked,
                    FileStatus::Untracked
                ),
            ]
        );
    }

    #[test]
    fn unmerged_entries_are_conflicted() {
        let status = parse_ok(&on_main(&[
            &format!("u UD N... 100644 100644 000000 100644 {H1} {H2} {H0} conflict file.txt"),
            &format!("u AA N... 000000 100644 100644 100644 {H0} {H1} {H2} both.txt"),
            &format!("u DU N... 100644 000000 100644 100644 {H1} {H0} {H2} theirs.txt"),
        ]));

        let conflicted = |path, ours, theirs| StatusEntry {
            is_conflicted: true,
            ..entry(path, None, ours, theirs)
        };
        assert_eq!(
            status.entries,
            [
                conflicted(
                    "conflict file.txt",
                    FileStatus::Unmerged,
                    FileStatus::Deleted
                ),
                conflicted("both.txt", FileStatus::Added, FileStatus::Added),
                conflicted("theirs.txt", FileStatus::Deleted, FileStatus::Unmerged),
            ]
        );
    }

    #[test]
    fn untracked_and_ignored_entries_have_only_a_path() {
        let status = parse_ok(&on_main(&["? new file.txt", "! build/", "! debug.log"]));

        assert_eq!(
            status.entries,
            [
                entry(
                    "new file.txt",
                    None,
                    FileStatus::Untracked,
                    FileStatus::Untracked
                ),
                entry("build/", None, FileStatus::Ignored, FileStatus::Ignored),
                entry("debug.log", None, FileStatus::Ignored, FileStatus::Ignored),
            ]
        );
    }

    #[test]
    fn paths_may_contain_newlines() {
        // Only NUL ends a record under -z.
        let status = parse_ok(&on_main(&["? line\nbreak.txt"]));

        assert_eq!(status.entries[0].path, Path::new("line\nbreak.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_kept_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;

        let mut records: Vec<Vec<u8>> = vec![
            format!("# branch.oid {OID}").into_bytes(),
            b"# branch.head main".to_vec(),
            format!("2 R. N... 100644 100644 100644 {H1} {H1} R100 caf").into_bytes(),
            b"old-\xff".to_vec(),
        ];
        records[2].push(0xe9);
        let status = parse_ok(&z(&records));

        assert_eq!(status.entries[0].path.as_os_str().as_bytes(), b"caf\xe9");
        assert_eq!(
            status.entries[0]
                .old_path
                .as_ref()
                .map(|p| p.as_os_str().as_bytes()),
            Some(&b"old-\xff"[..])
        );
    }

    // ---- malformed output --------------------------------------------------

    #[test]
    fn output_without_branch_headers_is_rejected() {
        assert_unexpected(b"");
        assert_unexpected(&z(&["# branch.head main"]));
        assert_unexpected(&z(&[format!("# branch.oid {OID}")]));
    }

    #[test]
    fn detached_head_without_a_commit_is_rejected() {
        assert_unexpected(&z(&["# branch.oid (initial)", "# branch.head (detached)"]));
    }

    #[test]
    fn malformed_ahead_behind_is_rejected() {
        for ab in ["# branch.ab 3 12", "# branch.ab +3", "# branch.ab +x -1"] {
            assert_unexpected(&on_main(&[ab]));
        }
    }

    #[test]
    fn truncated_entries_are_rejected() {
        assert_unexpected(&on_main(&["1 .M N... 100644 100644"]));
        assert_unexpected(&on_main(&[&format!(
            "1 .M N... 100644 100644 100644 {H1} {H1} "
        )]));
        assert_unexpected(&on_main(&[&format!(
            "u UU N... 100644 100644 100644 100644 {H1} {H1} path"
        )]));
        assert_unexpected(&on_main(&["?"]));
        assert_unexpected(&on_main(&["? "]));
    }

    #[test]
    fn a_rename_without_its_old_path_is_rejected() {
        assert_unexpected(&on_main(&[&format!(
            "2 R. N... 100644 100644 100644 {H1} {H1} R100 new.txt"
        )]));
    }

    #[test]
    fn unknown_status_letters_and_entry_types_are_rejected() {
        assert_unexpected(&on_main(&[&format!(
            "1 .Z N... 100644 100644 100644 {H1} {H1} file"
        )]));
        assert_unexpected(&on_main(&[&format!(
            "1 M N... 100644 100644 100644 {H1} {H1} file"
        )]));
        assert_unexpected(&on_main(&[&format!(
            "1 .M X... 100644 100644 100644 {H1} {H1} file"
        )]));
        assert_unexpected(&on_main(&[
            &format!("2 .M N... 100644 100644 100644 {H1} {H1} X100 new"),
            "old",
        ]));
        assert_unexpected(&on_main(&["x something"]));
        assert_unexpected(&on_main(&["1x .M N..."]));
    }
}
