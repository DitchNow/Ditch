use super::*;
use ditch_core::{TaskIssueKind, TaskOperation, TaskRequest, TaskResponse};

pub(super) fn status(state: &Arc<Mutex<RuntimeState>>, task_id: TaskId) -> Result<Value, String> {
    let s = state.lock().unwrap();
    let task = s.tasks.get(&task_id).ok_or("Task unavailable")?;
    let payload: Option<String> = s
        .store
        .with_extension_connection(NS, |c| {
            Ok(c.query_row(
                "SELECT payload FROM github_publications WHERE task_id=?1",
                [task_id.0.to_string()],
                |r| r.get(0),
            )
            .optional()?)
        })
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"source":task.github_source,"publication":payload.map(|v| serde_json::from_str::<Value>(&v)).transpose().map_err(|e|e.to_string())?}),
    )
}

pub(super) fn create(
    state: Arc<Mutex<RuntimeState>>,
    service: &dyn GitHubService,
    project: ProjectId,
    repository_id: u64,
    request_id: String,
    title: String,
    body: String,
    task_id: Option<TaskId>,
    issue_type: Option<String>,
    issue_kind: TaskIssueKind,
) -> Result<Value, String> {
    check_registered(&state, project)?;
    Uuid::parse_str(&request_id).map_err(|_| "Invalid creation request ID")?;
    if title.trim().is_empty()
        || title.chars().count() > 256
        || body.len() > 65536
        || issue_type
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 256)
    {
        return Err("Enter a title of 1–256 characters and a body of at most 64 KiB".into());
    }
    let repo = verified_repository(&state, service, project, repository_id)?;
    let mut payload = json!({"project_id":project,"repository_id":repository_id,"request_id":request_id,"title":title.trim(),"body":body,"task_id":task_id,"issue_type":issue_type,"issue_kind":issue_kind});
    let mut recovered = None;
    if let Some(id) = task_id {
        let s = state.lock().unwrap();
        let task = s
            .tasks
            .get(&id)
            .filter(|t| t.project_id == project)
            .ok_or("Task unavailable in this project")?;
        if let Some(source) = &task.github_source {
            let pending: bool = s
                .store
                .with_extension_connection(NS, |c| {
                    Ok(c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM github_publications WHERE task_id=?1)",
                        [id.0.to_string()],
                        |r| r.get(0),
                    )?)
                })
                .map_err(|e| e.to_string())?;
            if !pending {
                return Ok(json!({"task_ids":[id],"source":source}));
            }
        }
        s.store
            .with_extension_connection(NS, |c| {
                let prior: Option<(String, Option<String>)> = c
                    .query_row(
                        "SELECT payload,result FROM github_publications WHERE task_id=?1",
                        [id.0.to_string()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((saved, result)) = prior {
                    payload =
                        serde_json::from_str(&saved).map_err(|_| rusqlite::Error::InvalidQuery)?;
                    recovered = result;
                } else {
                    c.execute(
                        "INSERT INTO github_publications(task_id,payload) VALUES(?1,?2)",
                        params![id.0.to_string(), payload.to_string()],
                    )?;
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        if payload["repository_id"] != repository_id
            || payload["title"] != title.trim()
            || payload["body"] != body
            || payload["issue_type"] != json!(issue_type)
        {
            return Err("This task has a pending publication. Reopen Create on GitHub to retry its saved contents.".into());
        }
    }
    let raw = match recovered {
        Some(raw) => serde_json::from_str(&raw).map_err(|_| "Invalid saved publication")?,
        None => match service.create(
            project,
            repository_id,
            payload["request_id"]
                .as_str()
                .ok_or("Invalid saved request")?,
            title.trim(),
            &body,
            issue_type.as_deref(),
            task_id,
        ) {
            Ok(value) => value,
            Err(error) => {
                // This Relay error is emitted only before reserving/sending a POST.
                // Unknown outcomes retain the immutable publication for safe recovery.
                if error.starts_with(
                    "Ditch Relay rejected the request (github_issue_type_unavailable):",
                ) {
                    if let Some(id) = task_id {
                        let s = state.lock().unwrap();
                        s.store.with_extension_connection(NS, |c| {
                            c.execute("DELETE FROM github_publications WHERE task_id=?1 AND result IS NULL", [id.0.to_string()])?;
                            Ok(())
                        }).map_err(|e|e.to_string())?;
                    }
                }
                return Err(error);
            }
        },
    };
    let issue: GitHubIssue =
        serde_json::from_value(raw.clone()).map_err(|_| "Invalid created issue")?;
    if !issue.html_url.eq_ignore_ascii_case(&format!(
        "https://github.com/{}/issues/{}",
        repo.full_name, issue.number
    )) {
        return Err("Created issue identity could not be verified".into());
    }
    if let Some(id) = task_id {
        {
            let s = state.lock().unwrap();
            service.check_commit(&s)?;
            s.store
                .with_extension_connection(NS, |c| {
                    c.execute(
                        "UPDATE github_publications SET result=?2 WHERE task_id=?1",
                        params![id.0.to_string(), raw.to_string()],
                    )?;
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
        }
        let response = handle_task_request(
            state.clone(),
            TaskRequest {
                project_id: Some(project),
                request_id: Uuid::new_v4(),
                operation: TaskOperation::AttachGitHub {
                    task_id: id,
                    source: issue_source(&repo, &issue),
                    issue: issue.clone(),
                },
            },
        );
        let ServerResponse::TaskResponse(TaskResponse::Changed(attached)) = response else {
            return Err(format!(
                "Issue created; task attachment needs retry: {response:?}"
            ));
        };
        if attached.id != id || attached.project_id != project {
            return Err("Issue created; remote task identity did not match".into());
        }
        let mut s = state.lock().unwrap();
        service.check_commit(&s)?;
        s.update_remote_task(attached);
        s.store.with_extension_connection(NS, |c| {
            c.execute("INSERT INTO github_sources VALUES(?1,'github.com',?2,?3,?4,?5,?6) ON CONFLICT(project_id,host,issue_id) DO UPDATE SET snapshot=excluded.snapshot",
                params![project.0.to_string(),issue.id.to_string(),repo.id.to_string(),issue.number.to_string(),id.0.to_string(),serde_json::to_string(&issue).unwrap()])?; Ok(())
        }).map_err(|e|e.to_string())?;
        Ok(json!({"task_ids":[id],"source":issue_source(&repo,&issue)}))
    } else {
        let mut s = state.lock().unwrap();
        service.check_commit(&s)?;
        import_issues_with_kind(
            &mut s,
            project,
            &repo,
            vec![issue],
            Some(request_id),
            Some(issue_kind),
        )
    }
}

pub(crate) fn attach_source(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
    id: TaskId,
    source: GitHubTaskSource,
    issue: GitHubIssue,
) -> ServerResponse {
    let mut s = state.lock().unwrap();
    // SSH hosts can receive task metadata without ever opening a GitHub account session.
    if let Err(error) = initialize(&s.store) {
        return task_error(ditch_core::TaskErrorCode::Storage, error);
    }
    let Some(mut task) = s
        .tasks
        .get(&id)
        .filter(|t| Some(t.project_id) == request.project_id)
        .cloned()
    else {
        return task_error(ditch_core::TaskErrorCode::NotFound, "Task unavailable");
    };
    let prior: Result<Option<String>, _> = s.store.with_extension_connection(NS, |c| Ok(c.query_row(
        "SELECT task_id FROM github_sources WHERE project_id=?1 AND host='github.com' AND issue_id=?2",
        params![task.project_id.0.to_string(),source.issue_id.to_string()], |r|r.get(0)).optional()?));
    match prior {
        Ok(Some(other)) if other != id.0.to_string() => {
            return task_error(
                ditch_core::TaskErrorCode::InvalidInput,
                "Issue is already attached to another task",
            );
        }
        Err(e) => return task_error(ditch_core::TaskErrorCode::Storage, e.to_string()),
        _ => {}
    }
    if let Some(existing) = &task.github_source {
        if existing.issue_id != source.issue_id || existing.repository_id != source.repository_id {
            return task_error(
                ditch_core::TaskErrorCode::InvalidInput,
                "Task is already linked to another GitHub issue",
            );
        }
        return ServerResponse::TaskResponse(TaskResponse::Changed(task));
    }
    if repository_name(&source.repository).is_err()
        || issue.id != source.issue_id
        || issue.number != source.number
        || issue.html_url != source.url
        || source.url
            != format!(
                "https://github.com/{}/issues/{}",
                source.repository, source.number
            )
    {
        return task_error(
            ditch_core::TaskErrorCode::InvalidInput,
            "Invalid GitHub issue identity",
        );
    }
    task.github_source = Some(source.clone());
    task.touch();
    let audit = task.audit(
        TaskActor::User,
        "github_published",
        Some(task.column()),
        None,
    );
    let response = TaskResponse::Changed(task.clone());
    if let Err(e) = s.store.save_task_group(NS, &[(task.clone(),audit.clone())], |tx| {
        tx.execute("INSERT INTO github_sources VALUES(?1,'github.com',?2,?3,?4,?5,?6) ON CONFLICT(project_id,host,issue_id) DO UPDATE SET snapshot=excluded.snapshot",
            params![task.project_id.0.to_string(),source.issue_id.to_string(),source.repository_id.to_string(),source.number.to_string(),id.0.to_string(),serde_json::to_string(&issue).unwrap()])?;
        Ok(())
    }) {
        return task_error(ditch_core::TaskErrorCode::Storage,e.to_string());
    }
    s.publish_tasks(vec![(task, audit)]);
    ServerResponse::TaskResponse(response)
}
