use log::trace;
use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RangeDiff {
    pub from: CommitHash,
    pub to: CommitHash,
    pub diff: String,
}

impl Extractor {
    pub async fn range_diff(
        &self,
        from_revision: &str,
        to_revision: &str,
    ) -> Result<RangeDiff, ExtractError> {
        trace!("extract range diff: {from_revision}..{to_revision}");
        let from = CommitHash::resolve(from_revision, &self.project_root).await?;
        let to = CommitHash::resolve(to_revision, &self.project_root).await?;
        let diff = run_git(
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                from.as_ref(),
                to.as_ref(),
                "--",
            ],
            &self.project_root,
        )
        .await?;

        Ok(RangeDiff { from, to, diff })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn compares_the_range_endpoints() {
        let directory = tempfile::tempdir().unwrap();
        run_git(&["init"], directory.path()).await.unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            directory.path(),
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], directory.path())
            .await
            .unwrap();
        std::fs::write(directory.path().join("file.txt"), "before\n").unwrap();
        run_git(&["add", "file.txt"], directory.path())
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "before"],
            directory.path(),
        )
        .await
        .unwrap();
        let from = CommitHash::resolve("HEAD", directory.path()).await.unwrap();
        std::fs::write(directory.path().join("file.txt"), "after\n").unwrap();
        run_git(
            &["commit", "-am", "after", "--no-gpg-sign"],
            directory.path(),
        )
        .await
        .unwrap();
        let to = CommitHash::resolve("HEAD", directory.path()).await.unwrap();

        let result = Extractor::new(directory.path().to_path_buf())
            .range_diff(from.as_ref(), "HEAD")
            .await
            .unwrap();

        assert_eq!(result.from, from);
        assert_eq!(result.to, to);
        assert!(result.diff.contains("-before"));
        assert!(result.diff.contains("+after"));
    }

    #[tokio::test]
    async fn disables_text_conversion() {
        let directory = tempfile::tempdir().unwrap();
        run_git(&["init"], directory.path()).await.unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            directory.path(),
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], directory.path())
            .await
            .unwrap();
        run_git(
            &["config", "diff.marker.textconv", "sed s/before/CONVERTED/"],
            directory.path(),
        )
        .await
        .unwrap();
        std::fs::write(
            directory.path().join(".gitattributes"),
            "*.txt diff=marker\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("file.txt"), "before\n").unwrap();
        run_git(&["add", ".gitattributes", "file.txt"], directory.path())
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "before"],
            directory.path(),
        )
        .await
        .unwrap();
        let from = CommitHash::resolve("HEAD", directory.path()).await.unwrap();
        std::fs::write(directory.path().join("file.txt"), "after\n").unwrap();
        run_git(
            &["commit", "-am", "after", "--no-gpg-sign"],
            directory.path(),
        )
        .await
        .unwrap();

        let result = Extractor::new(directory.path().to_path_buf())
            .range_diff(from.as_ref(), "HEAD")
            .await
            .unwrap();

        assert!(!result.diff.contains("CONVERTED"));
        assert!(result.diff.contains("-before"));
        assert!(result.diff.contains("+after"));
    }
}
