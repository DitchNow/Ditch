use chrono::Utc;
use ditch_core::{
    AgentId, ChangeIntent, ConflictState, IntegrationState, ManagedWorktree, OverlapRisk, Project,
    ProjectGitOperation, ProjectGitOperationId, ProjectGitOperationState, ValidationCheck,
    ValidationState, WorktreeFileChange, WorktreeId, WorktreeLockState, WorktreeReview,
    WorktreeStatus,
};
use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DITCH_BRANCH_PREFIX: &str = "ditch/agent/";
const CHECKPOINT_PREFIX: &str = "refs/ditch/checkpoints/";
const CANDIDATE_PREFIX: &str = "refs/ditch/candidates/";

#[derive(Debug)]
pub struct GitError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for GitError {}

impl From<io::Error> for GitError {
    fn from(value: io::Error) -> Self {
        Self {
            code: "git_io_failed",
            message: value.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreatedWorktree {
    pub managed: ManagedWorktree,
    pub agent_cwd: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedWorktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitialSnapshotPreview {
    pub repository_root: PathBuf,
    pub target_branch: String,
    pub included_paths: Vec<String>,
    pub warnings: Vec<String>,
    pub tree_oid: Option<String>,
    pub target_oid: Option<String>,
    pub ready: bool,
    pub unsafe_to_snapshot: bool,
}

#[derive(Default)]
pub struct GitCoordinator {
    locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl GitCoordinator {
    fn project_lock(&self, project: &Project) -> Result<Arc<Mutex<()>>, GitError> {
        let repository = repository_root(&project.root)?;
        Ok(self
            .locks
            .lock()
            .expect("Git coordinator lock map should not be poisoned")
            .entry(repository)
            .or_default()
            .clone())
    }

    pub fn inspect_initial_snapshot(
        &self,
        project: &Project,
    ) -> Result<InitialSnapshotPreview, GitError> {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        inspect_initial_snapshot_unlocked(project)
    }

    pub fn create_initial_snapshot<F>(
        &self,
        project: &Project,
        expected_tree_oid: &str,
        mut journal: F,
    ) -> Result<String, GitError>
    where
        F: FnMut(&ProjectGitOperation) -> Result<(), GitError>,
    {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let preview = inspect_initial_snapshot_unlocked(project)?;
        if preview.ready {
            return rev_parse(&preview.repository_root, "HEAD");
        }
        if preview.unsafe_to_snapshot {
            return Err(GitError {
                code: "initial_snapshot_unsafe",
                message: preview.warnings.join(" "),
            });
        }
        let actual_tree = preview.tree_oid.as_deref().ok_or_else(|| GitError {
            code: "initial_snapshot_missing",
            message: "The proposed starting snapshot could not be built".into(),
        })?;
        if actual_tree != expected_tree_oid {
            return Err(GitError {
                code: "initial_snapshot_changed",
                message: "Project files changed while the starting snapshot was being reviewed. Review the refreshed snapshot and try again.".into(),
            });
        }
        let now = Utc::now();
        let mut operation = ProjectGitOperation {
            id: ProjectGitOperationId::new(),
            project_id: project.id,
            kind: "initial_snapshot".into(),
            target_branch: preview.target_branch.clone(),
            expected_tree_oid: actual_tree.to_owned(),
            expected_old_oid: preview.target_oid.clone(),
            created_commit_oid: None,
            state: ProjectGitOperationState::Preparing,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        journal(&operation)?;
        let parents = preview
            .target_oid
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let message = if preview.target_oid.is_some() {
            "Save current project state (The Ditch)"
        } else {
            "Initial project snapshot (The Ditch)"
        };
        let commit = commit_tree(&preview.repository_root, actual_tree, &parents, message)?;
        operation.created_commit_oid = Some(commit.clone());
        operation.state = ProjectGitOperationState::CommitCreated;
        operation.updated_at = Utc::now();
        journal(&operation)?;
        let reference = format!("refs/heads/{}", preview.target_branch);
        let expected_old = preview
            .target_oid
            .as_deref()
            .unwrap_or("0000000000000000000000000000000000000000");
        git_ok(
            &preview.repository_root,
            ["update-ref", &reference, &commit, expected_old],
            None,
        )?;
        operation.state = ProjectGitOperationState::RefUpdated;
        operation.updated_at = Utc::now();
        journal(&operation)?;
        // The user approved committing the complete current snapshot. Align only
        // the index with that tree; never update or overwrite working files.
        git_ok(
            &preview.repository_root,
            ["read-tree", "--reset", &commit],
            None,
        )?;
        operation.state = ProjectGitOperationState::Completed;
        operation.updated_at = Utc::now();
        journal(&operation)?;
        Ok(commit)
    }

    pub fn reconcile_initial_snapshot(
        &self,
        project: &Project,
        operation: &mut ProjectGitOperation,
    ) -> Result<(), GitError> {
        if operation.kind != "initial_snapshot"
            || operation.state == ProjectGitOperationState::Completed
        {
            return Ok(());
        }
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        let reference = format!("refs/heads/{}", operation.target_branch);
        let current = rev_parse(&repo_root, &reference).ok();
        match (current.as_deref(), operation.created_commit_oid.as_deref()) {
            (Some(current), Some(created)) if current == created => {
                let actual_tree = rev_parse(&repo_root, &format!("{created}^{{tree}}"))?;
                if actual_tree != operation.expected_tree_oid {
                    operation.state = ProjectGitOperationState::RecoveryNeeded;
                    operation.last_error =
                        Some("The recorded starting snapshot does not match its Git commit".into());
                    operation.updated_at = Utc::now();
                    return Ok(());
                }
                git_ok(&repo_root, ["read-tree", "--reset", created], None)?;
                operation.state = ProjectGitOperationState::Completed;
                operation.last_error = None;
            }
            (None, Some(created)) => {
                if operation.expected_old_oid.is_some() {
                    operation.state = ProjectGitOperationState::RecoveryNeeded;
                    operation.last_error = Some(
                        "The project target disappeared while Ditch was saving its state".into(),
                    );
                    operation.updated_at = Utc::now();
                    return Ok(());
                }
                git_ok(
                    &repo_root,
                    [
                        "update-ref",
                        &reference,
                        created,
                        "0000000000000000000000000000000000000000",
                    ],
                    None,
                )?;
                git_ok(&repo_root, ["read-tree", "--reset", created], None)?;
                operation.state = ProjectGitOperationState::Completed;
                operation.last_error = None;
            }
            (Some(current), Some(created))
                if operation.expected_old_oid.as_deref() == Some(current) =>
            {
                let actual_tree = rev_parse(&repo_root, &format!("{created}^{{tree}}"))?;
                if actual_tree != operation.expected_tree_oid {
                    operation.state = ProjectGitOperationState::RecoveryNeeded;
                    operation.last_error =
                        Some("The recorded project save point has the wrong tree".into());
                    operation.updated_at = Utc::now();
                    return Ok(());
                }
                git_ok(
                    &repo_root,
                    ["update-ref", &reference, created, current],
                    None,
                )?;
                git_ok(&repo_root, ["read-tree", "--reset", created], None)?;
                operation.state = ProjectGitOperationState::Completed;
                operation.last_error = None;
            }
            (Some(_), _) => {
                operation.state = ProjectGitOperationState::RecoveryNeeded;
                operation.last_error = Some(
                    "Another Git operation changed the project target; Ditch left it untouched."
                        .into(),
                );
            }
            (None, None) => {
                operation.state = ProjectGitOperationState::Failed;
                operation.last_error = Some(
                    "Starting snapshot preparation stopped before a commit was created".into(),
                );
            }
        }
        operation.updated_at = Utc::now();
        Ok(())
    }

    #[cfg(test)]
    pub fn create(
        &self,
        project: &Project,
        agent_id: AgentId,
        prompt: &str,
        worktrees_root: &Path,
    ) -> Result<CreatedWorktree, GitError> {
        self.create_journaled(project, agent_id, prompt, worktrees_root, |_| Ok(()))
    }

    pub fn create_journaled<F>(
        &self,
        project: &Project,
        agent_id: AgentId,
        prompt: &str,
        worktrees_root: &Path,
        mut journal: F,
    ) -> Result<CreatedWorktree, GitError>
    where
        F: FnMut(&ManagedWorktree) -> Result<(), GitError>,
    {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        let launch_preview = inspect_initial_snapshot_unlocked(project)?;
        if !launch_preview.ready && launch_preview.target_oid.is_some() {
            return Err(GitError {
                code: "project_snapshot_required",
                message: "Ditch is still preparing the current project files. Try starting the agent again.".into(),
            });
        }
        let relative_cwd = project
            .root
            .canonicalize()
            .unwrap_or_else(|_| project.root.clone())
            .strip_prefix(&repo_root)
            .map(Path::to_path_buf)
            .map_err(|_| GitError {
                code: "project_repository_mismatch",
                message: "The project path is not contained by its Git repository".to_owned(),
            })?;
        let target_branch = symbolic_head(&repo_root)?;
        let base_oid = ensure_head(&repo_root, &target_branch)?;
        let short_id = agent_id.0.simple().to_string()[..12].to_owned();
        let slug = sanitize_slug(prompt);
        let branch_name = format!("{DITCH_BRANCH_PREFIX}{}/{slug}", agent_id.0);
        validate_ditch_branch(&branch_name)?;

        let project_dir = worktrees_root.join(project.id.0.simple().to_string());
        fs::create_dir_all(&project_dir)?;
        let path = project_dir.join(format!("{short_id}-{slug}"));
        if path.exists() {
            return Err(GitError {
                code: "worktree_path_exists",
                message: format!("Managed worktree path already exists: {}", path.display()),
            });
        }
        let reason = format!("The Ditch session {}", agent_id.0);
        let now = Utc::now();
        let agent_cwd = path.join(relative_cwd);
        let mut managed = ManagedWorktree {
            id: WorktreeId::new(),
            project_id: project.id,
            session_id: agent_id,
            path: path.clone(),
            agent_cwd: agent_cwd.clone(),
            branch_name: branch_name.clone(),
            base_branch: target_branch.clone(),
            base_commit_oid: base_oid.clone(),
            target_branch: target_branch.clone(),
            target_oid_at_start: base_oid.clone(),
            head_oid: base_oid.clone(),
            checkpoint_oid: None,
            candidate_oid: None,
            candidate_target_oid: None,
            candidate_checkpoint_oid: None,
            resolution_path: None,
            resolution_branch: None,
            resolution_target_oid: None,
            status: WorktreeStatus::Creating,
            lock_state: WorktreeLockState::Unknown,
            dirty: false,
            changed_paths: Vec::new(),
            intent: Some(infer_change_intent(prompt)),
            overlap_override: false,
            overlapping_session_ids: Vec::new(),
            overlapping_paths: Vec::new(),
            overlap_risk: OverlapRisk::None,
            conflict_state: ConflictState::None,
            integration_state: IntegrationState::NotRequested,
            validation_state: ValidationState::NotRun,
            validation_checks: Vec::new(),
            operation: Some("create".into()),
            last_error: None,
            created_at: now,
            updated_at: now,
            last_reconciled_at: None,
        };
        journal(&managed)?;
        git_ok(
            &repo_root,
            [
                OsString::from("worktree"),
                OsString::from("add"),
                OsString::from("--lock"),
                OsString::from("--reason"),
                OsString::from(reason),
                OsString::from("-b"),
                OsString::from(&branch_name),
                path.as_os_str().to_owned(),
                OsString::from(&base_oid),
            ],
            None,
        )?;
        let checked_out_head = rev_parse(&path, "HEAD")?;
        let expected_tree = rev_parse(&repo_root, &format!("{base_oid}^{{tree}}"))?;
        let checked_out_tree = rev_parse(&path, "HEAD^{tree}")?;
        if checked_out_head != base_oid || checked_out_tree != expected_tree {
            return Err(GitError {
                code: "worktree_materialization_failed",
                message:
                    "Ditch could not verify the prepared project workspace. No agent was started."
                        .into(),
            });
        }
        fs::create_dir_all(&agent_cwd)?;
        managed.status = WorktreeStatus::Ready;
        managed.lock_state = WorktreeLockState::Locked;
        managed.operation = None;
        managed.updated_at = Utc::now();
        managed.last_reconciled_at = Some(managed.updated_at);
        Ok(CreatedWorktree { agent_cwd, managed })
    }

    pub fn refresh(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
    ) -> Result<(), GitError> {
        if !worktree.path.is_dir() {
            worktree.status = WorktreeStatus::RecoveryNeeded;
            worktree.lock_state = WorktreeLockState::Missing;
            worktree.last_error =
                Some("The managed workspace directory was removed outside Ditch".into());
            worktree.last_reconciled_at = Some(Utc::now());
            worktree.updated_at = Utc::now();
            return Ok(());
        }
        let repo_root = repository_root(&project.root)?;
        let listed = parse_worktree_porcelain(&git_bytes(
            &repo_root,
            ["worktree", "list", "--porcelain", "-z"],
            None,
        )?)?;
        let expected = canonical_or_original(&worktree.path);
        let actual = listed
            .iter()
            .find(|entry| canonical_or_original(&entry.path) == expected);
        let Some(actual) = actual else {
            worktree.status = WorktreeStatus::RecoveryNeeded;
            worktree.lock_state = WorktreeLockState::Missing;
            worktree.last_error = Some("The managed worktree is missing from Git metadata".into());
            worktree.last_reconciled_at = Some(Utc::now());
            worktree.updated_at = Utc::now();
            return Ok(());
        };
        let expected_branch = format!("refs/heads/{}", worktree.branch_name);
        if actual.branch.as_deref() != Some(expected_branch.as_str()) {
            worktree.status = WorktreeStatus::RecoveryNeeded;
            worktree.last_error = Some("The managed path now points at a different branch".into());
            return Ok(());
        }
        worktree.lock_state = if actual.locked {
            WorktreeLockState::Locked
        } else {
            WorktreeLockState::Unlocked
        };
        worktree.head_oid = rev_parse(&worktree.path, "HEAD")?;
        worktree.changed_paths = changed_paths(&worktree.path)?;
        worktree.dirty = !worktree.changed_paths.is_empty();
        if matches!(worktree.status, WorktreeStatus::Creating) {
            worktree.operation = None;
            worktree.status = if worktree.dirty {
                WorktreeStatus::Dirty
            } else {
                WorktreeStatus::Ready
            };
        } else if matches!(
            worktree.status,
            WorktreeStatus::Ready | WorktreeStatus::Active | WorktreeStatus::Dirty
        ) {
            worktree.status = if worktree.dirty {
                WorktreeStatus::Dirty
            } else {
                WorktreeStatus::Active
            };
        }
        worktree.last_reconciled_at = Some(Utc::now());
        worktree.updated_at = Utc::now();
        Ok(())
    }

    pub fn reconcile_operation(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
    ) -> Result<(), GitError> {
        self.refresh(project, worktree)?;
        match worktree.operation.as_deref() {
            Some("apply_integration") => {
                let repo_root = repository_root(&project.root)?;
                let target = rev_parse(
                    &repo_root,
                    &format!("refs/heads/{}", worktree.target_branch),
                )?;
                if worktree.candidate_oid.as_deref() == Some(target.as_str()) {
                    worktree.status = WorktreeStatus::Integrated;
                    worktree.integration_state = IntegrationState::Applied;
                    worktree.operation = None;
                } else if worktree.candidate_target_oid.as_deref() == Some(target.as_str()) {
                    worktree.status = WorktreeStatus::ReadyToApply;
                    worktree.integration_state = IntegrationState::Ready;
                    worktree.operation = None;
                } else {
                    worktree.status = WorktreeStatus::NeedsReview;
                    worktree.integration_state = IntegrationState::Queued;
                    worktree.operation = None;
                    worktree.last_error = Some(
                        "The target moved during recovery; integration must be recomputed".into(),
                    );
                }
            }
            Some("prepare_integration") => {
                worktree.status = WorktreeStatus::NeedsReview;
                worktree.integration_state = IntegrationState::Queued;
                worktree.validation_state = ValidationState::NotRun;
                worktree.operation = None;
                worktree.last_error =
                    Some("Preparation was interrupted and must be run again".into());
            }
            Some("create_resolution") => {
                if let Some(path) = worktree.resolution_path.as_ref() {
                    verify_resolution_path(project, worktree, path)?;
                    if path.is_dir() {
                        let unmerged = git_bytes(path, ["ls-files", "-u", "-z"], None)?;
                        let merge_in_progress = git_output(
                            path,
                            ["rev-parse", "--quiet", "--verify", "MERGE_HEAD"],
                            None,
                        )?
                        .status
                        .success();
                        if !unmerged.is_empty() || merge_in_progress {
                            if worktree.resolution_target_oid.is_none() {
                                worktree.resolution_target_oid = Some(rev_parse(path, "HEAD")?);
                            }
                            worktree.status = WorktreeStatus::NeedsReview;
                            worktree.operation = None;
                        } else {
                            worktree.status = WorktreeStatus::RecoveryNeeded;
                            worktree.last_error = Some(
                                "Conflict resolution creation was interrupted before Git prepared the merge"
                                    .into(),
                            );
                        }
                    } else {
                        worktree.resolution_path = None;
                        worktree.resolution_branch = None;
                        worktree.resolution_target_oid = None;
                        worktree.operation = None;
                        worktree.status = WorktreeStatus::ConflictRisk;
                    }
                }
            }
            _ => {}
        }
        worktree.updated_at = Utc::now();
        Ok(())
    }

    pub fn checkpoint(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
    ) -> Result<String, GitError> {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        self.refresh(project, worktree)?;
        if matches!(worktree.status, WorktreeStatus::RecoveryNeeded) {
            return Err(GitError {
                code: "worktree_recovery_needed",
                message: worktree.last_error.clone().unwrap_or_default(),
            });
        }
        let temp_index = unique_temp_path("ditch-index");
        let index_env = [(
            OsString::from("GIT_INDEX_FILE"),
            temp_index.as_os_str().to_owned(),
        )];
        git_ok(&worktree.path, ["read-tree", "HEAD"], Some(&index_env))?;
        let add_result = git_ok(&worktree.path, ["add", "-A"], Some(&index_env));
        if let Err(error) = add_result {
            let _ = fs::remove_file(&temp_index);
            return Err(error);
        }
        let tree = git_text(&worktree.path, ["write-tree"], Some(&index_env))?;
        let parent = rev_parse(&worktree.path, "HEAD")?;
        let checkpoint = commit_tree(
            &worktree.path,
            &tree,
            &[parent.as_str()],
            &format!("Ditch checkpoint for session {}", worktree.session_id.0),
        )?;
        let checkpoint_ref = format!("{CHECKPOINT_PREFIX}{}", worktree.session_id.0);
        update_internal_ref(
            &worktree.path,
            &checkpoint_ref,
            &checkpoint,
            worktree.checkpoint_oid.as_deref(),
        )?;
        let _ = fs::remove_file(temp_index);
        worktree.checkpoint_oid = Some(checkpoint.clone());
        worktree.status = WorktreeStatus::Finished;
        worktree.integration_state = IntegrationState::NotRequested;
        worktree.updated_at = Utc::now();
        Ok(checkpoint)
    }

    pub fn prepare_integration(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
        validation_root: &Path,
    ) -> Result<(), GitError> {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        worktree.status = WorktreeStatus::CheckingMerge;
        worktree.integration_state = IntegrationState::Checking;
        worktree.validation_checks.clear();
        let checkpoint = worktree.checkpoint_oid.clone().ok_or_else(|| GitError {
            code: "checkpoint_missing",
            message: "The agent work has not been checkpointed".to_owned(),
        })?;
        let repo_root = repository_root(&project.root)?;
        let target_ref = format!("refs/heads/{}", worktree.target_branch);
        let target_oid = rev_parse(&repo_root, &target_ref)?;
        let merge = git_output(
            &repo_root,
            [
                "merge-tree",
                "--write-tree",
                "--messages",
                "--name-only",
                "-z",
                &target_oid,
                &checkpoint,
            ],
            None,
        )?;
        if !merge.status.success() {
            if merge.status.code() != Some(1) {
                return Err(GitError {
                    code: "merge_tree_failed",
                    message: output_detail(&merge),
                });
            }
            worktree.status = WorktreeStatus::ConflictRisk;
            worktree.conflict_state = ConflictState::GitConflict;
            worktree.integration_state = IntegrationState::Conflict;
            worktree.last_error = Some(output_detail(&merge));
            worktree.candidate_oid = None;
            worktree.candidate_target_oid = Some(target_oid);
            worktree.candidate_checkpoint_oid = None;
            worktree.updated_at = Utc::now();
            return Ok(());
        }
        let tree = first_oid(&merge.stdout).ok_or_else(|| GitError {
            code: "merge_tree_invalid",
            message: "Git did not return a merge result tree".to_owned(),
        })?;
        let candidate = commit_tree(
            &repo_root,
            &tree,
            &[target_oid.as_str(), checkpoint.as_str()],
            &format!(
                "Ditch integration candidate for session {}",
                worktree.session_id.0
            ),
        )?;
        let candidate_ref = format!("{CANDIDATE_PREFIX}{}", worktree.session_id.0);
        update_internal_ref(
            &repo_root,
            &candidate_ref,
            &candidate,
            worktree.candidate_oid.as_deref(),
        )?;
        worktree.candidate_oid = Some(candidate.clone());
        worktree.candidate_target_oid = Some(target_oid);
        worktree.candidate_checkpoint_oid = Some(checkpoint.clone());
        worktree.conflict_state = ConflictState::None;
        worktree.status = WorktreeStatus::Validating;
        worktree.integration_state = IntegrationState::Validating;
        worktree.validation_state = ValidationState::Running;

        let project_root = canonical_or_original(&project.root);
        let project_relative = project_root
            .strip_prefix(&repo_root)
            .unwrap_or(Path::new(""));
        let checks = validate_candidate(
            &repo_root,
            validation_root,
            worktree,
            &candidate,
            project_relative,
            &project_root,
        )?;
        set_validation_result(worktree, checks);
        worktree.updated_at = Utc::now();
        Ok(())
    }

    pub fn create_conflict_resolution(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
        resolution_root: &Path,
    ) -> Result<PathBuf, GitError> {
        if worktree.conflict_state != ConflictState::GitConflict {
            return Err(GitError {
                code: "resolution_not_needed",
                message: "This result does not currently have a Git conflict".into(),
            });
        }
        if let Some(path) = worktree.resolution_path.as_ref()
            && path.is_dir()
        {
            let unmerged = git_bytes(path, ["ls-files", "-u", "-z"], None)?;
            let merge_in_progress = git_output(
                path,
                ["rev-parse", "--quiet", "--verify", "MERGE_HEAD"],
                None,
            )?
            .status
            .success();
            if !unmerged.is_empty() || merge_in_progress {
                if worktree.resolution_target_oid.is_none() {
                    worktree.resolution_target_oid = Some(rev_parse(path, "HEAD")?);
                }
                return Ok(path.clone());
            }
            return Err(GitError {
                code: "resolution_recovery_needed",
                message: "The resolution workspace exists without an active merge. Its files were preserved for recovery.".into(),
            });
        }
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        let checkpoint = worktree.checkpoint_oid.as_deref().ok_or_else(|| GitError {
            code: "checkpoint_missing",
            message: "The preserved agent result is missing".into(),
        })?;
        let target_oid = rev_parse(
            &repo_root,
            &format!("refs/heads/{}", worktree.target_branch),
        )?;
        let path = worktree.resolution_path.clone().unwrap_or_else(|| {
            resolution_root
                .join(project.id.0.simple().to_string())
                .join(worktree.session_id.0.simple().to_string())
        });
        verify_child_path(resolution_root, &path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let branch = worktree
            .resolution_branch
            .clone()
            .unwrap_or_else(|| format!("ditch/resolver/{}", worktree.session_id.0.simple()));
        validate_resolution_branch(worktree, &branch)?;
        let reason = format!("The Ditch conflict resolver {}", worktree.session_id.0);
        git_ok(
            &repo_root,
            [
                "worktree",
                "add",
                "--lock",
                "--reason",
                &reason,
                "-b",
                &branch,
                path.to_string_lossy().as_ref(),
                &target_oid,
            ],
            None,
        )?;
        let merge = git_output(&path, ["merge", "--no-commit", "--no-ff", checkpoint], None)?;
        if merge.status.code() != Some(1) {
            let _ = git_ok(&path, ["merge", "--abort"], None);
            let _ = git_ok(
                &repo_root,
                ["worktree", "unlock", path.to_string_lossy().as_ref()],
                None,
            );
            let _ = git_ok(
                &repo_root,
                [
                    "worktree",
                    "remove",
                    "--force",
                    path.to_string_lossy().as_ref(),
                ],
                None,
            );
            let branch_ref = format!("refs/heads/{branch}");
            if let Ok(current) = rev_parse(&repo_root, &branch_ref) {
                let _ = git_ok(
                    &repo_root,
                    ["update-ref", "-d", &branch_ref, &current],
                    None,
                );
            }
            return Err(GitError {
                code: "resolution_state_changed",
                message: if merge.status.success() {
                    "The conflict changed and can now be checked normally".into()
                } else {
                    output_detail(&merge)
                },
            });
        }
        worktree.resolution_path = Some(path.clone());
        worktree.resolution_branch = Some(branch);
        worktree.resolution_target_oid = Some(target_oid);
        worktree.status = WorktreeStatus::NeedsReview;
        worktree.updated_at = Utc::now();
        Ok(path)
    }

    pub fn finalize_conflict_resolution(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
        validation_root: &Path,
    ) -> Result<(), GitError> {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        let path = worktree.resolution_path.clone().ok_or_else(|| GitError {
            code: "resolution_missing",
            message: "No manual resolution workspace exists for this result".into(),
        })?;
        verify_resolution_path(project, worktree, &path)?;
        let checkpoint = worktree.checkpoint_oid.clone().ok_or_else(|| GitError {
            code: "checkpoint_missing",
            message: "The preserved agent result is missing".into(),
        })?;
        let expected_target = worktree
            .resolution_target_oid
            .clone()
            .ok_or_else(|| GitError {
                code: "resolution_missing",
                message: "The resolution starting point is missing".into(),
            })?;
        let current_target = rev_parse(
            &repo_root,
            &format!("refs/heads/{}", worktree.target_branch),
        )?;
        if current_target != expected_target {
            return Err(GitError {
                code: "resolution_stale",
                message: "The project changed while this conflict was being resolved. The existing resolution was preserved.".into(),
            });
        }
        git_ok(&path, ["add", "-A"], None)?;
        let unmerged = git_bytes(&path, ["ls-files", "-u", "-z"], None)?;
        if !unmerged.is_empty() {
            return Err(GitError {
                code: "resolution_incomplete",
                message: "Some conflicting files still need a decision".into(),
            });
        }
        let check = git_output(&path, ["diff", "--cached", "--check"], None)?;
        if !check.status.success() {
            return Err(GitError {
                code: "resolution_markers_remain",
                message: "Conflict markers or invalid whitespace remain in the resolution".into(),
            });
        }
        let tree = git_text(&path, ["write-tree"], None)?;
        let candidate = commit_tree(
            &repo_root,
            &tree,
            &[&expected_target, &checkpoint],
            &format!(
                "Ditch resolved candidate for session {}",
                worktree.session_id.0
            ),
        )?;
        let candidate_ref = format!("{CANDIDATE_PREFIX}{}", worktree.session_id.0);
        let previous_candidate = rev_parse(&repo_root, &candidate_ref).ok();
        update_internal_ref(
            &repo_root,
            &candidate_ref,
            &candidate,
            previous_candidate.as_deref(),
        )?;
        worktree.candidate_oid = Some(candidate.clone());
        worktree.candidate_target_oid = Some(expected_target);
        worktree.candidate_checkpoint_oid = Some(checkpoint);
        worktree.conflict_state = ConflictState::None;
        worktree.status = WorktreeStatus::Validating;
        worktree.integration_state = IntegrationState::Validating;
        worktree.validation_state = ValidationState::Running;
        let project_root = canonical_or_original(&project.root);
        let project_relative = project_root
            .strip_prefix(&repo_root)
            .unwrap_or(Path::new(""));
        let checks = validate_candidate(
            &repo_root,
            validation_root,
            worktree,
            &candidate,
            project_relative,
            &project_root,
        )?;
        set_validation_result(worktree, checks);
        if worktree.validation_state == ValidationState::Passed {
            cleanup_resolution_worktree(&repo_root, worktree)?;
        }
        worktree.updated_at = Utc::now();
        Ok(())
    }

    pub fn review(
        &self,
        project: &Project,
        worktree: &ManagedWorktree,
    ) -> Result<WorktreeReview, GitError> {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        let checkpoint = worktree.checkpoint_oid.as_deref().ok_or_else(|| GitError {
            code: "checkpoint_missing",
            message: "The agent result is still being preserved. Try again in a moment.".into(),
        })?;
        let agent_changes = diff_name_status(&repo_root, &worktree.base_commit_oid, checkpoint)?;
        let current_target = rev_parse(
            &repo_root,
            &format!("refs/heads/{}", worktree.target_branch),
        )?;
        let candidate_is_stale = worktree.candidate_oid.is_some()
            && (worktree.candidate_checkpoint_oid.as_deref() != Some(checkpoint)
                || worktree
                    .candidate_target_oid
                    .as_deref()
                    .is_some_and(|expected| expected != current_target));
        let combined_changes = match (
            worktree.candidate_target_oid.as_deref(),
            worktree.candidate_oid.as_deref(),
        ) {
            (Some(target), Some(candidate)) if !candidate_is_stale => {
                diff_name_status(&repo_root, target, candidate)?
            }
            _ => Vec::new(),
        };
        Ok(WorktreeReview {
            session_id: worktree.session_id,
            base_commit_oid: worktree.base_commit_oid.clone(),
            checkpoint_oid: checkpoint.to_owned(),
            candidate_target_oid: worktree.candidate_target_oid.clone(),
            candidate_oid: worktree.candidate_oid.clone(),
            candidate_is_stale,
            agent_changes,
            combined_changes,
        })
    }

    pub fn apply(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
        allow_without_validation: bool,
    ) -> Result<(), GitError> {
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        if !matches!(
            worktree.status,
            WorktreeStatus::ReadyToApply | WorktreeStatus::Applying
        ) && !(allow_without_validation && worktree.status == WorktreeStatus::NeedsReview)
        {
            return Err(GitError {
                code: "integration_not_ready",
                message: "This task is not ready to apply".to_owned(),
            });
        }
        let validation_allows_apply = worktree.validation_state == ValidationState::Passed
            || (allow_without_validation
                && worktree.validation_state == ValidationState::NotConfigured);
        if !validation_allows_apply
            || worktree.candidate_checkpoint_oid.as_deref() != worktree.checkpoint_oid.as_deref()
        {
            return Err(GitError {
                code: "candidate_stale",
                message: "This result changed after validation and must be checked again".into(),
            });
        }
        let repo_root = repository_root(&project.root)?;
        let target_ref = format!("refs/heads/{}", worktree.target_branch);
        let current = rev_parse(&repo_root, &target_ref)?;
        let expected = worktree.candidate_target_oid.as_deref().unwrap_or_default();
        if current != expected {
            worktree.status = WorktreeStatus::NeedsReview;
            worktree.integration_state = IntegrationState::Queued;
            worktree.last_error =
                Some("The project advanced; the candidate must be checked again".into());
            return Err(GitError {
                code: "target_advanced",
                message: "The project advanced while this task was being checked. Revalidate it against the current project.".to_owned(),
            });
        }
        let candidate = worktree.candidate_oid.as_deref().ok_or_else(|| GitError {
            code: "candidate_missing",
            message: "The integration candidate is missing".to_owned(),
        })?;
        let listed = parse_worktree_porcelain(&git_bytes(
            &repo_root,
            ["worktree", "list", "--porcelain", "-z"],
            None,
        )?)?;
        let target_full = format!("refs/heads/{}", worktree.target_branch);
        if let Some(checkout) = listed
            .iter()
            .find(|item| item.branch.as_deref() == Some(&target_full))
        {
            if canonical_or_original(&checkout.path) != canonical_or_original(&repo_root) {
                return Err(GitError {
                    code: "target_checked_out_elsewhere",
                    message: "The project branch is checked out in another workspace; application is paused".to_owned(),
                });
            }
            if !user_changed_paths(&repo_root)?.is_empty() {
                worktree.integration_state = IntegrationState::Blocked;
                worktree.last_error = Some("The project workspace has local changes".into());
                return Err(GitError {
                    code: "canonical_workspace_dirty",
                    message: "Agent work is safe. The project has local changes, so application will wait until the project workspace is clean.".to_owned(),
                });
            }
            let current_branch = symbolic_head(&repo_root)?;
            if current_branch != worktree.target_branch {
                return Err(GitError {
                    code: "canonical_branch_changed",
                    message: "The project is currently on another branch".to_owned(),
                });
            }
            git_ok(&repo_root, ["merge", "--ff-only", candidate], None)?;
        } else {
            git_ok(
                &repo_root,
                ["update-ref", &target_ref, candidate, expected],
                None,
            )?;
        }
        worktree.status = WorktreeStatus::Integrated;
        worktree.integration_state = IntegrationState::Applied;
        worktree.updated_at = Utc::now();
        Ok(())
    }

    pub fn discard(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
        confirm_dirty: bool,
    ) -> Result<(), GitError> {
        verify_managed_path(project, worktree)?;
        self.refresh(project, worktree)?;
        if worktree.dirty && !confirm_dirty && worktree.checkpoint_oid.is_none() {
            return Err(GitError {
                code: "dirty_worktree_confirmation_required",
                message: "This task has unpreserved changes. Confirm discard to preserve a checkpoint before cleanup.".to_owned(),
            });
        }
        if worktree.dirty && worktree.checkpoint_oid.is_none() {
            self.checkpoint(project, worktree)?;
        }
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        if worktree.resolution_path.is_some() {
            verify_resolution_path(
                project,
                worktree,
                worktree.resolution_path.as_deref().unwrap_or(Path::new("")),
            )?;
            cleanup_resolution_worktree(&repo_root, worktree)?;
        }
        if worktree.path.exists() {
            git_ok(
                &repo_root,
                [
                    "worktree",
                    "unlock",
                    worktree.path.to_string_lossy().as_ref(),
                ],
                None,
            )?;
            let mut args = vec![OsString::from("worktree"), OsString::from("remove")];
            if worktree.dirty && worktree.checkpoint_oid.is_some() {
                args.push(OsString::from("--force"));
            }
            args.push(worktree.path.as_os_str().to_owned());
            git_ok(&repo_root, args, None)?;
        }
        validate_ditch_branch(&worktree.branch_name)?;
        let branch_ref = format!("refs/heads/{}", worktree.branch_name);
        if let Ok(current) = rev_parse(&repo_root, &branch_ref) {
            git_ok(
                &repo_root,
                ["update-ref", "-d", &branch_ref, &current],
                None,
            )?;
        }
        worktree.status = WorktreeStatus::Discarded;
        worktree.lock_state = WorktreeLockState::Missing;
        worktree.updated_at = Utc::now();
        Ok(())
    }

    pub fn archive_integrated(
        &self,
        project: &Project,
        worktree: &mut ManagedWorktree,
    ) -> Result<(), GitError> {
        verify_managed_path(project, worktree)?;
        if worktree.integration_state != IntegrationState::Applied
            || worktree.checkpoint_oid.is_none()
        {
            return Err(GitError {
                code: "archive_not_safe",
                message: "Integrated work cannot be archived until its checkpoint is durable"
                    .into(),
            });
        }
        let lock = self.project_lock(project)?;
        let _guard = lock
            .lock()
            .expect("project Git lock should not be poisoned");
        let repo_root = repository_root(&project.root)?;
        if worktree.resolution_path.is_some() {
            verify_resolution_path(
                project,
                worktree,
                worktree.resolution_path.as_deref().unwrap_or(Path::new("")),
            )?;
            cleanup_resolution_worktree(&repo_root, worktree)?;
        }
        if worktree.path.exists() {
            let _ = git_ok(
                &repo_root,
                [
                    "worktree",
                    "unlock",
                    worktree.path.to_string_lossy().as_ref(),
                ],
                None,
            );
            git_ok(
                &repo_root,
                [
                    "worktree",
                    "remove",
                    "--force",
                    worktree.path.to_string_lossy().as_ref(),
                ],
                None,
            )?;
        }
        validate_ditch_branch(&worktree.branch_name)?;
        let branch_ref = format!("refs/heads/{}", worktree.branch_name);
        if let Ok(current) = rev_parse(&repo_root, &branch_ref) {
            git_ok(
                &repo_root,
                ["update-ref", "-d", &branch_ref, &current],
                None,
            )?;
        }
        worktree.lock_state = WorktreeLockState::Missing;
        worktree.updated_at = Utc::now();
        Ok(())
    }
}

pub fn apply_overlap_projection(worktrees: &mut [ManagedWorktree]) {
    for item in worktrees.iter_mut() {
        item.overlapping_session_ids.clear();
        item.overlapping_paths.clear();
        item.overlap_risk = OverlapRisk::None;
        if item.status == WorktreeStatus::Waiting {
            item.status = if item.dirty {
                WorktreeStatus::Dirty
            } else {
                WorktreeStatus::Active
            };
        }
    }
    for left in 0..worktrees.len() {
        for right in (left + 1)..worktrees.len() {
            if worktrees[left].project_id != worktrees[right].project_id
                || terminal_worktree(&worktrees[left])
                || terminal_worktree(&worktrees[right])
            {
                continue;
            }
            let actual = paths_overlap(
                &worktrees[left].changed_paths,
                &worktrees[right].changed_paths,
            );
            let intent = intents_overlap(
                worktrees[left].intent.as_ref(),
                worktrees[right].intent.as_ref(),
            );
            if actual || intent {
                let left_id = worktrees[left].session_id;
                let right_id = worktrees[right].session_id;
                worktrees[left].overlapping_session_ids.push(right_id);
                worktrees[right].overlapping_session_ids.push(left_id);
                let risk = if actual {
                    OverlapRisk::Path
                } else {
                    OverlapRisk::Intent
                };
                worktrees[left].overlap_risk = risk.clone();
                worktrees[right].overlap_risk = risk;
                if actual {
                    let paths = overlapping_paths(
                        &worktrees[left].changed_paths,
                        &worktrees[right].changed_paths,
                    );
                    worktrees[left].overlapping_paths.extend(paths.clone());
                    worktrees[right].overlapping_paths.extend(paths);
                }
            }
        }
    }
}

fn diff_name_status(
    repo_root: &Path,
    from_oid: &str,
    to_oid: &str,
) -> Result<Vec<WorktreeFileChange>, GitError> {
    let output = git_bytes(
        repo_root,
        [
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            from_oid,
            to_oid,
        ],
        None,
    )?;
    parse_name_status_z(&output)
}

pub fn parse_name_status_z(bytes: &[u8]) -> Result<Vec<WorktreeFileChange>, GitError> {
    let fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = std::str::from_utf8(fields[index]).map_err(|_| GitError {
            code: "git_diff_parse_failed",
            message: "Git returned a non-UTF-8 change status".into(),
        })?;
        index += 1;
        let renamed = status.starts_with('R') || status.starts_with('C');
        let required = if renamed { 2 } else { 1 };
        if index + required > fields.len() {
            return Err(GitError {
                code: "git_diff_parse_failed",
                message: "Git returned an incomplete changed-file record".into(),
            });
        }
        let first = std::str::from_utf8(fields[index]).map_err(|_| GitError {
            code: "non_utf8_git_path",
            message: "This repository contains a non-UTF-8 path, which this Ditch version cannot safely display".into(),
        })?;
        index += 1;
        let (previous_path, path) = if renamed {
            let second = std::str::from_utf8(fields[index]).map_err(|_| GitError {
                code: "non_utf8_git_path",
                message: "This repository contains a non-UTF-8 path, which this Ditch version cannot safely display".into(),
            })?;
            index += 1;
            (
                Some(normalize_git_path(first)?),
                normalize_git_path(second)?,
            )
        } else {
            (None, normalize_git_path(first)?)
        };
        changes.push(WorktreeFileChange {
            status: status.to_owned(),
            path,
            previous_path,
        });
    }
    Ok(changes)
}

fn terminal_worktree(worktree: &ManagedWorktree) -> bool {
    matches!(
        worktree.status,
        WorktreeStatus::Integrated | WorktreeStatus::Discarded
    )
}

pub fn changed_paths(worktree: &Path) -> Result<Vec<String>, GitError> {
    let bytes = git_bytes(
        worktree,
        ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        None,
    )?;
    parse_status_porcelain_v2(&bytes)
}

fn user_changed_paths(worktree: &Path) -> Result<Vec<String>, GitError> {
    changed_paths(worktree).map(|paths| {
        paths
            .into_iter()
            .filter(|path| path != ".ditch" && !path.starts_with(".ditch/"))
            .collect()
    })
}

pub fn parse_status_porcelain_v2(bytes: &[u8]) -> Result<Vec<String>, GitError> {
    let fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut paths = BTreeSet::new();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        index += 1;
        if field.is_empty() {
            continue;
        }
        match field[0] {
            b'?' => insert_status_path(field, 1, &mut paths)?,
            b'!' | b'#' => {}
            b'1' => insert_status_path(field, 8, &mut paths)?,
            b'2' => {
                insert_status_path(field, 9, &mut paths)?;
                if let Some(original) = fields.get(index) {
                    insert_git_path(original, &mut paths)?;
                    index += 1;
                }
            }
            b'u' => insert_status_path(field, 10, &mut paths)?,
            _ => {}
        }
    }
    Ok(paths.into_iter().collect())
}

fn insert_status_path(
    field: &[u8],
    spaces: usize,
    paths: &mut BTreeSet<String>,
) -> Result<(), GitError> {
    let mut offset = 0;
    for _ in 0..spaces {
        let Some(found) = field[offset..].iter().position(|byte| *byte == b' ') else {
            return Err(GitError {
                code: "git_status_parse_failed",
                message: "Malformed porcelain v2 status record".into(),
            });
        };
        offset += found + 1;
    }
    insert_git_path(&field[offset..], paths)
}

fn insert_git_path(path: &[u8], paths: &mut BTreeSet<String>) -> Result<(), GitError> {
    let value = std::str::from_utf8(path).map_err(|_| GitError {
        code: "non_utf8_git_path",
        message: "This repository contains a non-UTF-8 path, which this Ditch version cannot safely display".into(),
    })?;
    let normalized = normalize_git_path(value)?;
    paths.insert(normalized);
    Ok(())
}

pub fn parse_worktree_porcelain(bytes: &[u8]) -> Result<Vec<ListedWorktree>, GitError> {
    let mut result = Vec::new();
    let mut current: Option<ListedWorktree> = None;
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(item) = current.take() {
                result.push(item);
            }
            continue;
        }
        let text = std::str::from_utf8(field).map_err(|_| GitError {
            code: "worktree_parse_failed",
            message: "Git returned a non-UTF-8 worktree path".into(),
        })?;
        if let Some(path) = text.strip_prefix("worktree ") {
            if let Some(item) = current.take() {
                result.push(item);
            }
            current = Some(ListedWorktree {
                path: PathBuf::from(path),
                head: None,
                branch: None,
                locked: false,
                prunable: false,
            });
        } else if let Some(item) = current.as_mut() {
            if let Some(head) = text.strip_prefix("HEAD ") {
                item.head = Some(head.into());
            } else if let Some(branch) = text.strip_prefix("branch ") {
                item.branch = Some(branch.into());
            } else if text == "locked" || text.starts_with("locked ") {
                item.locked = true;
            } else if text == "prunable" || text.starts_with("prunable ") {
                item.prunable = true;
            }
        }
    }
    if let Some(item) = current {
        result.push(item);
    }
    Ok(result)
}

fn infer_change_intent(prompt: &str) -> ChangeIntent {
    let expected_paths = prompt
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                matches!(
                    character,
                    '`' | '\'' | '"' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
                )
            })
        })
        .filter(|token| {
            !token.starts_with('/')
                && !token.contains("..")
                && (token.contains('/')
                    || token.rsplit_once('.').is_some_and(|(_, extension)| {
                        !extension.is_empty() && extension.len() <= 12
                    }))
        })
        .filter_map(|path| normalize_git_path(path.trim_end_matches('.')).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ChangeIntent {
        summary: prompt.trim().chars().take(240).collect(),
        expected_paths,
        shared_paths: Vec::new(),
    }
}

fn intents_overlap(left: Option<&ChangeIntent>, right: Option<&ChangeIntent>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    let left_paths = left
        .expected_paths
        .iter()
        .chain(&left.shared_paths)
        .cloned()
        .collect::<Vec<_>>();
    let right_paths = right
        .expected_paths
        .iter()
        .chain(&right.shared_paths)
        .cloned()
        .collect::<Vec<_>>();
    patterns_overlap(&left_paths, &right_paths)
}

fn paths_overlap(left: &[String], right: &[String]) -> bool {
    left.iter().any(|a| {
        let a = a.to_lowercase();
        right.iter().any(|b| a == b.to_lowercase())
    })
}

fn overlapping_paths(left: &[String], right: &[String]) -> Vec<String> {
    left.iter()
        .filter(|left_path| {
            right
                .iter()
                .any(|right_path| left_path.eq_ignore_ascii_case(right_path))
        })
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn patterns_overlap(left: &[String], right: &[String]) -> bool {
    left.iter()
        .any(|a| right.iter().any(|b| pattern_pair_overlaps(a, b)))
}

fn pattern_pair_overlaps(left: &str, right: &str) -> bool {
    let left_value = left.to_lowercase();
    let right_value = right.to_lowercase();
    let left = left_value.trim_end_matches("/**").trim_end_matches('/');
    let right = right_value.trim_end_matches("/**").trim_end_matches('/');
    left == right || path_is_under(left, right) || path_is_under(right, left)
}

fn path_is_under(path: &str, directory: &str) -> bool {
    path.strip_prefix(directory)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn normalize_git_path(value: &str) -> Result<String, GitError> {
    if value.is_empty() || value.starts_with('/') || value.contains('\0') {
        return Err(GitError {
            code: "invalid_git_path",
            message: "Git returned an unsafe repository path".into(),
        });
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GitError {
            code: "invalid_git_path",
            message: format!("Unsafe repository path: {value}"),
        });
    }
    Ok(value.replace('\\', "/"))
}

fn sanitize_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    if slug.is_empty() {
        "task".to_owned()
    } else {
        slug.trim_end_matches('-').to_owned()
    }
}

fn validate_ditch_branch(branch: &str) -> Result<(), GitError> {
    if !branch.starts_with(DITCH_BRANCH_PREFIX) || branch.contains("..") || branch.contains("@{") {
        return Err(GitError {
            code: "unsafe_branch",
            message: "Refusing to mutate a branch not owned by Ditch".into(),
        });
    }
    Ok(())
}

fn verify_managed_path(project: &Project, worktree: &ManagedWorktree) -> Result<(), GitError> {
    let expected_project = project.id.0.simple().to_string();
    let expected_session = worktree.session_id.0.simple().to_string()[..12].to_owned();
    let project_component = worktree
        .path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str);
    let root_component = worktree
        .path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(OsStr::to_str);
    let leaf = worktree
        .path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if project_component != Some(expected_project.as_str())
        || root_component != Some("worktrees")
        || !leaf.starts_with(&format!("{expected_session}-"))
    {
        return Err(GitError {
            code: "worktree_ownership_unproven",
            message: "Refusing cleanup because the workspace path does not match Ditch ownership metadata".into(),
        });
    }
    Ok(())
}

fn verify_child_path(root: &Path, candidate: &Path) -> Result<(), GitError> {
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
        || !candidate.starts_with(root)
    {
        return Err(GitError {
            code: "managed_path_invalid",
            message: "The managed workspace path is outside Ditch storage".into(),
        });
    }
    Ok(())
}

fn verify_resolution_path(
    project: &Project,
    worktree: &ManagedWorktree,
    path: &Path,
) -> Result<(), GitError> {
    let expected_project = project.id.0.simple().to_string();
    let expected_session = worktree.session_id.0.simple().to_string();
    let project_component = path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str);
    let root_component = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(OsStr::to_str);
    if project_component != Some(expected_project.as_str())
        || root_component != Some("resolution-worktrees")
        || path.file_name().and_then(OsStr::to_str) != Some(expected_session.as_str())
    {
        return Err(GitError {
            code: "resolution_ownership_unproven",
            message: "Refusing to modify a resolution workspace whose ownership cannot be proven"
                .into(),
        });
    }
    Ok(())
}

fn repository_root(cwd: &Path) -> Result<PathBuf, GitError> {
    let value = git_text(cwd, ["rev-parse", "--show-toplevel"], None)?;
    Ok(canonical_or_original(Path::new(&value)))
}

fn symbolic_head(cwd: &Path) -> Result<String, GitError> {
    git_text(cwd, ["symbolic-ref", "--quiet", "--short", "HEAD"], None).map_err(|_| GitError {
        code: "detached_target_unsupported",
        message: "Choose a project branch before starting parallel work; detached HEAD cannot be an integration target".into(),
    })
}

fn inspect_initial_snapshot_unlocked(
    project: &Project,
) -> Result<InitialSnapshotPreview, GitError> {
    let repo_root = repository_root(&project.root)?;
    let target_branch = symbolic_head(&repo_root)?;
    let target_oid = rev_parse(&repo_root, "HEAD").ok();
    let preflight_paths = user_changed_paths(&repo_root)?;
    let operation_markers = [
        ("MERGE_HEAD", "A merge is currently in progress."),
        (
            "CHERRY_PICK_HEAD",
            "A cherry-pick is currently in progress.",
        ),
        ("REVERT_HEAD", "A revert is currently in progress."),
        ("rebase-merge", "A rebase is currently in progress."),
        ("rebase-apply", "A rebase is currently in progress."),
        ("BISECT_LOG", "Git bisect is currently in progress."),
    ];
    let mut active_operation_warnings = Vec::new();
    for (marker, warning) in operation_markers {
        let marker_path = git_text(&repo_root, ["rev-parse", "--git-path", marker], None)?;
        let marker_path = Path::new(&marker_path);
        let marker_path = if marker_path.is_absolute() {
            marker_path.to_path_buf()
        } else {
            repo_root.join(marker_path)
        };
        if marker_path.exists() {
            active_operation_warnings.push(warning.to_owned());
        }
    }
    if !active_operation_warnings.is_empty() {
        return Ok(InitialSnapshotPreview {
            repository_root: repo_root,
            target_branch,
            included_paths: preflight_paths,
            warnings: active_operation_warnings,
            tree_oid: None,
            target_oid,
            ready: false,
            unsafe_to_snapshot: true,
        });
    }
    if target_oid.is_some() && preflight_paths.is_empty() {
        return Ok(InitialSnapshotPreview {
            repository_root: repo_root,
            target_branch,
            included_paths: Vec::new(),
            warnings: Vec::new(),
            tree_oid: None,
            target_oid,
            ready: true,
            unsafe_to_snapshot: false,
        });
    }
    let mut preflight_warnings = Vec::new();
    if target_oid.is_some() {
        let staged = git_output(
            &repo_root,
            ["diff", "--cached", "--quiet", "--ignore-submodules"],
            None,
        )?;
        match staged.status.code() {
            Some(0) => {}
            Some(1) => preflight_warnings.push(
                "Some changes were staged in Terminal. Review before Ditch saves the complete current project state."
                    .into(),
            ),
            _ => return Err(git_failure(&staged)),
        }
    }
    let mut nested_repository = false;
    for path in &preflight_paths {
        let lower = path.to_lowercase();
        let file_name = lower
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&lower);
        if sensitive_snapshot_name(file_name) {
            preflight_warnings.push(format!(
                "{} may contain credentials; review it before creating the snapshot.",
                path
            ));
        }
        if repo_root.join(path).join(".git").exists() {
            preflight_warnings.push(format!(
                "{} is a nested Git repository and must be prepared manually.",
                path.trim_end_matches('/')
            ));
            nested_repository = true;
        }
    }
    if nested_repository {
        preflight_warnings.sort();
        preflight_warnings.dedup();
        return Ok(InitialSnapshotPreview {
            repository_root: repo_root,
            target_branch,
            included_paths: preflight_paths,
            warnings: preflight_warnings,
            tree_oid: None,
            target_oid,
            ready: false,
            unsafe_to_snapshot: true,
        });
    }
    let temp_index = unique_temp_path("ditch-initial-index");
    let index_env = [(
        OsString::from("GIT_INDEX_FILE"),
        temp_index.as_os_str().to_owned(),
    )];
    let result = (|| {
        git_ok(&repo_root, ["read-tree", "--empty"], Some(&index_env))?;
        git_ok(&repo_root, ["add", "-A", "--", "."], Some(&index_env))?;
        let metadata_path = canonical_or_original(&project.root)
            .join(".ditch")
            .strip_prefix(&repo_root)
            .map(Path::to_path_buf)
            .map_err(|_| GitError {
                code: "project_repository_mismatch",
                message: "The project metadata path is outside its Git repository".into(),
            })?;
        git_ok(
            &repo_root,
            [
                OsString::from("rm"),
                OsString::from("-r"),
                OsString::from("--cached"),
                OsString::from("--ignore-unmatch"),
                OsString::from("--"),
                metadata_path.as_os_str().to_owned(),
            ],
            Some(&index_env),
        )?;
        let tree_oid = git_text(&repo_root, ["write-tree"], Some(&index_env))?;
        let listed = git_bytes(&repo_root, ["ls-files", "-z"], Some(&index_env))?;
        let mut included_paths = Vec::new();
        let mut warnings = preflight_warnings;
        let mut unsafe_to_snapshot = false;
        for raw in listed
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let Ok(path) = std::str::from_utf8(raw) else {
                warnings.push(
                    "A repository path is not valid UTF-8 and cannot be reviewed safely.".into(),
                );
                unsafe_to_snapshot = true;
                continue;
            };
            let normalized = normalize_git_path(path)?;
            let lower = normalized.to_lowercase();
            let file_name = lower.rsplit('/').next().unwrap_or(&lower);
            if sensitive_snapshot_name(file_name) {
                warnings.push(format!(
                    "{} may contain credentials; review it before creating the snapshot.",
                    normalized
                ));
            }
            let absolute = repo_root.join(Path::new(&normalized));
            if absolute.join(".git").exists() {
                warnings.push(format!(
                    "{} is a nested Git repository and must be prepared manually.",
                    normalized
                ));
                unsafe_to_snapshot = true;
            }
            if fs::metadata(&absolute)
                .map(|metadata| metadata.len() > 100 * 1024 * 1024)
                .unwrap_or(false)
            {
                warnings.push(format!("{} is larger than 100 MB.", normalized));
            }
            included_paths.push(normalized);
        }
        included_paths.sort();
        warnings.sort();
        warnings.dedup();
        if let Some(head) = target_oid.as_deref() {
            let head_tree = rev_parse(&repo_root, &format!("{head}^{{tree}}"))?;
            if head_tree == tree_oid {
                return Ok(InitialSnapshotPreview {
                    repository_root: repo_root.clone(),
                    target_branch,
                    included_paths: Vec::new(),
                    warnings: Vec::new(),
                    tree_oid: None,
                    target_oid: Some(head.to_owned()),
                    ready: true,
                    unsafe_to_snapshot: false,
                });
            }
        }
        Ok(InitialSnapshotPreview {
            repository_root: repo_root.clone(),
            target_branch,
            included_paths,
            warnings,
            tree_oid: Some(tree_oid),
            target_oid,
            ready: false,
            unsafe_to_snapshot,
        })
    })();
    let _ = fs::remove_file(temp_index);
    result
}

fn sensitive_snapshot_name(file_name: &str) -> bool {
    matches!(file_name, ".env" | "id_rsa" | "id_ed25519")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
}

fn ensure_head(cwd: &Path, branch: &str) -> Result<String, GitError> {
    if let Ok(oid) = rev_parse(cwd, "HEAD") {
        return Ok(oid);
    }
    if !user_changed_paths(cwd)?.is_empty() {
        return Err(GitError {
            code: "unborn_repository_has_changes",
            message: "Ditch has not finished preparing the current project files. No agent was started; try again.".into(),
        });
    }
    let tree = git_text_with_input(cwd, ["mktree"], b"")?;
    let commit = commit_tree(cwd, &tree, &[], "Ditch initial baseline")?;
    let reference = format!("refs/heads/{branch}");
    git_ok(
        cwd,
        [
            "update-ref",
            &reference,
            &commit,
            "0000000000000000000000000000000000000000",
        ],
        None,
    )?;
    Ok(commit)
}

fn rev_parse(cwd: &Path, reference: &str) -> Result<String, GitError> {
    git_text(cwd, ["rev-parse", "--verify", reference], None)
}

fn git_bytes<I, S>(
    cwd: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<Vec<u8>, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_output(cwd, args, env)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_failure(&output))
    }
}

fn git_text<I, S>(
    cwd: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<String, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let bytes = git_bytes(cwd, args, env)?;
    String::from_utf8(bytes)
        .map(|value| value.trim().to_owned())
        .map_err(|_| GitError {
            code: "git_output_invalid",
            message: "Git returned non-UTF-8 object data".into(),
        })
}

fn git_ok<I, S>(cwd: &Path, args: I, env: Option<&[(OsString, OsString)]>) -> Result<(), GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_bytes(cwd, args, env).map(|_| ())
}

fn git_output<I, S>(
    cwd: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.args(["-c", "color.ui=false", "-c", "core.quotepath=false"]);
    command.args(args);
    command.current_dir(cwd);
    command.env("LC_ALL", "C");
    if let Some(env) = env {
        command.envs(env.iter().cloned());
    }
    command.output().map_err(Into::into)
}

fn git_text_with_input<I, S>(cwd: &Path, args: I, input: &[u8]) -> Result<String, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env("LC_ALL", "C");
    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| GitError {
            code: "git_io_failed",
            message: "Git stdin was unavailable".into(),
        })?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(git_failure(&output));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| GitError {
            code: "git_output_invalid",
            message: "Git returned non-UTF-8 object data".into(),
        })
}

fn commit_tree(
    cwd: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> Result<String, GitError> {
    let mut args = vec![OsString::from("commit-tree"), OsString::from(tree)];
    for parent in parents {
        args.extend([OsString::from("-p"), OsString::from(parent)]);
    }
    args.extend([OsString::from("-m"), OsString::from(message)]);
    let identity = [
        (
            OsString::from("GIT_AUTHOR_NAME"),
            OsString::from("The Ditch"),
        ),
        (
            OsString::from("GIT_AUTHOR_EMAIL"),
            OsString::from("ditch@localhost"),
        ),
        (
            OsString::from("GIT_COMMITTER_NAME"),
            OsString::from("The Ditch"),
        ),
        (
            OsString::from("GIT_COMMITTER_EMAIL"),
            OsString::from("ditch@localhost"),
        ),
    ];
    git_text(cwd, args, Some(&identity))
}

fn update_internal_ref(
    cwd: &Path,
    reference: &str,
    oid: &str,
    old: Option<&str>,
) -> Result<(), GitError> {
    if !reference.starts_with("refs/ditch/") {
        return Err(GitError {
            code: "unsafe_ref",
            message: "Refusing to update a non-Ditch internal ref".into(),
        });
    }
    let expected = old.unwrap_or("0000000000000000000000000000000000000000");
    git_ok(cwd, ["update-ref", reference, oid, expected], None)
}

fn validate_candidate(
    repo_root: &Path,
    validation_root: &Path,
    worktree: &ManagedWorktree,
    candidate: &str,
    project_relative: &Path,
    config_root: &Path,
) -> Result<Vec<ValidationCheck>, GitError> {
    fs::create_dir_all(validation_root)?;
    let path = validation_root.join(worktree.session_id.0.simple().to_string());
    if path.exists() {
        return Err(GitError {
            code: "validation_path_exists",
            message: "A previous validation workspace requires reconciliation".into(),
        });
    }
    git_ok(
        repo_root,
        [
            "worktree",
            "add",
            "--detach",
            path.to_string_lossy().as_ref(),
            candidate,
        ],
        None,
    )?;
    let candidate_project = path.join(project_relative);
    let commands = discover_validation_commands(&candidate_project, config_root);
    let mut checks = Vec::new();
    for (name, program, args, cwd) in commands {
        let mut command = Command::new(&program);
        command.args(&args).current_dir(cwd).env("CI", "1");
        add_existing_dependency_environment(&mut command, config_root);
        let output = validation_command_output(command, Duration::from_secs(600));
        match output {
            Ok(output) => checks.push(ValidationCheck {
                name,
                passed: output.status.success(),
                exit_code: output.status.code(),
                output: validation_output_detail(&output)
                    .chars()
                    .take(20_000)
                    .collect(),
            }),
            Err(message) => checks.push(ValidationCheck {
                name,
                passed: false,
                exit_code: None,
                output: message,
            }),
        }
        if checks.last().is_some_and(|check| !check.passed) {
            break;
        }
    }
    let removal = git_ok(
        repo_root,
        ["worktree", "remove", path.to_string_lossy().as_ref()],
        None,
    );
    if let Err(error) = removal {
        return Err(GitError {
            code: "validation_cleanup_failed",
            message: error.message,
        });
    }
    Ok(checks)
}

fn set_validation_result(worktree: &mut ManagedWorktree, checks: Vec<ValidationCheck>) {
    let configured = !checks.is_empty();
    let passed = configured && checks.iter().all(|check| check.passed);
    worktree.validation_checks = checks;
    if passed {
        worktree.status = WorktreeStatus::ReadyToApply;
        worktree.integration_state = IntegrationState::Ready;
        worktree.validation_state = ValidationState::Passed;
        worktree.conflict_state = ConflictState::None;
        worktree.last_error = None;
    } else {
        worktree.status = WorktreeStatus::NeedsReview;
        worktree.conflict_state = if configured {
            ConflictState::ValidationFailed
        } else {
            ConflictState::None
        };
        worktree.integration_state = IntegrationState::Blocked;
        worktree.validation_state = if configured {
            ValidationState::Failed
        } else {
            ValidationState::NotConfigured
        };
        worktree.last_error = Some(if configured {
            "Combined validation failed".to_owned()
        } else {
            "No combined validation command is configured for this project".to_owned()
        });
    }
}

fn cleanup_resolution_worktree(
    repo_root: &Path,
    worktree: &mut ManagedWorktree,
) -> Result<(), GitError> {
    let Some(path) = worktree.resolution_path.clone() else {
        return Ok(());
    };
    git_ok(
        repo_root,
        ["worktree", "unlock", path.to_string_lossy().as_ref()],
        None,
    )?;
    git_ok(
        repo_root,
        [
            "worktree",
            "remove",
            "--force",
            path.to_string_lossy().as_ref(),
        ],
        None,
    )?;
    if let Some(branch) = worktree.resolution_branch.as_deref() {
        validate_resolution_branch(worktree, branch)?;
        let branch_ref = format!("refs/heads/{branch}");
        if let Ok(current) = rev_parse(repo_root, &branch_ref) {
            git_ok(repo_root, ["update-ref", "-d", &branch_ref, &current], None)?;
        }
    }
    worktree.resolution_path = None;
    worktree.resolution_branch = None;
    worktree.resolution_target_oid = None;
    Ok(())
}

fn validate_resolution_branch(worktree: &ManagedWorktree, branch: &str) -> Result<(), GitError> {
    let expected = format!("ditch/resolver/{}", worktree.session_id.0.simple());
    if branch != expected {
        return Err(GitError {
            code: "resolution_ownership_unproven",
            message: "Refusing to modify a resolver branch whose ownership cannot be proven".into(),
        });
    }
    Ok(())
}

type ValidationCommand = (String, String, Vec<String>, PathBuf);

fn discover_validation_commands(root: &Path, config_root: &Path) -> Vec<ValidationCommand> {
    if let Ok(bytes) = fs::read(config_root.join(".ditch/validation.json"))
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(commands) = value.get("commands").and_then(|item| item.as_array())
    {
        return commands
            .iter()
            .filter_map(|command| {
                let parts = command
                    .as_array()?
                    .iter()
                    .map(|part| part.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?;
                let (program, args) = parts.split_first()?;
                Some((
                    parts.join(" "),
                    program.clone(),
                    args.to_vec(),
                    root.to_path_buf(),
                ))
            })
            .collect();
    }
    let task_runner = discover_task_runner_commands(root);
    if !task_runner.is_empty() {
        return task_runner;
    }
    let mut commands = Vec::new();
    if root.join("package.json").is_file() {
        commands.extend(discover_package_json_commands(root));
    }
    if root.join("Cargo.toml").is_file() {
        commands.push((
            "Rust tests".into(),
            "cargo".into(),
            vec!["test".into(), "--workspace".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("pubspec.yaml").is_file() {
        commands.push((
            "Dart/Flutter analysis".into(),
            "flutter".into(),
            vec!["analyze".into()],
            root.to_path_buf(),
        ));
        commands.push((
            "Flutter tests".into(),
            "flutter".into(),
            vec!["test".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("go.mod").is_file() {
        commands.push((
            "Go tests".into(),
            "go".into(),
            vec!["test".into(), "./...".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("Package.swift").is_file() {
        commands.push((
            "Swift tests".into(),
            "swift".into(),
            vec!["test".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("gradlew").is_file()
        || root.join("build.gradle").is_file()
        || root.join("build.gradle.kts").is_file()
    {
        let program = if root.join("gradlew").is_file() {
            "./gradlew"
        } else {
            "gradle"
        };
        commands.push((
            "Gradle tests".into(),
            program.into(),
            vec!["test".into(), "--no-daemon".into()],
            root.to_path_buf(),
        ));
    } else if root.join("mvnw").is_file() || root.join("pom.xml").is_file() {
        commands.push((
            "Maven tests".into(),
            if root.join("mvnw").is_file() {
                "./mvnw".into()
            } else {
                "mvn".into()
            },
            vec!["test".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("pyproject.toml").is_file()
        || root.join("pytest.ini").is_file()
        || root.join("tox.ini").is_file()
        || root.join("noxfile.py").is_file()
    {
        commands.extend(discover_python_commands(root));
    }
    if root.join("mix.exs").is_file() {
        commands.push((
            "Elixir tests".into(),
            "mix".into(),
            vec!["test".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("Gemfile").is_file() && root.join("Rakefile").is_file() {
        commands.push((
            "Ruby tests".into(),
            "bundle".into(),
            vec!["exec".into(), "rake".into(), "test".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("composer.json").is_file() {
        commands.extend(discover_composer_commands(root));
    }
    if has_extension(root, "csproj") || has_extension(root, "sln") {
        commands.push((
            ".NET tests".into(),
            "dotnet".into(),
            vec!["test".into(), "--no-restore".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("MODULE.bazel").is_file() || root.join("WORKSPACE").is_file() {
        commands.push((
            "Bazel tests".into(),
            "bazel".into(),
            vec!["test".into(), "//...".into()],
            root.to_path_buf(),
        ));
    }
    if root.join("pants.toml").is_file() {
        commands.push((
            "Pants tests".into(),
            if root.join("pants").is_file() {
                "./pants".into()
            } else {
                "pants".into()
            },
            vec!["test".into(), "::".into()],
            root.to_path_buf(),
        ));
    }
    commands
}

fn discover_package_json_commands(root: &Path) -> Vec<ValidationCommand> {
    let Ok(bytes) = fs::read(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|item| item.as_object()) else {
        return Vec::new();
    };
    let (program, run_arg) = if root.join("pnpm-lock.yaml").is_file() {
        ("pnpm", "run")
    } else if root.join("yarn.lock").is_file() {
        ("yarn", "run")
    } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
        ("bun", "run")
    } else {
        ("npm", "run")
    };
    let preferred = [
        "check",
        "validate",
        "lint",
        "typecheck",
        "type-check",
        "test",
        "build",
    ];
    preferred
        .into_iter()
        .filter_map(|script| {
            let body = scripts.get(script)?.as_str()?;
            if body.contains("no test specified") {
                return None;
            }
            Some((
                format!("{script} ({program})"),
                program.into(),
                vec![run_arg.into(), script.into()],
                root.to_path_buf(),
            ))
        })
        .collect()
}

fn discover_python_commands(root: &Path) -> Vec<ValidationCommand> {
    let config = ["pyproject.toml", "pytest.ini", "tox.ini"]
        .into_iter()
        .filter_map(|name| fs::read_to_string(root.join(name)).ok())
        .collect::<Vec<_>>()
        .join("\n");
    let prefix = if root.join("uv.lock").is_file() {
        vec!["uv", "run"]
    } else if root.join("poetry.lock").is_file() {
        vec!["poetry", "run"]
    } else {
        Vec::new()
    };
    let mut commands = Vec::new();
    if root.join("tox.ini").is_file() {
        commands.push((
            "Python tox environments".into(),
            "tox".into(),
            Vec::new(),
            root.to_path_buf(),
        ));
        return commands;
    }
    if root.join("noxfile.py").is_file() {
        commands.push((
            "Python nox sessions".into(),
            "nox".into(),
            Vec::new(),
            root.to_path_buf(),
        ));
        return commands;
    }
    for (configured, name, tool, args) in [
        (
            config.contains("[tool.ruff") || root.join("ruff.toml").is_file(),
            "Python lint",
            "ruff",
            vec!["check", "."],
        ),
        (
            config.contains("[tool.mypy"),
            "Python typecheck",
            "mypy",
            vec!["."],
        ),
        (
            config.contains("pytest") || root.join("tests").is_dir(),
            "Python tests",
            "pytest",
            Vec::new(),
        ),
    ] {
        if !configured {
            continue;
        }
        let (program, mut command_args) = if let Some((program, runner)) = prefix.split_first() {
            (
                (*program).to_owned(),
                runner.iter().map(|value| (*value).to_owned()).collect(),
            )
        } else {
            (tool.to_owned(), Vec::new())
        };
        if !prefix.is_empty() {
            command_args.push(tool.to_owned());
        }
        command_args.extend(args.into_iter().map(str::to_owned));
        commands.push((name.into(), program, command_args, root.to_path_buf()));
    }
    commands
}

fn discover_composer_commands(root: &Path) -> Vec<ValidationCommand> {
    let Ok(bytes) = fs::read(root.join("composer.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|item| item.as_object()) else {
        return Vec::new();
    };
    ["check", "lint", "test"]
        .into_iter()
        .filter(|name| scripts.contains_key(*name))
        .map(|name| {
            (
                format!("{name} (Composer)"),
                "composer".into(),
                vec!["run-script".into(), name.into()],
                root.to_path_buf(),
            )
        })
        .collect()
}

fn discover_task_runner_commands(root: &Path) -> Vec<ValidationCommand> {
    for (file, program) in [("Makefile", "make"), ("justfile", "just")] {
        let Ok(contents) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        for target in ["check", "validate", "test"] {
            if contents.lines().any(|line| {
                !line.starts_with([' ', '\t'])
                    && line
                        .split_once(':')
                        .is_some_and(|(name, _)| name.trim() == target)
            }) {
                return vec![(
                    format!("{target} ({program})"),
                    program.into(),
                    vec![target.into()],
                    root.to_path_buf(),
                )];
            }
        }
    }
    Vec::new()
}

fn has_extension(root: &Path, extension: &str) -> bool {
    fs::read_dir(root).ok().is_some_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == extension)
        })
    })
}

fn add_existing_dependency_environment(command: &mut Command, project_root: &Path) {
    let mut executable_paths = Vec::new();
    let node_modules = project_root.join("node_modules");
    if node_modules.is_dir() {
        executable_paths.push(node_modules.join(".bin"));
        command.env("NODE_PATH", &node_modules);
    }
    let virtual_env = project_root.join(".venv");
    if virtual_env.is_dir() {
        executable_paths.push(virtual_env.join("bin"));
        command.env("VIRTUAL_ENV", &virtual_env);
    }
    if let Some(existing) = std::env::var_os("PATH") {
        executable_paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(path) = std::env::join_paths(executable_paths) {
        command.env("PATH", path);
    }
}

fn validation_command_output(mut command: Command, timeout: Duration) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start validation: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                return Err("Validation timed out after 10 minutes".into());
            }
            Err(error) => return Err(format!("Validation process failed: {error}")),
        }
    };
    Ok(Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

fn read_pipe(mut pipe: Option<impl Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(ref mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes);
    }
    bytes
}

fn validation_output_detail(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => "Validation completed without output".into(),
        (stdout, "") => stdout.into(),
        ("", stderr) => stderr.into(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn first_oid(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()?
        .split(|character: char| character == '\0' || character.is_whitespace())
        .find(|value| !value.is_empty())
        .filter(|value| value.len() == 40 || value.len() == 64)
        .map(str::to_owned)
}

fn output_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        stderr
    } else {
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}

fn git_failure(output: &Output) -> GitError {
    GitError {
        code: "git_command_failed",
        message: output_detail(output),
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn unique_temp_path(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> PathBuf {
        let root = unique_temp_path(name);
        fs::create_dir_all(&root).unwrap();
        git_ok(&root, ["init", "-b", "main"], None).unwrap();
        fs::write(root.join("base.txt"), "base\n").unwrap();
        git_ok(&root, ["add", "base.txt"], None).unwrap();
        let tree = git_text(&root, ["write-tree"], None).unwrap();
        let commit = commit_tree(&root, &tree, &[], "base").unwrap();
        git_ok(&root, ["update-ref", "refs/heads/main", &commit], None).unwrap();
        git_ok(&root, ["reset", "--hard", &commit], None).unwrap();
        fs::create_dir_all(root.join(".ditch")).unwrap();
        fs::write(
            root.join(".ditch/validation.json"),
            r#"{"commands":[["sh","-c","exit 0"]]}"#,
        )
        .unwrap();
        root
    }

    #[test]
    fn parses_nul_status_including_rename_and_spaces() {
        let data = b"1 .M N... 100644 100644 100644 aaaaaaa bbbbbbb file with space.txt\x002 R. N... 100644 100644 100644 aaaaaaa bbbbbbb R100 new name.txt\0old name.txt\0? untracked.txt\0";
        assert_eq!(
            parse_status_porcelain_v2(data).unwrap(),
            vec![
                "file with space.txt",
                "new name.txt",
                "old name.txt",
                "untracked.txt"
            ]
        );
    }

    #[test]
    fn path_prefix_overlap_respects_component_boundaries() {
        assert!(pattern_pair_overlaps("lib/auth/**", "lib/auth/login.dart"));
        assert!(!pattern_pair_overlaps(
            "lib/auth/**",
            "lib/authentication_old/a.dart"
        ));
    }

    #[test]
    fn name_status_z_parses_paths_with_spaces_and_renames() {
        let changes = parse_name_status_z(
            b"M\0lib/file with spaces.dart\0R100\0old name.dart\0new name.dart\0D\0gone.txt\0",
        )
        .unwrap();
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].path, "lib/file with spaces.dart");
        assert_eq!(changes[1].status, "R100");
        assert_eq!(changes[1].previous_path.as_deref(), Some("old name.dart"));
        assert_eq!(changes[1].path, "new name.dart");
        assert_eq!(changes[2].status, "D");
    }

    #[test]
    fn discovers_declared_checks_across_supported_project_tooling() {
        let root = unique_temp_path("ditch-validation-discovery");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","typecheck":"tsc --noEmit","test":"echo \"Error: no test specified\" && exit 1","build":"next build"}}"#,
        )
        .unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").unwrap();
        fs::write(root.join("go.mod"), "module example.test/project\n").unwrap();
        fs::write(
            root.join("pyproject.toml"),
            "[tool.ruff]\n[tool.pytest.ini_options]\n",
        )
        .unwrap();

        let commands = discover_validation_commands(&root, &root);
        let names = commands
            .iter()
            .map(|(name, _, _, _)| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            &names[..3],
            &["lint (pnpm)", "typecheck (pnpm)", "build (pnpm)"]
        );
        assert!(names.contains(&"Go tests"));
        assert!(names.contains(&"Python lint"));
        assert!(names.contains(&"Python tests"));
        assert!(!names.iter().any(|name| name.starts_with("test (pnpm)")));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn clean_candidate_can_only_bypass_missing_checks_with_explicit_override() {
        let repo = temp_repo("ditch-git-explicit-unvalidated-apply");
        fs::remove_file(repo.join(".ditch/validation.json")).unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let mut agent = coordinator
            .create(&project, AgentId::new(), "Unvalidated", &managed_root)
            .unwrap()
            .managed;
        fs::write(agent.path.join("feature.txt"), "feature\n").unwrap();
        coordinator.checkpoint(&project, &mut agent).unwrap();
        coordinator
            .prepare_integration(&project, &mut agent, &managed_root.join("validation"))
            .unwrap();
        assert_eq!(agent.validation_state, ValidationState::NotConfigured);
        assert_eq!(
            coordinator
                .apply(&project, &mut agent, false)
                .unwrap_err()
                .code,
            "integration_not_ready"
        );
        coordinator.apply(&project, &mut agent, true).unwrap();
        assert_eq!(
            fs::read_to_string(repo.join("feature.txt")).unwrap(),
            "feature\n"
        );
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn manual_conflict_resolution_stays_isolated_until_validated_and_applied() {
        let repo = temp_repo("ditch-git-manual-resolution");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let case_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()));
        let managed_root = case_root.join("worktrees");
        let mut agent = coordinator
            .create(&project, AgentId::new(), "Conflicting", &managed_root)
            .unwrap()
            .managed;
        fs::write(agent.path.join("base.txt"), "agent version\n").unwrap();
        coordinator.checkpoint(&project, &mut agent).unwrap();
        fs::write(repo.join("base.txt"), "target version\n").unwrap();
        git_ok(&repo, ["add", "base.txt"], None).unwrap();
        let tree = git_text(&repo, ["write-tree"], None).unwrap();
        let parent = rev_parse(&repo, "HEAD").unwrap();
        let advanced = commit_tree(&repo, &tree, &[&parent], "target change").unwrap();
        git_ok(&repo, ["reset", "--hard", &advanced], None).unwrap();
        coordinator
            .prepare_integration(&project, &mut agent, &case_root.join("validation"))
            .unwrap();
        assert_eq!(agent.conflict_state, ConflictState::GitConflict);

        let resolver = coordinator
            .create_conflict_resolution(
                &project,
                &mut agent,
                &case_root.join("resolution-worktrees"),
            )
            .unwrap();
        assert!(resolver.join("base.txt").is_file());
        assert_eq!(
            fs::read_to_string(repo.join("base.txt")).unwrap(),
            "target version\n"
        );
        fs::write(resolver.join("base.txt"), "resolved version\n").unwrap();
        coordinator
            .finalize_conflict_resolution(&project, &mut agent, &case_root.join("validation"))
            .unwrap();
        assert_eq!(agent.validation_state, ValidationState::Passed);
        assert_eq!(agent.status, WorktreeStatus::ReadyToApply);
        assert!(agent.resolution_path.is_none());
        coordinator.apply(&project, &mut agent, false).unwrap();
        assert_eq!(
            fs::read_to_string(repo.join("base.txt")).unwrap(),
            "resolved version\n"
        );
        fs::remove_dir_all(case_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn populated_unborn_repository_can_be_previewed_and_snapshotted_safely() {
        let repo = unique_temp_path("ditch-git-initial-snapshot");
        fs::create_dir_all(repo.join(".ditch")).unwrap();
        git_ok(&repo, ["init", "-b", "main"], None).unwrap();
        fs::write(repo.join("app.txt"), "hello\n").unwrap();
        fs::write(repo.join("file with space.txt"), "space\n").unwrap();
        fs::write(repo.join(".gitignore"), "ignored.log\n").unwrap();
        fs::write(repo.join("ignored.log"), "ignore me\n").unwrap();
        fs::write(repo.join(".ditch/project.json"), "{}\n").unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();

        let preview = coordinator.inspect_initial_snapshot(&project).unwrap();
        assert!(!preview.ready);
        assert!(!preview.unsafe_to_snapshot);
        assert_eq!(
            preview.included_paths,
            vec![".gitignore", "app.txt", "file with space.txt"]
        );
        let mut journal = Vec::new();
        let commit = coordinator
            .create_initial_snapshot(
                &project,
                preview.tree_oid.as_deref().unwrap(),
                |operation| {
                    journal.push(operation.clone());
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(rev_parse(&repo, "HEAD").unwrap(), commit);
        assert!(user_changed_paths(&repo).unwrap().is_empty());
        assert!(
            !git_text(&repo, ["show", "--format=", "--name-only", "HEAD"], None)
                .unwrap()
                .contains(".ditch")
        );
        assert_eq!(
            journal.last().unwrap().state,
            ProjectGitOperationState::Completed
        );
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn application_added_after_empty_head_is_saved_and_materialized_for_agent() {
        let repo = unique_temp_path("ditch-git-empty-head-later-populated");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, ["init", "-b", "main"], None).unwrap();
        let empty_tree = git_text_with_input(&repo, ["mktree"], b"").unwrap();
        let empty_commit = commit_tree(&repo, &empty_tree, &[], "empty baseline").unwrap();
        git_ok(
            &repo,
            [
                "update-ref",
                "refs/heads/main",
                &empty_commit,
                "0000000000000000000000000000000000000000",
            ],
            None,
        )
        .unwrap();
        git_ok(&repo, ["read-tree", "--reset", &empty_commit], None).unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/app.rs"), "fn main() {}\n").unwrap();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname='app'\nversion='0.1.0'\n",
        )
        .unwrap();

        let project = Project::new("Later populated", &repo);
        let coordinator = GitCoordinator::default();
        let preview = coordinator.inspect_initial_snapshot(&project).unwrap();
        assert!(!preview.ready);
        assert_eq!(preview.target_oid.as_deref(), Some(empty_commit.as_str()));
        let rejected_root = repo
            .parent()
            .unwrap()
            .join(format!("rejected-{}", uuid::Uuid::new_v4()));
        assert_eq!(
            coordinator
                .create(
                    &project,
                    AgentId::new(),
                    "must not start incomplete",
                    &rejected_root,
                )
                .unwrap_err()
                .code,
            "project_snapshot_required"
        );
        let prepared = coordinator
            .create_initial_snapshot(&project, preview.tree_oid.as_deref().unwrap(), |_| Ok(()))
            .unwrap();
        assert_eq!(rev_parse(&repo, "HEAD").unwrap(), prepared);

        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()));
        let created = coordinator
            .create(
                &project,
                AgentId::new(),
                "Dockerize this app",
                &managed_root,
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(created.agent_cwd.join("src/app.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert!(created.agent_cwd.join("Cargo.toml").is_file());
        assert_eq!(rev_parse(&created.agent_cwd, "HEAD").unwrap(), prepared);

        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(rejected_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn staged_manual_work_requires_review_and_active_git_operations_block_launch() {
        let repo = temp_repo("ditch-git-launch-hazards");
        let project = Project::new("Hazards", &repo);
        let coordinator = GitCoordinator::default();
        fs::write(repo.join("base.txt"), "staged manual edit\n").unwrap();
        git_ok(&repo, ["add", "base.txt"], None).unwrap();

        let staged = coordinator.inspect_initial_snapshot(&project).unwrap();
        assert!(!staged.ready);
        assert!(!staged.unsafe_to_snapshot);
        assert!(
            staged
                .warnings
                .iter()
                .any(|warning| warning.contains("staged in Terminal"))
        );

        let merge_head = git_text(&repo, ["rev-parse", "HEAD"], None).unwrap();
        let marker = git_text(&repo, ["rev-parse", "--git-path", "MERGE_HEAD"], None).unwrap();
        let marker = if Path::new(&marker).is_absolute() {
            PathBuf::from(marker)
        } else {
            repo.join(marker)
        };
        fs::write(&marker, format!("{merge_head}\n")).unwrap();
        let merging = coordinator.inspect_initial_snapshot(&project).unwrap();
        assert!(merging.unsafe_to_snapshot);
        assert!(
            merging
                .warnings
                .iter()
                .any(|warning| warning.contains("merge is currently in progress"))
        );

        fs::remove_file(marker).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn initial_snapshot_rejects_a_changed_preview_without_creating_history() {
        let repo = unique_temp_path("ditch-git-initial-race");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, ["init", "-b", "main"], None).unwrap();
        fs::write(repo.join("app.txt"), "first\n").unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let preview = coordinator.inspect_initial_snapshot(&project).unwrap();
        fs::write(repo.join("app.txt"), "changed\n").unwrap();

        let error = coordinator
            .create_initial_snapshot(&project, preview.tree_oid.as_deref().unwrap(), |_| Ok(()))
            .unwrap_err();
        assert_eq!(error.code, "initial_snapshot_changed");
        assert!(rev_parse(&repo, "HEAD").is_err());
        assert_eq!(
            fs::read_to_string(repo.join("app.txt")).unwrap(),
            "changed\n"
        );
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn initial_snapshot_preview_warns_about_credentials_and_blocks_nested_repositories() {
        let repo = unique_temp_path("ditch-git-initial-warnings");
        fs::create_dir_all(repo.join("vendor/nested")).unwrap();
        git_ok(&repo, ["init", "-b", "main"], None).unwrap();
        fs::write(repo.join(".env"), "TOKEN=test-only\n").unwrap();
        git_ok(&repo.join("vendor/nested"), ["init", "-b", "main"], None).unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();

        let preview = coordinator.inspect_initial_snapshot(&project).unwrap();
        assert!(preview.unsafe_to_snapshot);
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.contains(".env"))
        );
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.contains("nested Git repository"))
        );
        let error = coordinator
            .create_initial_snapshot(&project, "unsafe-preview", |_| Ok(()))
            .unwrap_err();
        assert_eq!(error.code, "initial_snapshot_unsafe");
        assert!(rev_parse(&repo, "HEAD").is_err());
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn interrupted_initial_snapshot_finishes_from_its_journaled_commit() {
        let repo = unique_temp_path("ditch-git-initial-recovery");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, ["init", "-b", "main"], None).unwrap();
        fs::write(repo.join("app.txt"), "hello\n").unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let preview = coordinator.inspect_initial_snapshot(&project).unwrap();
        let tree = preview.tree_oid.unwrap();
        let commit = commit_tree(&repo, &tree, &[], "interrupted snapshot").unwrap();
        let now = Utc::now();
        let mut operation = ProjectGitOperation {
            id: ProjectGitOperationId::new(),
            project_id: project.id,
            kind: "initial_snapshot".into(),
            target_branch: "main".into(),
            expected_tree_oid: tree,
            expected_old_oid: None,
            created_commit_oid: Some(commit.clone()),
            state: ProjectGitOperationState::CommitCreated,
            last_error: None,
            created_at: now,
            updated_at: now,
        };

        coordinator
            .reconcile_initial_snapshot(&project, &mut operation)
            .unwrap();
        assert_eq!(operation.state, ProjectGitOperationState::Completed);
        assert_eq!(rev_parse(&repo, "HEAD").unwrap(), commit);
        assert!(user_changed_paths(&repo).unwrap().is_empty());
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn creates_isolated_locked_worktrees_and_checkpoints_dirty_state() {
        let repo = temp_repo("ditch-git-create");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo.parent().unwrap().join("create-case").join("worktrees");
        let created = coordinator
            .create(&project, AgentId::new(), "Build auth", &managed_root)
            .unwrap();
        fs::write(created.managed.path.join("auth.txt"), "secret\n").unwrap();
        assert!(!repo.join("auth.txt").exists());
        let mut managed = created.managed;
        coordinator.refresh(&project, &mut managed).unwrap();
        assert_eq!(managed.changed_paths, vec!["auth.txt"]);
        let checkpoint = coordinator.checkpoint(&project, &mut managed).unwrap();
        assert_eq!(rev_parse(&managed.path, &checkpoint).unwrap(), checkpoint);
        assert!(
            !git_bytes(
                &managed.path,
                ["diff", "--cached", "--name-only", "-z"],
                None
            )
            .unwrap()
            .starts_with(b"auth.txt")
        );
        fs::remove_dir_all(managed_root.parent().unwrap()).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn merge_tree_quarantines_same_file_conflict() {
        let repo = temp_repo("ditch-git-conflict");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join("conflict-case")
            .join("worktrees");
        let mut a = coordinator
            .create(&project, AgentId::new(), "A", &managed_root)
            .unwrap()
            .managed;
        let mut b = coordinator
            .create(&project, AgentId::new(), "B", &managed_root)
            .unwrap()
            .managed;
        fs::write(a.path.join("base.txt"), "agent a\n").unwrap();
        fs::write(b.path.join("base.txt"), "agent b\n").unwrap();
        coordinator.checkpoint(&project, &mut a).unwrap();
        coordinator.checkpoint(&project, &mut b).unwrap();
        coordinator
            .prepare_integration(&project, &mut a, &managed_root.join("validation"))
            .unwrap();
        coordinator.apply(&project, &mut a, false).unwrap();
        coordinator
            .prepare_integration(&project, &mut b, &managed_root.join("validation"))
            .unwrap();
        assert_eq!(b.status, WorktreeStatus::ConflictRisk);
        assert_eq!(
            fs::read_to_string(repo.join("base.txt")).unwrap(),
            "agent a\n"
        );
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn independent_agents_integrate_sequentially_from_the_same_base() {
        let repo = temp_repo("ditch-git-independent");
        fs::create_dir_all(repo.join(".ditch")).unwrap();
        fs::write(repo.join(".ditch/project.json"), "{}\n").unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let mut a = coordinator
            .create(&project, AgentId::new(), "Authentication", &managed_root)
            .unwrap()
            .managed;
        let mut b = coordinator
            .create(&project, AgentId::new(), "Billing", &managed_root)
            .unwrap()
            .managed;
        fs::write(a.path.join("auth.txt"), "auth\n").unwrap();
        fs::write(b.path.join("billing.txt"), "billing\n").unwrap();
        coordinator.checkpoint(&project, &mut a).unwrap();
        coordinator.checkpoint(&project, &mut b).unwrap();
        coordinator
            .prepare_integration(&project, &mut a, &managed_root.join("validation"))
            .unwrap();
        coordinator.apply(&project, &mut a, false).unwrap();
        coordinator.archive_integrated(&project, &mut a).unwrap();
        coordinator
            .prepare_integration(&project, &mut b, &managed_root.join("validation"))
            .unwrap();
        coordinator.apply(&project, &mut b, false).unwrap();
        assert_eq!(fs::read_to_string(repo.join("auth.txt")).unwrap(), "auth\n");
        assert_eq!(
            fs::read_to_string(repo.join("billing.txt")).unwrap(),
            "billing\n"
        );
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn target_advance_invalidates_a_prepared_candidate() {
        let repo = temp_repo("ditch-git-target-advance");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let mut agent = coordinator
            .create(&project, AgentId::new(), "Feature", &managed_root)
            .unwrap()
            .managed;
        fs::write(agent.path.join("feature.txt"), "feature\n").unwrap();
        coordinator.checkpoint(&project, &mut agent).unwrap();
        coordinator
            .prepare_integration(&project, &mut agent, &managed_root.join("validation"))
            .unwrap();
        fs::write(repo.join("manual.txt"), "manual\n").unwrap();
        git_ok(&repo, ["add", "manual.txt"], None).unwrap();
        let tree = git_text(&repo, ["write-tree"], None).unwrap();
        let parent = rev_parse(&repo, "HEAD").unwrap();
        let advanced = commit_tree(&repo, &tree, &[&parent], "manual advance").unwrap();
        git_ok(&repo, ["reset", "--hard", &advanced], None).unwrap();
        let error = coordinator.apply(&project, &mut agent, false).unwrap_err();
        assert_eq!(error.code, "target_advanced");
        assert!(!repo.join("feature.txt").exists());
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn dirty_canonical_workspace_blocks_apply_without_touching_user_work() {
        let repo = temp_repo("ditch-git-dirty-canonical");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let mut agent = coordinator
            .create(&project, AgentId::new(), "Feature", &managed_root)
            .unwrap()
            .managed;
        fs::write(agent.path.join("feature.txt"), "feature\n").unwrap();
        coordinator.checkpoint(&project, &mut agent).unwrap();
        coordinator
            .prepare_integration(&project, &mut agent, &managed_root.join("validation"))
            .unwrap();
        fs::write(repo.join("base.txt"), "user work\n").unwrap();
        let error = coordinator.apply(&project, &mut agent, false).unwrap_err();
        assert_eq!(error.code, "canonical_workspace_dirty");
        assert_eq!(
            fs::read_to_string(repo.join("base.txt")).unwrap(),
            "user work\n"
        );
        assert!(!repo.join("feature.txt").exists());
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn rename_then_modify_original_is_quarantined() {
        let repo = temp_repo("ditch-git-rename-conflict");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let mut rename = coordinator
            .create(&project, AgentId::new(), "Rename", &managed_root)
            .unwrap()
            .managed;
        let mut modify = coordinator
            .create(&project, AgentId::new(), "Modify", &managed_root)
            .unwrap()
            .managed;
        fs::rename(
            rename.path.join("base.txt"),
            rename.path.join("renamed.txt"),
        )
        .unwrap();
        fs::remove_file(modify.path.join("base.txt")).unwrap();
        coordinator.checkpoint(&project, &mut rename).unwrap();
        coordinator.checkpoint(&project, &mut modify).unwrap();
        coordinator
            .prepare_integration(&project, &mut rename, &managed_root.join("validation"))
            .unwrap();
        coordinator.apply(&project, &mut rename, false).unwrap();
        coordinator
            .archive_integrated(&project, &mut rename)
            .unwrap();
        coordinator
            .prepare_integration(&project, &mut modify, &managed_root.join("validation"))
            .unwrap();
        assert_eq!(modify.status, WorktreeStatus::ConflictRisk);
        assert!(repo.join("renamed.txt").exists());
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn foreign_worktree_is_never_removed_with_a_ditch_workspace() {
        let repo = temp_repo("ditch-git-foreign");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let foreign = repo
            .parent()
            .unwrap()
            .join(format!("foreign-{}", uuid::Uuid::new_v4()));
        git_ok(
            &repo,
            [
                "worktree",
                "add",
                "-b",
                "user/foreign",
                foreign.to_string_lossy().as_ref(),
                "HEAD",
            ],
            None,
        )
        .unwrap();
        let mut managed = coordinator
            .create(&project, AgentId::new(), "Ditch", &managed_root)
            .unwrap()
            .managed;
        coordinator.discard(&project, &mut managed, true).unwrap();
        assert!(foreign.join("base.txt").exists());
        let listed = parse_worktree_porcelain(
            &git_bytes(&repo, ["worktree", "list", "--porcelain", "-z"], None).unwrap(),
        )
        .unwrap();
        assert!(
            listed
                .iter()
                .any(|item| canonical_or_original(&item.path) == canonical_or_original(&foreign))
        );
        git_ok(
            &repo,
            ["worktree", "remove", foreign.to_string_lossy().as_ref()],
            None,
        )
        .unwrap();
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn clean_unborn_repository_gets_a_deterministic_baseline_without_git_identity() {
        let repo = unique_temp_path("ditch-git-unborn");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, ["init", "-b", "main"], None).unwrap();
        let project = Project::new("Empty", &repo);
        let coordinator = GitCoordinator::default();
        let managed_root = repo
            .parent()
            .unwrap()
            .join(format!("case-{}", uuid::Uuid::new_v4()))
            .join("worktrees");
        let created = coordinator
            .create(&project, AgentId::new(), "First task", &managed_root)
            .unwrap();
        assert_eq!(
            created.managed.base_commit_oid,
            rev_parse(&repo, "HEAD").unwrap()
        );
        assert!(created.managed.path.is_dir());
        fs::remove_dir_all(managed_root).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn creation_journal_recovers_when_git_succeeds_before_final_persistence() {
        let repo = temp_repo("ditch-git-create-recovery");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let root = repo
            .parent()
            .unwrap()
            .join("recovery-case")
            .join("worktrees");
        let mut journaled = None;
        let created = coordinator
            .create_journaled(&project, AgentId::new(), "Recover", &root, |worktree| {
                journaled = Some(worktree.clone());
                Ok(())
            })
            .unwrap();
        let mut restored = journaled.unwrap();
        assert_eq!(restored.status, WorktreeStatus::Creating);
        coordinator
            .reconcile_operation(&project, &mut restored)
            .unwrap();
        assert_eq!(restored.status, WorktreeStatus::Ready);
        assert_eq!(restored.path, created.managed.path);
        coordinator.discard(&project, &mut restored, true).unwrap();
        fs::remove_dir_all(root.parent().unwrap()).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn different_lines_are_conservatively_reserved_but_git_can_merge_them() {
        let repo = temp_repo("ditch-git-different-lines");
        let base = "one\n2\n3\n4\n5\n6\n7\n8\n9\nten\n";
        fs::write(repo.join("base.txt"), base).unwrap();
        git_ok(&repo, ["add", "base.txt"], None).unwrap();
        let tree = git_text(&repo, ["write-tree"], None).unwrap();
        let parent = rev_parse(&repo, "HEAD").unwrap();
        let baseline = commit_tree(&repo, &tree, &[&parent], "two lines").unwrap();
        git_ok(&repo, ["reset", "--hard", &baseline], None).unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let root = repo.parent().unwrap().join("lines-case").join("worktrees");
        let mut a = coordinator
            .create(&project, AgentId::new(), "A", &root)
            .unwrap()
            .managed;
        let mut b = coordinator
            .create(&project, AgentId::new(), "B", &root)
            .unwrap()
            .managed;
        fs::write(a.path.join("base.txt"), base.replacen("one", "ONE", 1)).unwrap();
        fs::write(b.path.join("base.txt"), base.replacen("ten", "TEN", 1)).unwrap();
        coordinator.refresh(&project, &mut a).unwrap();
        coordinator.refresh(&project, &mut b).unwrap();
        let mut projection = vec![a.clone(), b.clone()];
        apply_overlap_projection(&mut projection);
        assert!(
            projection
                .iter()
                .all(|item| item.overlap_risk == OverlapRisk::Path)
        );
        assert!(
            projection
                .iter()
                .all(|item| item.overlapping_paths == ["base.txt"])
        );
        assert!(projection.iter().all(|item| {
            matches!(item.status, WorktreeStatus::Dirty | WorktreeStatus::Active)
                && item.integration_state != IntegrationState::Blocked
        }));
        coordinator.checkpoint(&project, &mut a).unwrap();
        coordinator.checkpoint(&project, &mut b).unwrap();
        coordinator
            .prepare_integration(&project, &mut a, &root.join("validation"))
            .unwrap();
        let review = coordinator.review(&project, &a).unwrap();
        assert_eq!(review.checkpoint_oid, a.checkpoint_oid.clone().unwrap());
        assert!(!review.candidate_is_stale);
        assert_eq!(review.agent_changes.len(), 1);
        assert_eq!(review.combined_changes.len(), 1);
        coordinator.apply(&project, &mut a, false).unwrap();
        coordinator.archive_integrated(&project, &mut a).unwrap();
        coordinator
            .prepare_integration(&project, &mut b, &root.join("validation"))
            .unwrap();
        assert_eq!(b.status, WorktreeStatus::ReadyToApply);
        fs::remove_dir_all(root.parent().unwrap()).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn two_different_new_files_at_the_same_path_conflict_before_apply() {
        let repo = temp_repo("ditch-git-new-file-conflict");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let root = repo
            .parent()
            .unwrap()
            .join("new-file-case")
            .join("worktrees");
        let mut a = coordinator
            .create(&project, AgentId::new(), "A", &root)
            .unwrap()
            .managed;
        let mut b = coordinator
            .create(&project, AgentId::new(), "B", &root)
            .unwrap()
            .managed;
        fs::write(a.path.join("new.txt"), "A\n").unwrap();
        fs::write(b.path.join("new.txt"), "B\n").unwrap();
        coordinator.checkpoint(&project, &mut a).unwrap();
        coordinator.checkpoint(&project, &mut b).unwrap();
        coordinator
            .prepare_integration(&project, &mut a, &root.join("validation"))
            .unwrap();
        coordinator.apply(&project, &mut a, false).unwrap();
        coordinator.archive_integrated(&project, &mut a).unwrap();
        coordinator
            .prepare_integration(&project, &mut b, &root.join("validation"))
            .unwrap();
        assert_eq!(b.status, WorktreeStatus::ConflictRisk);
        assert_eq!(fs::read_to_string(repo.join("new.txt")).unwrap(), "A\n");
        fs::remove_dir_all(root.parent().unwrap()).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn clean_merge_with_failed_combined_validation_cannot_apply() {
        let repo = temp_repo("ditch-git-validation-failure");
        fs::create_dir_all(repo.join(".ditch")).unwrap();
        fs::write(
            repo.join(".ditch/validation.json"),
            r#"{"commands":[["sh","-c","exit 7"]]}"#,
        )
        .unwrap();
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let root = repo
            .parent()
            .unwrap()
            .join("validation-case")
            .join("worktrees");
        let mut agent = coordinator
            .create(&project, AgentId::new(), "Feature", &root)
            .unwrap()
            .managed;
        fs::write(agent.path.join("feature.txt"), "feature\n").unwrap();
        coordinator.checkpoint(&project, &mut agent).unwrap();
        coordinator
            .prepare_integration(&project, &mut agent, &root.join("validation"))
            .unwrap();
        assert_eq!(agent.validation_state, ValidationState::Failed);
        assert_eq!(agent.status, WorktreeStatus::NeedsReview);
        assert_eq!(
            coordinator
                .apply(&project, &mut agent, false)
                .unwrap_err()
                .code,
            "integration_not_ready"
        );
        assert!(!repo.join("feature.txt").exists());
        fs::remove_dir_all(root.parent().unwrap()).ok();
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn manually_deleted_managed_directory_becomes_recovery_needed() {
        let repo = temp_repo("ditch-git-manual-delete");
        let project = Project::new("Repo", &repo);
        let coordinator = GitCoordinator::default();
        let root = repo.parent().unwrap().join("delete-case").join("worktrees");
        let mut managed = coordinator
            .create(&project, AgentId::new(), "Task", &root)
            .unwrap()
            .managed;
        fs::remove_dir_all(&managed.path).unwrap();
        coordinator.refresh(&project, &mut managed).unwrap();
        assert_eq!(managed.status, WorktreeStatus::RecoveryNeeded);
        assert_eq!(managed.lock_state, WorktreeLockState::Missing);
        git_ok(&repo, ["worktree", "prune"], None).unwrap();
        fs::remove_dir_all(root.parent().unwrap()).ok();
        fs::remove_dir_all(repo).ok();
    }
}
