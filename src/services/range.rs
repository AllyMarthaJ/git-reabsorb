use crate::git::{GitError, GitOps};

/// Inclusive/exclusive commit range (base is exclusive, head inclusive).
#[derive(Clone, Debug)]
pub struct CommitRange {
    pub base: String,
    pub head: String,
}

impl CommitRange {
    pub fn new(base: String, head: String) -> Self {
        Self { base, head }
    }
}

/// Resolves CLI range flags into a concrete pair of commits.
pub struct RangeResolver<'a, G: GitOps> {
    git: &'a G,
}

impl<'a, G: GitOps> RangeResolver<'a, G> {
    pub fn new(git: &'a G) -> Self {
        Self { git }
    }

    pub fn resolve(
        &self,
        range: Option<&str>,
        base_branch: Option<&str>,
    ) -> Result<CommitRange, GitError> {
        match (range, base_branch) {
            (Some(r), None) => {
                let parts: Vec<&str> = r.split("..").collect();
                if parts.len() != 2 {
                    return Err(GitError::CommandFailed(format!(
                        "Invalid range: {}. Expected 'base..head'",
                        r
                    )));
                }
                Ok(CommitRange::new(
                    self.git.resolve_ref(parts[0])?,
                    self.git.resolve_ref(parts[1])?,
                ))
            }
            (None, Some(branch)) => Ok(CommitRange::new(
                self.git.resolve_ref(branch)?,
                self.git.get_head()?,
            )),
            (None, None) => Ok(CommitRange::new(
                self.git.find_branch_base()?,
                self.git.get_head()?,
            )),
            (Some(_), Some(_)) => Err(GitError::CommandFailed(
                "Cannot specify both range and --base".to_string(),
            )),
        }
    }
}
