use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BASE_CANDIDATES: [&str; 6] = [
    "origin/develop",
    "origin/main",
    "origin/master",
    "develop",
    "main",
    "master",
];
const MAX_COUNTED_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Pr,
    Uncommitted,
}

impl Mode {
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Pr => Self::Uncommitted,
            Self::Uncommitted => Self::Pr,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pr => "PR",
            Self::Uncommitted => "não commitado",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: char,
    pub adds: u64,
    pub dels: u64,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RepoDiff {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub base_label: String,
    pub base_rev: String,
    pub files: Vec<FileChange>,
    pub error: Option<String>,
}

impl RepoDiff {
    pub(crate) fn adds(&self) -> u64 {
        self.files.iter().map(|file| file.adds).sum()
    }

    pub(crate) fn dels(&self) -> u64 {
        self.files.iter().map(|file| file.dels).sum()
    }

    pub(crate) fn has_changes(&self) -> bool {
        !self.files.is_empty()
    }
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null());
    command
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = git_command(root)
        .args(args)
        .output()
        .map_err(|err| format!("git: {err}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .next()
        .unwrap_or("git falhou")
        .trim()
        .to_owned())
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    git_bytes(root, args).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn verified(root: &Path, rev: &str) -> bool {
    git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .is_ok()
}

pub(crate) fn base_branch(root: &Path) -> Option<String> {
    let origin_head = git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .map(|text| text.trim().to_owned())
    .filter(|name| !name.is_empty() && verified(root, name));
    origin_head.or_else(|| {
        BASE_CANDIDATES
            .iter()
            .find(|candidate| verified(root, candidate))
            .map(|candidate| (*candidate).to_owned())
    })
}

pub(crate) fn current_branch(root: &Path) -> Option<String> {
    git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            git(root, &["rev-parse", "--short", "HEAD"])
                .ok()
                .map(|sha| format!("({})", sha.trim()))
        })
}

pub(crate) fn resolve_base(root: &Path, mode: Mode) -> Result<(String, String), String> {
    if !verified(root, "HEAD") {
        return Ok(("vazio".to_owned(), EMPTY_TREE.to_owned()));
    }
    if mode == Mode::Uncommitted {
        return Ok(("HEAD".to_owned(), "HEAD".to_owned()));
    }
    let Some(base) = base_branch(root) else {
        return Ok(("HEAD (sem base)".to_owned(), "HEAD".to_owned()));
    };
    let merge_base = git(root, &["merge-base", "HEAD", &base])
        .map_err(|err| format!("merge-base com {base}: {err}"))?;
    Ok((base, merge_base.trim().to_owned()))
}

pub(crate) fn load_repo(root: &Path, mode: Mode) -> RepoDiff {
    let mut repo = RepoDiff {
        root: root.to_path_buf(),
        branch: current_branch(root),
        ..RepoDiff::default()
    };
    match resolve_base(root, mode).and_then(|(label, rev)| {
        repo.base_label = label;
        repo.base_rev = rev.clone();
        changed_files(root, &rev)
    }) {
        Ok(files) => repo.files = files,
        Err(error) => repo.error = Some(error),
    }
    repo
}

pub(crate) fn changed_files(root: &Path, base_rev: &str) -> Result<Vec<FileChange>, String> {
    let diff_args = |kind: &'static str| ["diff", "--no-ext-diff", kind, "-z", "-M", base_rev];
    let statuses = parse_name_status(&git(root, &diff_args("--name-status"))?);
    let counts = parse_numstat(&git(root, &diff_args("--numstat"))?);
    let mut files: Vec<FileChange> = statuses
        .into_iter()
        .map(|(status, old_path, path)| {
            let (adds, dels, binary) = counts.get(&path).copied().unwrap_or_default();
            FileChange {
                path,
                old_path,
                status,
                adds,
                dels,
                binary,
            }
        })
        .collect();
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        let (adds, binary) = count_lines(&root.join(path));
        files.push(FileChange {
            path: path.to_owned(),
            old_path: None,
            status: '?',
            adds,
            dels: 0,
            binary,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn count_lines(path: &Path) -> (u64, bool) {
    let too_big = std::fs::metadata(path).map_or(true, |meta| meta.len() > MAX_COUNTED_BYTES);
    if too_big {
        return (0, true);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (0, false);
    };
    if bytes.contains(&0) {
        return (0, true);
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let trailing = u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    (newlines + trailing, false)
}

pub(crate) fn parse_name_status(text: &str) -> Vec<(char, Option<String>, String)> {
    let mut fields = text.split('\0').filter(|field| !field.is_empty());
    let mut entries = Vec::new();
    while let Some(code) = fields.next() {
        let status = code.chars().next().unwrap_or('M');
        if matches!(status, 'R' | 'C') {
            let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
                break;
            };
            entries.push((status, Some(old.to_owned()), new.to_owned()));
        } else if let Some(path) = fields.next() {
            entries.push((status, None, path.to_owned()));
        }
    }
    entries
}

pub(crate) fn parse_numstat(text: &str) -> HashMap<String, (u64, u64, bool)> {
    let mut fields = text.split('\0');
    let mut counts = HashMap::new();
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (Some(adds), Some(dels), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            match fields.nth(1) {
                Some(new) => new.to_owned(),
                None => break,
            }
        } else {
            path.to_owned()
        };
        let binary = adds == "-" || dels == "-";
        counts.insert(
            path,
            (
                adds.parse().unwrap_or_default(),
                dels.parse().unwrap_or_default(),
                binary,
            ),
        );
    }
    counts
}

fn raw_file_diff(
    root: &Path,
    base_rev: &str,
    file: &FileChange,
    color: bool,
) -> Result<Vec<u8>, String> {
    let color = if color {
        "--color=always"
    } else {
        "--color=never"
    };
    let mut command = git_command(root);
    command.args(["diff", "--no-ext-diff", color, "-M"]);
    if file.status == '?' {
        command.args(["--no-index", "--", "/dev/null", &file.path]);
    } else {
        command.arg(base_rev).arg("--");
        if let Some(old) = &file.old_path {
            command.arg(old);
        }
        command.arg(&file.path);
    }
    let output = command.output().map_err(|err| format!("git: {err}"))?;
    let expected =
        output.status.success() || (file.status == '?' && output.status.code() == Some(1));
    if !expected {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

pub(crate) fn delta_available() -> bool {
    Command::new("delta")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn file_diff(
    root: &Path,
    base_rev: &str,
    file: &FileChange,
    width: u16,
    use_delta: bool,
) -> Result<Vec<u8>, String> {
    if !use_delta {
        return raw_file_diff(root, base_rev, file, true);
    }
    let raw = raw_file_diff(root, base_rev, file, false)?;
    let mut child = Command::new("delta")
        .args(["--light", "--paging=never", "--line-numbers"])
        .arg(format!("--width={}", width.max(20)))
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("delta: {err}"))?;
    let mut stdin = child.stdin.take().ok_or("delta sem stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&raw));
    let output = child
        .wait_with_output()
        .map_err(|err| format!("delta: {err}"))?;
    let _ = writer.join();
    Ok(output.stdout)
}

pub(crate) fn first_changed_line(root: &Path, base_rev: &str, file: &FileChange) -> u32 {
    if file.status == '?' {
        return 1;
    }
    git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--color=never",
            "-U0",
            base_rev,
            "--",
            &file.path,
        ],
    )
    .ok()
    .and_then(|text| parse_first_hunk_line(&text))
    .unwrap_or(1)
}

pub(crate) fn parse_first_hunk_line(diff: &str) -> Option<u32> {
    let line = diff.lines().find(|line| line.starts_with("@@ "))?;
    let new_side = line.split_whitespace().find(|part| part.starts_with('+'))?;
    let start = new_side
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse::<u32>()
        .ok()?;
    Some(start.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "herdr-diff-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.email", "t@example.com"]);
            repo.git(&["config", "user.name", "t"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }

        fn write(&self, path: &str, body: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn name_status_and_numstat_parse_renames_and_binaries() {
        let statuses = parse_name_status("M\0src/a.rs\0R087\0old.rs\0new.rs\0A\0b.txt\0");
        assert_eq!(
            statuses,
            [
                ('M', None, "src/a.rs".to_owned()),
                ('R', Some("old.rs".to_owned()), "new.rs".to_owned()),
                ('A', None, "b.txt".to_owned()),
            ]
        );
        let counts = parse_numstat("3\t1\tsrc/a.rs\x002\t0\t\0old.rs\0new.rs\0-\t-\timg.png\0");
        assert_eq!(counts["src/a.rs"], (3, 1, false));
        assert_eq!(counts["new.rs"], (2, 0, false));
        assert_eq!(counts["img.png"], (0, 0, true));
    }

    #[test]
    fn first_hunk_line_reads_the_new_side_start() {
        assert_eq!(
            parse_first_hunk_line("diff --git a/x b/x\n@@ -3,0 +4,2 @@ fn x\n+a\n"),
            Some(4)
        );
        assert_eq!(parse_first_hunk_line("@@ -1 +0,0 @@\n-gone\n"), Some(1));
        assert_eq!(parse_first_hunk_line("no hunks"), None);
    }

    #[test]
    fn base_branch_prefers_origin_head_then_known_names() {
        let repo = TempRepo::new("base");
        repo.write("a.txt", "one\n");
        repo.commit("init");
        assert_eq!(base_branch(&repo.0).as_deref(), Some("main"));

        repo.git(&["branch", "develop"]);
        assert_eq!(base_branch(&repo.0).as_deref(), Some("develop"));

        let head = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["update-ref", "refs/remotes/origin/trunk", &head]);
        repo.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ]);
        assert_eq!(base_branch(&repo.0).as_deref(), Some("origin/trunk"));
    }

    #[test]
    fn pr_mode_diffs_against_merge_base_including_uncommitted_and_untracked() {
        let repo = TempRepo::new("pr");
        repo.write("keep.txt", "k\n");
        repo.write("old.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        repo.write("gone.txt", "bye\n");
        repo.commit("init");
        repo.git(&["checkout", "-q", "-b", "task/x"]);
        repo.write("feature.rs", "fn a() {}\nfn b() {}\n");
        repo.git(&["mv", "old.txt", "renamed.txt"]);
        repo.commit("feature");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("main-only.txt", "m\n");
        repo.commit("main moves on");
        repo.git(&["checkout", "-q", "task/x"]);
        repo.write("keep.txt", "k\nmore\n");
        std::fs::remove_file(repo.0.join("gone.txt")).unwrap();
        repo.write("notes/new.md", "a\nb\nc");

        let pr = load_repo(&repo.0, Mode::Pr);
        assert_eq!(pr.error, None);
        assert_eq!(pr.branch.as_deref(), Some("task/x"));
        assert_eq!(pr.base_label, "main");
        let summary: Vec<(char, &str, u64, u64)> = pr
            .files
            .iter()
            .map(|file| (file.status, file.path.as_str(), file.adds, file.dels))
            .collect();
        assert_eq!(
            summary,
            [
                ('A', "feature.rs", 2, 0),
                ('D', "gone.txt", 0, 1),
                ('M', "keep.txt", 1, 0),
                ('?', "notes/new.md", 3, 0),
                ('R', "renamed.txt", 0, 0),
            ]
        );
        assert_eq!(pr.files[4].old_path.as_deref(), Some("old.txt"));
        assert_eq!((pr.adds(), pr.dels()), (6, 1));

        let uncommitted = load_repo(&repo.0, Mode::Uncommitted);
        assert_eq!(uncommitted.base_label, "HEAD");
        let paths: Vec<&str> = uncommitted
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, ["gone.txt", "keep.txt", "notes/new.md"]);

        let keep = &uncommitted.files[1];
        assert_eq!(first_changed_line(&repo.0, &uncommitted.base_rev, keep), 2);
        let plain = file_diff(&repo.0, &uncommitted.base_rev, keep, 80, false).unwrap();
        assert!(String::from_utf8_lossy(&plain).contains("more"));
        let untracked = file_diff(&repo.0, "HEAD", &uncommitted.files[2], 80, false).unwrap();
        assert!(String::from_utf8_lossy(&untracked).contains("new.md"));
    }

    #[test]
    fn repo_without_commits_diffs_against_the_empty_tree() {
        let repo = TempRepo::new("empty");
        repo.write("a.txt", "x\n");
        repo.git(&["add", "a.txt"]);

        let loaded = load_repo(&repo.0, Mode::Pr);

        assert_eq!(loaded.error, None);
        assert_eq!(loaded.base_rev, EMPTY_TREE);
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].status, 'A');
    }

    #[test]
    fn mode_toggles_between_pr_and_uncommitted() {
        assert_eq!(Mode::default(), Mode::Pr);
        assert_eq!(Mode::Pr.toggled(), Mode::Uncommitted);
        assert_eq!(Mode::Uncommitted.toggled(), Mode::Pr);
    }
}
