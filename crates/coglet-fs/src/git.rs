use crate::Snapshot;
use std::{
    collections::BTreeSet,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitPoint {
    BlobPublished,
    TreePublished,
    CommitPublished,
    RefPublished,
}
/// Logs-only projection into a caller-initialized private Git repository. Never
/// writes `/data` (even as unreachable objects), a working tree or the index.
/// Git object IDs are NOT ABI SHA256 tree hashes. Requires Git with core.fsync.
pub struct GitStore {
    repo: PathBuf,
}
impl GitStore {
    pub fn new(repo: impl AsRef<Path>) -> Self {
        Self {
            repo: repo.as_ref().to_path_buf(),
        }
    }
    fn run(&self, args: &[&str], input: &[u8]) -> io::Result<String> {
        let mut cmd = Command::new("git");
        // Ambient repository/index/object routing must not redirect publication.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(name);
            }
        }
        let mut child = cmd
            .arg("-C")
            .arg(&self.repo)
            .args([
                "-c",
                "core.fsync=all",
                "-c",
                "core.fsyncMethod=fsync",
                "-c",
                "commit.gpgSign=false",
            ])
            .args(args)
            .env("GIT_AUTHOR_NAME", "Coglet")
            .env("GIT_AUTHOR_EMAIL", "coglet@localhost")
            .env("GIT_COMMITTER_NAME", "Coglet")
            .env("GIT_COMMITTER_EMAIL", "coglet@localhost")
            .env("GIT_AUTHOR_DATE", "@0 +0000")
            .env("GIT_COMMITTER_DATE", "@0 +0000")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let written = child.stdin.take().unwrap().write_all(input);
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        written?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }
    fn ensure_repository_root(&self) -> io::Result<()> {
        let root = self.repo.canonicalize()?;
        let git_dir =
            PathBuf::from(self.run(&["rev-parse", "--absolute-git-dir"], &[])?).canonicalize()?;
        if git_dir == root {
            return Ok(());
        }
        let top =
            PathBuf::from(self.run(&["rev-parse", "--show-toplevel"], &[])?).canonicalize()?;
        if top != root {
            return Err(io::Error::other("GitStore path is not the repository root"));
        }
        Ok(())
    }
    pub fn logs_head(&self) -> io::Result<Option<String>> {
        self.ensure_repository_root()?;
        // show-ref --verify returns 1 only for absent ref, but use for-each-ref
        // to retain genuine repository/permission errors.
        let s = self.run(
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname) %(symref)",
                "refs/heads/logs",
            ],
            &[],
        )?;
        let exact = s
            .lines()
            .find_map(|line| line.strip_prefix("refs/heads/logs "))
            .map(str::trim);
        if exact.is_none() {
            Ok(None)
        } else if exact.is_some_and(valid_oid) {
            Ok(exact.map(str::to_owned))
        } else {
            Err(io::Error::other("invalid logs ref"))
        }
    }
    pub fn flush_logs(&self, snapshot: &Snapshot, expected: Option<&str>) -> io::Result<String> {
        self.flush_logs_with(snapshot, expected, |_| Ok(()))
    }
    /// Parent/CAS is the exact previous logs commit. Git handles atomic ref
    /// publication; errors after RefPublished require reading back logs_head.
    pub fn flush_logs_with(
        &self,
        snapshot: &Snapshot,
        expected: Option<&str>,
        mut hook: impl FnMut(GitPoint) -> io::Result<()>,
    ) -> io::Result<String> {
        if expected.is_some_and(|s| !valid_oid(s)) {
            return Err(io::Error::other("invalid parent id"));
        }
        if self.logs_head()?.as_deref() != expected {
            return Err(io::Error::other("logs checkpoint conflict"));
        }
        let files: Vec<_> = snapshot
            .files()
            .filter_map(|(p, b)| p.strip_prefix("/logs/").map(|p| (p, b)))
            .collect();
        // Keep the ABI namespace unchanged. Never silently drop a valid guest
        // file or publish a tree which Git cannot safely transfer/check out.
        if files
            .iter()
            .any(|(p, _)| p.split('/').any(|part| part.eq_ignore_ascii_case(".git")))
        {
            return Err(io::Error::other(
                "log path cannot be safely projected into Git",
            ));
        }
        let tree = self.tree(&files, &mut hook)?;
        let mut args = vec!["commit-tree", &tree];
        if let Some(parent) = expected {
            args.extend(["-p", parent]);
        }
        let commit = self.run(&args, b"Coglet logs checkpoint\n")?;
        hook(GitPoint::CommitPublished)?;
        let zeros = "0".repeat(commit.len());
        self.run(
            &[
                "update-ref",
                "--no-deref",
                "refs/heads/logs",
                &commit,
                expected.unwrap_or(&zeros),
            ],
            &[],
        )?;
        hook(GitPoint::RefPublished)?;
        Ok(commit)
    }
    fn tree(
        &self,
        files: &[(&str, &[u8])],
        hook: &mut impl FnMut(GitPoint) -> io::Result<()>,
    ) -> io::Result<String> {
        let mut input = Vec::new();
        let names: BTreeSet<_> = files
            .iter()
            .map(|(p, _)| p.split('/').next().unwrap())
            .collect();
        for name in names {
            if let Some((_, bytes)) = files.iter().find(|(p, _)| *p == name) {
                let oid = self.run(&["hash-object", "-w", "--stdin"], bytes)?;
                hook(GitPoint::BlobPublished)?;
                input.extend_from_slice(format!("100644 blob {oid}\t{name}\0").as_bytes());
            } else {
                let prefix = format!("{name}/");
                let children: Vec<_> = files
                    .iter()
                    .filter_map(|(p, b)| p.strip_prefix(&prefix).map(|p| (p, *b)))
                    .collect();
                let oid = self.tree(&children, hook)?;
                input.extend_from_slice(format!("040000 tree {oid}\t{name}\0").as_bytes());
            }
        }
        let oid = self.run(&["mktree", "-z"], &input)?;
        hook(GitPoint::TreePublished)?;
        Ok(oid)
    }
}
fn valid_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
