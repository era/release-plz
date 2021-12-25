use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{anyhow, Context};
use tracing::{debug, instrument, Span};

/// Repository
pub struct Repo {
    /// Repository root directory
    directory: PathBuf,
    current_branch: String,
}

impl Repo {
    /// Returns an error if the directory doesn't contain any commit
    pub fn new(directory: impl AsRef<Path>) -> anyhow::Result<Self> {
        let current_branch = Self::get_current_branch(&directory)?;
        // TODO move this in main
        crate::log::init();

        Ok(Self {
            directory: directory.as_ref().to_path_buf(),
            current_branch,
        })
    }

    fn get_current_branch(directory: impl AsRef<Path>) -> anyhow::Result<String> {
        let current_branch =
            Self::git_in_dir(directory.as_ref(), &["rev-parse", "--abbrev-ref", "HEAD"])?;
        stdout(current_branch).map_err(|e|
            if e.to_string().contains("fatal: ambiguous argument 'HEAD': unknown revision or path not in the working tree.") {
                anyhow!("git repository does not contain any commit.")
            }
            else {
                e
            }
        )
    }

    pub fn checkout_head(&self) -> anyhow::Result<()> {
        self.git(&["checkout", &self.current_branch])?;
        Ok(())
    }

    #[instrument(skip(self))]
    fn current_commit(&self) -> anyhow::Result<String> {
        self.nth_commit(1)
    }

    #[instrument(skip(self))]
    fn previous_commit(&self) -> anyhow::Result<String> {
        self.nth_commit(2)
    }

    #[instrument(
        skip(self)
        fields(
            nth_commit = tracing::field::Empty,
        )
    )]
    fn nth_commit(&self, nth: usize) -> anyhow::Result<String> {
        let nth = nth.to_string();
        let output = self.git(&["--format=\"%H\"", "-n", &nth])?;
        let commit_list = stdout(output)?;
        let last_commit = commit_list
            .lines()
            .last()
            .context("repository has no commits")?;
        Span::current().record("nth_commit", &last_commit);

        Ok(last_commit.to_string())
    }

    fn git_in_dir(dir: &Path, args: &[&str]) -> io::Result<Output> {
        Command::new("git").arg("-C").arg(dir).args(args).output()
    }

    /// Run a git command in the repository git directory
    fn git(&self, args: &[&str]) -> io::Result<Output> {
        Self::git_in_dir(&self.directory, args)
    }

    /// Checkout to the latest commit. I.e. go back in history of 1 commit.
    pub fn checkout_last_commit(&self) -> anyhow::Result<()> {
        let previous_commit = self.previous_commit()?;
        self.checkout(&previous_commit)?;
        Ok(())
    }

    /// Return the list of edited files of that commit. Absolute Path.
    pub fn edited_file_in_current_commit(&self) -> anyhow::Result<Vec<PathBuf>> {
        let commit = &self.current_commit()?;
        let output = self.git(&["diff-tree", "--no-commit-id", "--name-only", "-r", commit])?;
        let files = stdout(output)?;
        let files: Result<Vec<PathBuf>, io::Error> = files.lines().map(fs::canonicalize).collect();
        Ok(files?)
    }

    fn previous_commit_at_path(&self, path: &Path) -> anyhow::Result<String> {
        self.nth_commit_at_path(2, path)
    }

    pub fn checkout_previous_commit_at_path(&self, path: &Path) -> anyhow::Result<()> {
        let commit = self.previous_commit_at_path(path)?;
        self.checkout(&commit)?;
        Ok(())
    }

    #[instrument(skip(self))]
    fn checkout(&self, object: &str) -> io::Result<()> {
        let output = self.git(&["checkout", object])?;
        debug!("git checkout outcome: {:?}", output);
        Ok(())
    }

    #[instrument(
        skip(self)
        fields(
            nth_commit = tracing::field::Empty,
        )
    )]
    fn nth_commit_at_path(
        &self,
        nth: usize,
        path: impl AsRef<Path> + fmt::Debug,
    ) -> anyhow::Result<String> {
        let nth = nth.to_string();
        let path = path.as_ref().to_str().ok_or(anyhow!("invalid path"))?;
        let output = self.git(&["log", "--format=%H", "-n", &nth, path])?;
        let commit_list = stdout(output)?;
        let last_commit = commit_list
            .lines()
            .last()
            .context("repository has no commits")?;

        Span::current().record("nth_commit", &last_commit);
        debug!("nth_commit found");
        Ok(last_commit.to_string())
    }

    pub fn current_commit_message(&self) -> anyhow::Result<String> {
        let output = self.git(&["log", "-1", "--pretty=format:%s"])?;
        stdout(output)
    }
}

fn stdout(output: Output) -> anyhow::Result<String> {
    debug!("output: {:?}", output);
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8(output.stderr)?;
        return Err(anyhow!(stderr));
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    impl Repo {
        fn git_add(&self) {
            self.git(&["add", "."]).unwrap();
        }

        fn git_commit(&self, message: &str) {
            self.git(&["commit", "-m", message]).unwrap();
        }

        fn git_add_and_commit(&self, message: &str) {
            self.git_add();
            self.git_commit(message);
        }

        fn init(directory: impl AsRef<Path>) -> Self {
            Self::git_in_dir(directory.as_ref(), &["init"]).unwrap();
            fs::write(directory.as_ref().join("README.md"), "# my awesome project").unwrap();
            Self::git_in_dir(directory.as_ref(), &["add", "."]).unwrap();
            Self::git_in_dir(directory.as_ref(), &["commit", "-m", "add README"]).unwrap();
            Self::new(directory).unwrap()
        }
    }

    #[test]
    fn inexistent_previous_commit_detected() {
        let repository_dir = tempdir().unwrap();
        let repo = Repo::init(&repository_dir);
        let file1 = repository_dir.as_ref().join("file1.txt");
        repo.checkout_previous_commit_at_path(&file1).unwrap_err();
    }

    #[test]
    fn previous_commit_is_retrieved() {
        let repository_dir = tempdir().unwrap();
        let repo = Repo::init(&repository_dir);
        let file1 = repository_dir.as_ref().join("file1.txt");
        let file2 = repository_dir.as_ref().join("file2.txt");
        {
            fs::write(&file2, b"Hello, file2!-1").unwrap();
            repo.git_add_and_commit("file2-1");
            fs::write(&file1, b"Hello, file1!").unwrap();
            repo.git_add_and_commit("file1");
            fs::write(&file2, b"Hello, file2!-2").unwrap();
            repo.git_add_and_commit("file2-2");
        }
        repo.checkout_previous_commit_at_path(&file2).unwrap();
        assert_eq!(repo.current_commit_message().unwrap(), "file2-1");
    }

    #[test]
    fn current_commit_is_retrieved() {
        let repository_dir = tempdir().unwrap();
        let repo = Repo::init(&repository_dir);
        let file1 = repository_dir.as_ref().join("file1.txt");
        let commit_message = "file1 message";
        {
            fs::write(&file1, b"Hello, file1!").unwrap();
            repo.git_add_and_commit(commit_message);
        }
        assert_eq!(repo.current_commit_message().unwrap(), commit_message);
    }
}