use ditch_core::{
    ManagedSkill, SkillBinding, SkillEntry, SkillFile, SkillInstallPlan, SkillOperation,
    SkillRequest, SkillResponse, SkillSource,
};
use sha2::{Digest as _, Sha256};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SkillCatalog {
    sources: Vec<SkillSource>,
    installed: Vec<ManagedSkill>,
    plans: Vec<SkillInstallPlan>,
}
impl Default for SkillCatalog {
    fn default() -> Self {
        Self {
            sources: vec![SkillSource {
                id: Uuid::from_u128(0x44544348535000000000000000000001),
                name: "Superpowers".into(),
                location: "https://github.com/obra/superpowers.git".into(),
                reference: "HEAD".into(),
                seeded: true,
            }],
            installed: vec![],
            plans: vec![],
        }
    }
}
fn skill_catalog(state: &RuntimeState) -> Result<SkillCatalog, String> {
    state
        .store
        .setting("ditch.skills.catalog.v1")
        .map_err(|e| e.to_string())?
        .map(|s| {
            serde_json::from_str(&s).map_err(|e| {
                format!("Skill catalog is unreadable; retained without overwriting: {e}")
            })
        })
        .unwrap_or_else(|| Ok(SkillCatalog::default()))
}
fn save_skill_catalog(state: &mut RuntimeState, catalog: &SkillCatalog) -> Result<(), String> {
    state
        .store
        .set_setting(
            "ditch.skills.catalog.v1",
            &serde_json::to_string(catalog).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
}
fn skill_root(state: &RuntimeState) -> PathBuf {
    fs::canonicalize(&state.paths.data_dir)
        .unwrap_or_else(|_| state.paths.data_dir.clone())
        .join("skills")
}
fn skill_identity(source: &str, path: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(source.as_bytes());
    hash.update([0]);
    hash.update(path.as_bytes());
    format!("{:x}", hash.finalize())
}
fn selected_skill_roots(skills: &[SkillBinding]) -> Vec<PathBuf> {
    let mut roots = skills
        .iter()
        .filter_map(|s| s.path.parent()?.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}
fn catalog_roots(catalog: &SkillCatalog) -> Vec<PathBuf> {
    let mut roots = catalog
        .sources
        .iter()
        .filter(|s| Path::new(&s.location).is_absolute())
        .map(|s| PathBuf::from(&s.location))
        .collect::<Vec<_>>();
    roots.extend(
        catalog
            .installed
            .iter()
            .filter(|s| s.enabled)
            .filter_map(|s| {
                s.versions
                    .get(s.current)?
                    .entry
                    .path
                    .parent()?
                    .parent()
                    .map(Path::to_path_buf)
            }),
    );
    roots.sort();
    roots.dedup();
    roots
}
fn skill_client(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
) -> Result<app_server_client::Client, String> {
    let binary = active_codex_binary(state).ok_or(
        "Codex is unavailable. Install a compatible CLI or use legacy execution without skills.",
    )?;
    let home = state.lock().unwrap().codex_home.clone();
    app_server_client::Client::open(&binary, &project.root, home.as_deref(), None)
        .map_err(|e| e.to_string())
}
fn dependency_errors(value: &serde_json::Value) -> Vec<String> {
    value
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let kind = tool["type"].as_str().unwrap_or("unknown");
            let name = tool["value"].as_str().unwrap_or("unspecified");
            let satisfied = match kind {
                "env_var" => std::env::var_os(name).is_some_and(|v| !v.is_empty()),
                "command" | "binary" => {
                    !name.contains(['/', ' ', '\n'])
                        && std::env::var_os("PATH").is_some_and(|p| {
                            std::env::split_paths(&p).any(|dir| dir.join(name).is_file())
                        })
                }
                _ => false,
            };
            (!satisfied).then(|| format!("Missing or unverified {kind} dependency: {name}"))
        })
        .collect()
}
fn discover_skills(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
) -> Result<(Vec<SkillEntry>, Vec<String>), String> {
    let catalog = skill_catalog(&state.lock().unwrap())?;
    let mut client = skill_client(state, project)?;
    client
        .set_roots(&catalog_roots(&catalog))
        .map_err(|e| format!("Extra skill roots unsupported: {e}"))?;
    let value = client
        .call(
            "skills/list",
            serde_json::json!({"cwds":[project.root],"forceReload":true}),
        )
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let data = value["data"]
        .as_array()
        .ok_or("Malformed skills/list response")?;
    for group in data {
        if group["cwd"]
            .as_str()
            .map(Path::new)
            .map(canonical_project_root)
            != Some(canonical_project_root(&project.root))
        {
            continue;
        }
        errors.extend(group["errors"].as_array().into_iter().flatten().map(|e| {
            e["message"]
                .as_str()
                .unwrap_or("Skill discovery error")
                .to_owned()
        }));
        for raw in group["skills"].as_array().into_iter().flatten() {
            if entries.len() >= 4096 {
                return Err("Skill catalog exceeds discovery limit".into());
            }
            let Some(path) = raw["path"].as_str() else {
                errors.push("Skill omitted its path".into());
                continue;
            };
            let canonical = match fs::canonicalize(path) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(format!("{path}: {e}"));
                    continue;
                }
            };
            if canonical.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
                errors.push("Provider returned a non-SKILL.md path".into());
                continue;
            }
            let scope = raw["scope"].as_str().unwrap_or("unknown").to_owned();
            let managed = catalog.installed.iter().find(|s| {
                s.versions
                    .get(s.current)
                    .is_some_and(|v| v.entry.path == canonical)
            });
            let mut entry = SkillEntry {
                source_id: managed.map(|s| s.source.id),
                relative_path: managed.map(|s| s.relative_path.clone()),
                identity: managed
                    .map(|s| s.identity.clone())
                    .unwrap_or_else(|| skill_identity(&scope, &canonical.to_string_lossy())),
                name: raw["name"].as_str().unwrap_or("Unnamed").into(),
                description: raw["description"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(4096)
                    .collect(),
                path: canonical.clone(),
                scope,
                source: managed
                    .map(|s| s.source.location.clone())
                    .unwrap_or_else(|| {
                        raw["pluginId"].as_str().unwrap_or("Codex discovery").into()
                    }),
                enabled: raw["enabled"].as_bool().unwrap_or(false)
                    && managed.is_none_or(|s| s.enabled),
                recognized: true,
                content_hash: String::new(),
                revision: managed.and_then(|s| s.versions[s.current].resolved_revision.clone()),
                interface: raw["interface"].clone(),
                dependencies: raw["dependencies"].clone(),
                missing_dependencies: dependency_errors(&raw["dependencies"]),
                license: managed.and_then(|s| s.versions[s.current].entry.license.clone()),
                has_scripts: false,
                validation_error: None,
                managed: managed.is_some(),
            };
            match skill_files::read_tree(canonical.parent().unwrap())
                .and_then(|f| skill_files::validate_files(&f))
            {
                Ok((hash, _, _, scripts)) => {
                    if managed.is_some_and(|s| s.versions[s.current].entry.content_hash != hash) {
                        entry.validation_error = Some("Installed revision changed on disk; restore or explicitly update it before assignment".into());
                    }
                    entry.content_hash = hash;
                    entry.has_scripts = scripts;
                }
                Err(e) => entry.validation_error = Some(e.to_string()),
            }
            entries.push(entry);
        }
    }
    // Disabled managed revisions remain visible without exposing their roots to Codex.
    for managed in &catalog.installed {
        if !entries.iter().any(|e| e.identity == managed.identity) {
            let mut entry = managed.versions[managed.current].entry.clone();
            entry.enabled = managed.enabled;
            entry.recognized = false;
            if entry.enabled {
                entry.validation_error=Some("Codex did not recognize this revision; refresh or check the selected installation".into());
            }
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| a.identity.cmp(&b.identity));
    state
        .lock()
        .unwrap()
        .skill_discovery
        .insert(project.id, entries.clone());
    Ok((entries, errors))
}
fn validate_selected_skills(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    skills: &[SkillBinding],
) -> Result<(), String> {
    if skills.is_empty() {
        return Ok(());
    }
    let (entries, _) = discover_skills(state, project)?;
    let catalog = skill_catalog(&state.lock().unwrap())?;
    let mut identities = std::collections::HashSet::new();
    for binding in skills {
        if !identities.insert(&binding.identity) {
            return Err("Duplicate selected skill identity".into());
        }
        let pinned = catalog
            .installed
            .iter()
            .find(|s| s.identity == binding.identity && s.enabled)
            .and_then(|s| {
                s.versions.iter().find(|v| {
                    v.entry.content_hash == binding.content_hash && v.entry.path == binding.path
                })
            });
        if let Some(pinned) = pinned {
            verify_skill_plan_files(pinned)?;
            let metadata = skill_path_metadata(state, project, &pinned.entry.path)?;
            let missing = dependency_errors(&metadata["dependencies"]);
            if !missing.is_empty() {
                return Err(missing.join("; "));
            }
            if metadata["name"].as_str() != Some(binding.name.as_str()) {
                return Err("Selected skill name changed; reselect the revision".into());
            }
            continue;
        }
        let entry=entries.iter().find(|e|e.identity==binding.identity&&e.path==binding.path).ok_or_else(||format!("{} is missing on this execution target; explicitly sync or select its exact revision",binding.name))?;
        if !entry.enabled || !entry.recognized {
            return Err(format!("{} is disabled or unavailable", entry.name));
        }
        if let Some(error) = &entry.validation_error {
            return Err(format!("{}: {error}", entry.name));
        }
        if entry.content_hash != binding.content_hash || entry.name != binding.name {
            return Err(format!(
                "{} changed since selection. Review and accept the new revision before starting.",
                entry.name
            ));
        }
        if !entry.missing_dependencies.is_empty() {
            return Err(entry.missing_dependencies.join("; "));
        }
    }
    Ok(())
}
fn page<T: Clone>(items: &[T], offset: usize, limit: usize) -> (Vec<T>, Option<usize>) {
    let limit = limit.clamp(1, 100);
    let end = offset.saturating_add(limit).min(items.len());
    (
        items.get(offset..end).unwrap_or_default().to_vec(),
        (end < items.len()).then_some(end),
    )
}
fn skill_error(message: impl Into<String>) -> ServerResponse {
    ServerResponse::SkillResponse(SkillResponse::Error {
        code: "skill_operation_failed".into(),
        message: message.into(),
    })
}
fn handle_skill_request(state: Arc<Mutex<RuntimeState>>, request: SkillRequest) -> ServerResponse {
    if !matches!(request.operation, SkillOperation::Sync { .. })
        && let Some(project) = request
            .project_id
            .and_then(|id| project_by_id(&state, id))
            .filter(Project::is_remote)
    {
        let alias = ssh_remote::remote_alias(&project).unwrap();
        let connections = state.lock().unwrap().remote_connections.clone();
        match connections.request(alias, ClientRequest::RuntimeStatus) {
            Ok(ServerResponse::RuntimeStatus(status))
                if status.capabilities.iter().any(|c| c == "skills_v1") => {}
            _ => {
                return skill_error(
                    "This SSH runtime cannot serve skills; install the matching runtime.",
                );
            }
        }
        return connections
            .request(alias, ClientRequest::SkillRequest(request))
            .unwrap_or_else(|e| skill_error(format!("SSH skills unavailable: {e}")));
    }
    match skill_operation(&state, request) {
        Ok(response) => ServerResponse::SkillResponse(response),
        Err(error) => skill_error(error),
    }
}
fn skill_operation(
    state: &Arc<Mutex<RuntimeState>>,
    request: SkillRequest,
) -> Result<SkillResponse, String> {
    let project = request.project_id.and_then(|id| project_by_id(state, id));
    let require_project = || {
        project
            .clone()
            .ok_or_else(|| "Select a registered project to discover its skills.".to_owned())
    };
    match request.operation {
        SkillOperation::Capabilities => {
            let project = require_project()?;
            let remote = state.lock().unwrap().remote_runtime;
            let mut client = skill_client(state, &project)?;
            let roots = client.call(
                "skills/extraRoots/set",
                serde_json::json!({"extraRoots":[]}),
            );
            let policy = codex_app_server::execution_policy(
                &AgentExecutionProfile::default(),
                &project.root,
                false,
            )
            .2;
            let sandbox=client.call("command/exec",serde_json::json!({"command":["/usr/bin/true"],"cwd":project.root,"sandboxPolicy":policy,"timeoutMs":5000}));
            let supported = sandbox.as_ref().is_ok_and(|value| value["exitCode"] == 0);
            let mut details = Vec::new();
            if let Err(error) = &roots {
                details.push(error.to_string());
            }
            if let Err(error) = &sandbox {
                details.push(error.to_string());
            }
            if remote {
                details.push("SSH retains guarded, unconfined execution in this release even when the sandbox probe succeeds. Every approval is user-reviewed; account permissions define its boundary.".into());
            }
            Ok(SkillResponse::Capabilities {
                app_server: true,
                extra_roots: roots.is_ok(),
                sandbox_supported: supported,
                selected_boundary: if remote {
                    "SSH account (unconfined, guarded approvals)"
                } else {
                    "Canonical project root (Full Access is a separate explicit choice)"
                }
                .into(),
                details,
            })
        }
        SkillOperation::List {
            offset,
            limit,
            source,
            refresh: _,
        } => {
            let project = require_project()?;
            let (mut entries, errors) = discover_skills(state, &project)?;
            if let Some(source) = source {
                entries.retain(|e| e.scope == source || e.source == source);
            }
            let (entries, next_offset) = page(&entries, offset, limit);
            Ok(SkillResponse::Entries {
                entries,
                next_offset,
                errors,
                app_server: true,
            })
        }
        SkillOperation::Preview { identity } => {
            let project = require_project()?;
            let (entries, _) = discover_skills(state, &project)?;
            let entry = entries
                .into_iter()
                .find(|e| e.identity == identity)
                .ok_or("Skill no longer available")?;
            let bytes = skill_files::read_bounded(&entry.path, 65536).map_err(|e| e.to_string())?;
            let instructions = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            Ok(SkillResponse::Preview {
                entry,
                instructions,
            })
        }
        SkillOperation::Sources { offset, limit } => {
            let catalog = skill_catalog(&state.lock().unwrap())?;
            let (sources, next_offset) = page(&catalog.sources, offset, limit);
            Ok(SkillResponse::Sources {
                sources,
                next_offset,
            })
        }
        SkillOperation::AddSource {
            name,
            location,
            reference,
        } => {
            validate_skill_source(&location, &reference)?;
            if name.trim().is_empty() || name.len() > 200 {
                return Err("Source name is required (at most 200 characters)".into());
            }
            let mut locked = state.lock().unwrap();
            let mut catalog = skill_catalog(&locked)?;
            if catalog.sources.len() >= 100
                || catalog
                    .sources
                    .iter()
                    .any(|s| s.location == location && s.reference == reference)
            {
                return Err("Duplicate source or source limit reached".into());
            }
            catalog.sources.push(SkillSource {
                id: Uuid::new_v4(),
                name,
                location,
                reference,
                seeded: false,
            });
            save_skill_catalog(&mut locked, &catalog)?;
            Ok(SkillResponse::Accepted)
        }
        SkillOperation::RemoveSource { source_id } => {
            let mut locked = state.lock().unwrap();
            let mut catalog = skill_catalog(&locked)?;
            if catalog
                .sources
                .iter()
                .any(|s| s.id == source_id && s.seeded)
            {
                return Err("The seeded source is metadata only and cannot be removed".into());
            }
            catalog.sources.retain(|s| s.id != source_id);
            save_skill_catalog(&mut locked, &catalog)?;
            Ok(SkillResponse::Accepted)
        }
        SkillOperation::BrowseSource {
            source_id,
            allow_network,
            offset,
            limit,
        } => {
            let source = source_by_id(state, source_id)?;
            let fetched = fetch_skill_source(state, &source, allow_network)?;
            let paths = source_skill_paths(&fetched)?;
            let (paths, next_offset) = page(&paths, offset, limit);
            let mut entries = Vec::new();
            let mut errors = Vec::new();
            for relative in paths {
                match source_skill_entry(&source, &fetched, &relative) {
                    Ok((entry, _)) => entries.push(entry),
                    Err(e) => errors.push(format!("{relative}: {e}")),
                }
            }
            Ok(SkillResponse::Entries {
                entries,
                next_offset,
                errors,
                app_server: true,
            })
        }
        SkillOperation::PrepareInstall {
            source_id,
            relative_path,
            allow_network,
        } => {
            let project = require_project()?;
            let source = source_by_id(state, source_id)?;
            let fetched = fetch_skill_source(state, &source, allow_network)?;
            let (entry, files) = source_skill_entry(&source, &fetched, &relative_path)?;
            let plan = prepare_skill_plan(
                state,
                &project,
                source,
                relative_path,
                entry,
                files,
                fetched.revision.clone(),
            )?;
            Ok(SkillResponse::Plan(Box::new(plan)))
        }
        SkillOperation::ConfirmInstall { plan_id } => {
            confirm_skill_plan(state, &require_project()?, plan_id).map(SkillResponse::Installed)
        }
        SkillOperation::DiscardPlan { plan_id } => {
            let mut locked = state.lock().unwrap();
            let mut catalog = skill_catalog(&locked)?;
            catalog.plans.retain(|p| p.id != plan_id);
            save_skill_catalog(&mut locked, &catalog)?;
            let stage = skill_root(&locked).join("plans").join(plan_id.to_string());
            drop(locked);
            if stage.exists() {
                fs::remove_dir_all(stage).map_err(|e| e.to_string())?;
            }
            Ok(SkillResponse::Accepted)
        }
        SkillOperation::SetEnabled { identity, enabled } => {
            let project = require_project()?;
            {
                let mut locked = state.lock().unwrap();
                let mut catalog = skill_catalog(&locked)?;
                if let Some(skill) = catalog
                    .installed
                    .iter_mut()
                    .find(|s| s.identity == identity)
                {
                    skill.enabled = enabled;
                    save_skill_catalog(&mut locked, &catalog)?;
                    locked.broadcast(ServerEvent::SkillsChanged {
                        project_id: Some(project.id),
                    });
                    return Ok(SkillResponse::Accepted);
                }
            }
            let (entries, _) = discover_skills(state, &project)?;
            let entry = entries
                .iter()
                .find(|e| e.identity == identity)
                .ok_or("Skill missing")?;
            if entry.scope != "user" && entry.scope != "repo" {
                return Err("This skill is managed by its installation or administrator".into());
            }
            skill_client(state, &project)?
                .call(
                    "skills/config/write",
                    serde_json::json!({"path":entry.path,"enabled":enabled}),
                )
                .map_err(|e| e.to_string())?;
            state.lock().unwrap().broadcast(ServerEvent::SkillsChanged {
                project_id: Some(project.id),
            });
            Ok(SkillResponse::Accepted)
        }
        SkillOperation::Versions { identity } => {
            let catalog = skill_catalog(&state.lock().unwrap())?;
            Ok(SkillResponse::Versions(
                catalog
                    .installed
                    .iter()
                    .find(|s| s.identity == identity)
                    .ok_or("Managed skill missing")?
                    .versions
                    .clone(),
            ))
        }
        SkillOperation::Rollback {
            identity,
            content_hash,
        } => {
            let project = require_project()?;
            let catalog = skill_catalog(&state.lock().unwrap())?;
            let skill = catalog
                .installed
                .iter()
                .find(|s| s.identity == identity)
                .ok_or("Managed skill missing")?;
            let version = skill
                .versions
                .iter()
                .position(|v| v.entry.content_hash == content_hash)
                .ok_or("Previous revision missing")?;
            let plan = &skill.versions[version];
            verify_skill_plan_files(plan)?;
            recognize_skill_path(state, &project, &plan.entry.path)?;
            let mut locked = state.lock().unwrap();
            let mut current = skill_catalog(&locked)?;
            let skill = current
                .installed
                .iter_mut()
                .find(|s| s.identity == identity)
                .ok_or("Skill removed during rollback")?;
            skill.current = skill
                .versions
                .iter()
                .position(|v| v.id == plan.id)
                .ok_or("Revision changed during rollback")?;
            save_skill_catalog(&mut locked, &current)?;
            locked.broadcast(ServerEvent::SkillsChanged {
                project_id: Some(project.id),
            });
            Ok(SkillResponse::Installed(plan.entry.clone()))
        }
        SkillOperation::Remove { identity } => {
            let mut locked = state.lock().unwrap();
            if locked
                .tasks
                .values()
                .any(|t| t.skills.iter().any(|s| s.identity == identity))
                || locked.agents.values().any(|r| {
                    r.run
                        .execution_profile
                        .skills
                        .iter()
                        .any(|s| s.identity == identity)
                })
            {
                return Err("This revision is pinned by a task or recorded agent session. Unassign tasks and retain session evidence before removal.".into());
            }
            let mut catalog = skill_catalog(&locked)?;
            if !catalog.installed.iter().any(|s| s.identity == identity) {
                return Ok(SkillResponse::Accepted);
            }
            let destination = skill_root(&locked).join("versions").join(&identity);
            catalog.installed.retain(|s| s.identity != identity);
            save_skill_catalog(&mut locked, &catalog)?;
            locked.broadcast(ServerEvent::SkillsChanged {
                project_id: request.project_id,
            });
            drop(locked);
            if destination.exists() {
                fs::remove_dir_all(destination).map_err(|e| {
                    format!("Skill unregistered; retained files could not be removed: {e}")
                })?;
            }
            Ok(SkillResponse::Accepted)
        }
        SkillOperation::Bind {
            task_id,
            expected_revision,
            mut skills,
        } => {
            let project = require_project()?;
            validate_selected_skills(state, &project, &skills)?;
            let mut locked = state.lock().unwrap();
            let task = locked.tasks.get(&task_id).cloned().ok_or("Task missing")?;
            task.check_revision(expected_revision)
                .map_err(|e| e.message)?;
            if task.project_id != project.id
                || locked.task_live(&task)
                || task.archived
                || matches!(task.column(), TaskColumn::InReview | TaskColumn::Done)
            {
                return Err(
                    "Stop active work and reopen/request changes before changing skill bindings"
                        .into(),
                );
            }
            for skill in &mut skills {
                skill.origin = TaskActor::User;
                skill.reason = None;
                skill.created_at = Utc::now();
            }
            let mut next = task.clone();
            next.skills = skills;
            next.touch();
            let audit = next.audit(
                TaskActor::User,
                "skills_bound",
                Some(task.column()),
                Some(format!(
                    "Selected hashes in order: {}",
                    next.skills
                        .iter()
                        .map(|s| format!("{}@{}", s.identity, s.content_hash))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            );
            let changes = vec![(next.clone(), audit)];
            locked
                .store
                .save_task_changes(&changes, None, None)
                .map_err(|e| e.to_string())?;
            locked.publish_tasks(changes);
            Ok(SkillResponse::Bound(next))
        }
        SkillOperation::Sync {
            identity,
            content_hash,
            target_project_id,
        } => sync_skill_revision(state, &identity, &content_hash, target_project_id),
        SkillOperation::Import {
            resolved_revision,
            license,
            source,
            relative_path,
            content_hash,
            files,
        } => {
            if !state.lock().unwrap().remote_runtime {
                return Err("Skill import is only available on an SSH runtime".into());
            }
            let project = require_project()?;
            let (hash, name, description, has_scripts) =
                skill_files::validate_files(&files).map_err(|e| e.to_string())?;
            if hash != content_hash {
                return Err("Transferred skill checksum mismatch".into());
            }
            let entry = SkillEntry {
                source_id: Some(source.id),
                relative_path: Some(relative_path.clone()),
                identity: skill_identity(&source.location, &relative_path),
                name,
                description,
                path: PathBuf::new(),
                scope: "managed".into(),
                source: source.location.clone(),
                enabled: true,
                recognized: false,
                content_hash: hash,
                revision: resolved_revision.clone(),
                interface: serde_json::Value::Null,
                dependencies: serde_json::Value::Null,
                missing_dependencies: vec![],
                license,
                has_scripts,
                validation_error: None,
                managed: true,
            };
            let plan = prepare_skill_plan(
                state,
                &project,
                source,
                relative_path,
                entry,
                files,
                resolved_revision,
            )?;
            confirm_skill_plan(state, &project, plan.id).map(SkillResponse::Installed)
        }
    }
}

fn validate_skill_source(location: &str, reference: &str) -> Result<(), String> {
    if reference.is_empty()
        || reference.len() > 200
        || reference.starts_with('-')
        || reference
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("Invalid Git ref".into());
    }
    if Path::new(location).is_absolute() {
        if !Path::new(location).is_dir() {
            return Err("Source directory is missing".into());
        }
        return Ok(());
    }
    if location.len() > 2048
        || location
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || !(location.starts_with("https://")
            || location.starts_with("ssh://")
            || location.starts_with("git@"))
    {
        return Err("Use an absolute local directory or an HTTPS/SSH Git URL".into());
    }
    Ok(())
}
fn source_by_id(state: &Arc<Mutex<RuntimeState>>, id: Uuid) -> Result<SkillSource, String> {
    skill_catalog(&state.lock().unwrap())?
        .sources
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("Source no longer registered".into())
}
struct FetchedSkills {
    root: PathBuf,
    git: bool,
    revision: Option<String>,
    temporary: bool,
}
impl Drop for FetchedSkills {
    fn drop(&mut self) {
        if self.temporary {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn fetch_skill_source(
    state: &Arc<Mutex<RuntimeState>>,
    source: &SkillSource,
    allow_network: bool,
) -> Result<FetchedSkills, String> {
    validate_skill_source(&source.location, &source.reference)?;
    if Path::new(&source.location).is_absolute() {
        return Ok(FetchedSkills {
            root: fs::canonicalize(&source.location).map_err(|e| e.to_string())?,
            git: false,
            revision: None,
            temporary: false,
        });
    }
    if !allow_network {
        return Err(
            "Confirm network access to fetch this source. Nothing has been installed.".into(),
        );
    }
    let root = skill_root(&state.lock().unwrap())
        .join("staging")
        .join(Uuid::new_v4().to_string());
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let mut fetched = FetchedSkills {
        root,
        git: true,
        revision: None,
        temporary: true,
    };
    skill_files::git(&["init", "--quiet"], &fetched.root, 4096).map_err(|e| e.to_string())?;
    skill_files::git(
        &[
            "fetch",
            "--depth=1",
            "--no-tags",
            "--",
            &source.location,
            &source.reference,
        ],
        &fetched.root,
        4096,
    )
    .map_err(|e| e.to_string())?;
    let revision = skill_files::git(&["rev-parse", "FETCH_HEAD^{commit}"], &fetched.root, 128)
        .map_err(|e| e.to_string())?;
    fetched.revision = Some(String::from_utf8_lossy(&revision).trim().into());
    Ok(fetched)
}
fn git_tree(source: &FetchedSkills) -> Result<Vec<(String, String, String)>, String> {
    let bytes = skill_files::git(
        &[
            "ls-tree",
            "-r",
            "-z",
            source
                .revision
                .as_deref()
                .ok_or("Missing source revision")?,
        ],
        &source.root,
        2 * 1024 * 1024,
    )
    .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = std::str::from_utf8(record).map_err(|_| "Non-UTF8 source path")?;
        let (meta, path) = text.split_once('\t').ok_or("Malformed Git tree")?;
        let fields = meta.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err("Malformed Git metadata".into());
        }
        skill_files::safe_relative(path).map_err(|e| e.to_string())?;
        entries.push((fields[0].into(), fields[2].into(), path.into()));
        if entries.len() > 10000 {
            return Err("Collection exceeds file limit".into());
        }
    }
    Ok(entries)
}
fn source_skill_paths(source: &FetchedSkills) -> Result<Vec<String>, String> {
    if !source.git {
        return skill_files::enumerate(&source.root).map_err(|e| e.to_string());
    }
    Ok(git_tree(source)?
        .into_iter()
        .filter(|(_, _, p)| p == "SKILL.md" || p.ends_with("/SKILL.md"))
        .map(|(_, _, p)| p.strip_suffix("/SKILL.md").unwrap_or(".").into())
        .collect())
}
fn source_skill_entry(
    source: &SkillSource,
    fetched: &FetchedSkills,
    relative: &str,
) -> Result<(SkillEntry, Vec<SkillFile>), String> {
    if relative != "." {
        skill_files::safe_relative(relative).map_err(|e| e.to_string())?;
    }
    let files = if !fetched.git {
        let root = fs::canonicalize(fetched.root.join(relative)).map_err(|e| e.to_string())?;
        if !root.starts_with(&fetched.root) {
            return Err("Skill escapes source directory".into());
        }
        skill_files::read_tree(&root).map_err(|e| e.to_string())?
    } else {
        let mut files = Vec::new();
        let prefix = format!("{relative}/");
        let mut total = 0;
        for (mode, oid, path) in git_tree(fetched)? {
            let relative_path = if relative == "." {
                path.as_str()
            } else if let Some(path) = path.strip_prefix(&prefix) {
                path
            } else {
                continue;
            };
            if mode != "100644" && mode != "100755" {
                return Err("Skill includes a symlink, submodule or special file".into());
            }
            let bytes = skill_files::git(
                &["cat-file", "blob", &oid],
                &fetched.root,
                skill_files::MAX_FILE,
            )
            .map_err(|e| e.to_string())?;
            total += bytes.len();
            if total > skill_files::MAX_TOTAL || files.len() >= skill_files::MAX_FILES {
                return Err("Selected skill exceeds limits".into());
            }
            use base64::Engine as _;
            files.push(SkillFile {
                path: relative_path.into(),
                content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                executable: mode == "100755",
            });
        }
        files
    };
    let (hash, name, description, has_scripts) =
        skill_files::validate_files(&files).map_err(|e| e.to_string())?;
    let license = if !fetched.git {
        ["LICENSE", "LICENSE.md", "LICENSE.txt"]
            .iter()
            .find_map(|name| {
                skill_files::read_bounded(&fetched.root.join(name), 32768)
                    .ok()
                    .and_then(|v| String::from_utf8(v).ok())
            })
    } else {
        git_tree(fetched)?
            .into_iter()
            .find(|(_, _, p)| ["LICENSE", "LICENSE.md", "LICENSE.txt"].contains(&p.as_str()))
            .and_then(|(mode, oid, _)| {
                if mode == "100644" {
                    skill_files::git(&["cat-file", "blob", &oid], &fetched.root, 32768)
                        .ok()
                        .and_then(|v| String::from_utf8(v).ok())
                } else {
                    None
                }
            })
    };
    Ok((
        SkillEntry {
            source_id: Some(source.id),
            relative_path: Some(relative.into()),
            identity: skill_identity(&source.location, relative),
            name,
            description,
            path: PathBuf::from(relative),
            scope: "available".into(),
            source: source.location.clone(),
            enabled: false,
            recognized: false,
            content_hash: hash,
            revision: fetched.revision.clone(),
            interface: serde_json::Value::Null,
            dependencies: serde_json::Value::Null,
            missing_dependencies: vec![],
            license,
            has_scripts,
            validation_error: None,
            managed: true,
        },
        files,
    ))
}
fn skill_path_metadata(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    path: &Path,
) -> Result<serde_json::Value, String> {
    let mut client = skill_client(state, project)?;
    client
        .set_roots(&[path
            .parent()
            .and_then(Path::parent)
            .ok_or("Invalid skill root")?
            .to_path_buf()])
        .map_err(|e| e.to_string())?;
    let value = client
        .call(
            "skills/list",
            serde_json::json!({"cwds":[project.root],"forceReload":true}),
        )
        .map_err(|e| e.to_string())?;
    value["data"].as_array().into_iter().flatten().flat_map(|e|e["skills"].as_array().into_iter().flatten()).find(|s|s["path"].as_str()==path.to_str()&&s["enabled"]==true).cloned()
        .ok_or("Codex did not recognize this skill. Check its manifest and extra-root support. The previous version is retained.".into())
}
fn recognize_skill_path(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    path: &Path,
) -> Result<(), String> {
    skill_path_metadata(state, project, path).map(|_| ())
}
fn prepare_skill_plan(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    source: SkillSource,
    relative: String,
    mut entry: SkillEntry,
    files: Vec<SkillFile>,
    revision: Option<String>,
) -> Result<SkillInstallPlan, String> {
    let id = Uuid::new_v4();
    let root = skill_root(&state.lock().unwrap());
    let stage = root.join("plans").join(id.to_string());
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    skill_files::write_tree(&stage.join("skill"), &files).map_err(|e| e.to_string())?;
    let stage_manifest =
        fs::canonicalize(stage.join("skill/SKILL.md")).map_err(|e| e.to_string())?;
    let metadata = skill_path_metadata(state, project, &stage_manifest)?;
    entry.interface = metadata["interface"].clone();
    entry.dependencies = metadata["dependencies"].clone();
    entry.missing_dependencies = dependency_errors(&entry.dependencies);
    let destination = root
        .join("versions")
        .join(&entry.identity)
        .join(&entry.content_hash)
        .join("skill");
    entry.path = destination.join("SKILL.md");
    entry.scope = "managed".into();
    entry.enabled = true;
    entry.recognized = true;
    let mut locked = state.lock().unwrap();
    let mut catalog = skill_catalog(&locked)?;
    if catalog.plans.len() >= 200 {
        return Err(
            "Too many pending install plans; finish or remove old plans before preparing more"
                .into(),
        );
    }
    let prior_revision = catalog
        .installed
        .iter()
        .find(|s| s.identity == entry.identity)
        .map(|s| s.versions[s.current].entry.content_hash.clone());
    let plan = SkillInstallPlan {
        id,
        source,
        resolved_revision: revision,
        entry,
        included_paths: files.iter().map(|f| f.path.clone()).collect(),
        destination,
        prior_revision,
        created_at: Utc::now(),
    };
    // Relative source path is retained in the immutable plan, not inferred from display names.
    locked
        .store
        .set_setting(&format!("ditch.skills.plan.{id}.relative"), &relative)
        .map_err(|e| e.to_string())?;
    catalog.plans.push(plan.clone());
    save_skill_catalog(&mut locked, &catalog)?;
    Ok(plan)
}
fn verify_skill_plan_files(plan: &SkillInstallPlan) -> Result<(), String> {
    let files = skill_files::read_tree(&plan.destination).map_err(|e| e.to_string())?;
    if skill_files::validate_files(&files)
        .map_err(|e| e.to_string())?
        .0
        != plan.entry.content_hash
    {
        return Err("Installed skill checksum changed".into());
    }
    Ok(())
}
fn confirm_skill_plan(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    id: Uuid,
) -> Result<SkillEntry, String> {
    let (root, plan, relative) = {
        let locked = state.lock().unwrap();
        let catalog = skill_catalog(&locked)?;
        if let Some(version) = catalog
            .installed
            .iter()
            .flat_map(|s| s.versions.iter())
            .find(|p| p.id == id)
        {
            verify_skill_plan_files(version)?;
            return Ok(version.entry.clone());
        }
        (
            skill_root(&locked),
            catalog
                .plans
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or("Install plan expired")?,
            locked
                .store
                .setting(&format!("ditch.skills.plan.{id}.relative"))
                .map_err(|e| e.to_string())?
                .ok_or("Plan provenance missing")?,
        )
    };
    let stage = root.join("plans").join(id.to_string()).join("skill");
    if plan.destination.exists() {
        verify_skill_plan_files(&plan)?;
    } else {
        let files = skill_files::read_tree(&stage).map_err(|e| e.to_string())?;
        if skill_files::validate_files(&files)
            .map_err(|e| e.to_string())?
            .0
            != plan.entry.content_hash
        {
            return Err("Prepared skill changed. Prepare a new install plan.".into());
        }
        fs::create_dir_all(plan.destination.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::rename(&stage, &plan.destination).map_err(|e| e.to_string())?;
    }
    recognize_skill_path(state, project, &plan.entry.path)?;
    let mut locked = state.lock().unwrap();
    let mut catalog = skill_catalog(&locked)?;
    if let Some(installed) = catalog
        .installed
        .iter_mut()
        .find(|s| s.identity == plan.entry.identity)
    {
        if Some(&installed.versions[installed.current].entry.content_hash)
            != plan.prior_revision.as_ref()
        {
            return Err(
                "Installed version changed while reviewing. Prepare the update again.".into(),
            );
        }
        installed.versions.push(plan.clone());
        installed.current = installed.versions.len() - 1;
        installed.enabled = true;
    } else {
        catalog.installed.push(ManagedSkill {
            identity: plan.entry.identity.clone(),
            source: plan.source.clone(),
            relative_path: relative,
            versions: vec![plan.clone()],
            current: 0,
            enabled: true,
        });
    }
    catalog.plans.retain(|p| p.id != id);
    save_skill_catalog(&mut locked, &catalog)?;
    locked.broadcast(ServerEvent::SkillsChanged {
        project_id: Some(project.id),
    });
    Ok(plan.entry)
}
fn sync_skill_revision(
    state: &Arc<Mutex<RuntimeState>>,
    identity: &str,
    hash: &str,
    target: ProjectId,
) -> Result<SkillResponse, String> {
    let project = project_by_id(state, target)
        .filter(Project::is_remote)
        .ok_or("Select an SSH project for sync")?;
    let catalog = skill_catalog(&state.lock().unwrap())?;
    let skill = catalog
        .installed
        .iter()
        .find(|s| s.identity == identity)
        .ok_or("Only validated Ditch-managed skills can be synced")?;
    let plan = skill
        .versions
        .iter()
        .find(|v| v.entry.content_hash == hash)
        .ok_or("Selected revision is missing")?;
    verify_skill_plan_files(plan)?;
    let files = skill_files::read_tree(&plan.destination).map_err(|e| e.to_string())?;
    let response = handle_skill_request(
        Arc::clone(state),
        SkillRequest {
            project_id: Some(target),
            operation: SkillOperation::Import {
                resolved_revision: plan.resolved_revision.clone(),
                license: plan.entry.license.clone(),
                source: skill.source.clone(),
                relative_path: skill.relative_path.clone(),
                content_hash: hash.into(),
                files,
            },
        },
    );
    match response {
        ServerResponse::SkillResponse(SkillResponse::Installed(entry))
            if entry.content_hash == hash =>
        {
            Ok(SkillResponse::Installed(entry))
        }
        ServerResponse::SkillResponse(SkillResponse::Error { message, .. }) => Err(message),
        _ => Err(format!(
            "Sync to {} failed checksum/recognition verification",
            project.name
        )),
    }
}
