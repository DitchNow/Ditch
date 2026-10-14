fn session_name_home(origin: Option<&str>) -> Option<PathBuf> {
    origin
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

fn open_session_names(home: &Path) -> Option<rusqlite::Connection> {
    let connection = rusqlite::Connection::open_with_flags(
        home.join("state_5.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    connection
        .busy_timeout(std::time::Duration::from_millis(50))
        .ok()?;
    Some(connection)
}

fn read_session_name(connection: &rusqlite::Connection, thread_id: &str) -> Option<String> {
    connection
        .query_row(
            "SELECT name FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .as_deref()
        .and_then(ditch_core::normalize_session_title)
}

fn codex_title_from_state_at(thread_id: &str, origin: Option<&str>) -> Option<String> {
    read_session_name(&open_session_names(&session_name_home(origin)?)?, thread_id)
}

#[cfg(test)]
fn codex_title_from_state(thread_id: &str) -> Option<String> {
    codex_title_from_state_at(thread_id, None)
}

// The per-turn App Server process can exit before Codex persists its name.
// Poll only unnamed local sessions, batching reads by their original Codex home.
// This never sends a prompt or a request to Relay.
fn refresh_session_names(state: &Arc<Mutex<RuntimeState>>) {
    let pending = {
        let locked = state.lock().unwrap();
        let mut pending: HashMap<PathBuf, Vec<(AgentId, String)>> = HashMap::new();
        for record in locked.agents.values() {
            let run = &record.run;
            if run.codex_title.is_some()
                || !locked.projects.values().any(|project| {
                    project.id == run.project_id
                        && matches!(project.execution_target, ProjectExecutionTarget::Local)
                })
            {
                continue;
            }
            if let (Some(id), Some(home)) = (
                run.native_session_id.as_ref(),
                session_name_home(run.origin_codex_home.as_deref()),
            ) {
                pending.entry(home).or_default().push((run.id, id.clone()));
            }
        }
        pending
    };
    for (home, sessions) in pending {
        let Some(connection) = open_session_names(&home) else {
            continue;
        };
        for (agent_id, thread_id) in sessions {
            let Some(name) = read_session_name(&connection, &thread_id) else {
                continue;
            };
            let mut locked = state.lock().unwrap();
            let Some(record) = locked.agents.get_mut(&agent_id) else {
                continue;
            };
            if record.run.native_session_id.as_deref() != Some(&thread_id)
                || record.run.codex_title.is_some()
                || session_name_home(record.run.origin_codex_home.as_deref()).as_ref()
                    != Some(&home)
            {
                continue;
            }
            record.run.codex_title = Some(name);
            record.run.naming_failed = false;
            let run = record.run.clone();
            locked.persist_agent(agent_id);
            locked.broadcast(ServerEvent::AgentChanged(run));
        }
    }
}

fn start_session_name_watcher(state: &Arc<Mutex<RuntimeState>>) {
    let weak = Arc::downgrade(state);
    thread::spawn(move || {
        loop {
            thread::sleep(std::time::Duration::from_secs(3));
            let Some(state) = weak.upgrade() else { break };
            refresh_session_names(&state);
            process_session_name_job(&state);
        }
    });
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SessionNameReceipt {
    thread_id: String,
    attempts: u8,
    retry_at: i64,
    generated_name: Option<String>,
}

struct SessionNameJob {
    agent_id: AgentId,
    thread_id: String,
    home: Option<PathBuf>,
    binary: String,
    workspace: PathBuf,
    model: Option<String>,
    prompt: String,
    receipt: SessionNameReceipt,
}

fn name_receipt_key(agent_id: AgentId) -> String {
    format!("session-naming:{}", agent_id.0)
}

// A single background worker claims these jobs. The durable attempt reservation
// bounds model calls across restarts as well as repeated polling.
fn claim_session_name_job(state: &Arc<Mutex<RuntimeState>>) -> Option<SessionNameJob> {
    let mut locked = state.lock().unwrap();
    let binary = locked.codex_binary.clone()?;
    let mut candidates: Vec<_> = locked
        .agents
        .values()
        .filter(|record| {
            let run = &record.run;
            run.codex_title.is_none()
                && run.user_title.is_none()
                && !run.naming_failed
                && run.native_session_id.is_some()
                && locked.projects.values().any(|p| {
                    p.id == run.project_id
                        && p.archived_at.is_none()
                        && matches!(p.execution_target, ProjectExecutionTarget::Local)
                })
        })
        .map(|r| r.run.clone())
        .collect();
    candidates.sort_by_key(|run| std::cmp::Reverse(run.updated_at));
    for run in candidates {
        let thread_id = run.native_session_id.as_ref()?;
        let key = name_receipt_key(run.id);
        let mut receipt: SessionNameReceipt = match locked.store.setting(&key) {
            Ok(Some(value)) => match serde_json::from_str(&value) {
                Ok(r) => r,
                Err(_) => continue,
            },
            Ok(None) => SessionNameReceipt::default(),
            Err(_) => continue,
        };
        if receipt.thread_id != *thread_id {
            receipt = SessionNameReceipt {
                thread_id: thread_id.clone(),
                ..Default::default()
            };
        }
        if receipt.attempts >= 2 {
            set_session_naming_failed(&mut locked, run.id, thread_id);
            continue;
        }
        if receipt.retry_at > Utc::now().timestamp() {
            continue;
        }
        let prompt = locked
            .store
            .agent_messages_after(run.id, 0)
            .ok()?
            .into_iter()
            .find(|entry| entry.message.role == AgentChatRole::User)
            .map(|entry| entry.message.text)
            .or(run.current_prompt.clone())
            .filter(|s| !s.trim().is_empty());
        let Some(prompt) = prompt else { continue };
        receipt.attempts += 1;
        receipt.retry_at = Utc::now().timestamp() + 60;
        if locked
            .store
            .set_setting(&key, &serde_json::to_string(&receipt).ok()?)
            .is_err()
        {
            continue;
        }
        return Some(SessionNameJob {
            agent_id: run.id,
            thread_id: thread_id.clone(),
            home: session_name_home(run.origin_codex_home.as_deref()),
            binary,
            workspace: locked.paths.data_dir.join("session-naming"),
            model: run.execution_profile.model,
            prompt,
            receipt,
        });
    }
    None
}

fn session_name_job_matches(locked: &RuntimeState, job: &SessionNameJob) -> bool {
    locked.agents.get(&job.agent_id).is_some_and(|record| {
        record.run.native_session_id.as_deref() == Some(&job.thread_id)
            && session_name_home(record.run.origin_codex_home.as_deref()) == job.home
    })
}

fn set_session_naming_failed(locked: &mut RuntimeState, agent_id: AgentId, thread_id: &str) {
    let Some(record) = locked.agents.get_mut(&agent_id) else {
        return;
    };
    if record.run.native_session_id.as_deref() != Some(thread_id)
        || record.run.codex_title.is_some()
        || record.run.naming_failed
    {
        return;
    }
    record.run.naming_failed = true;
    let run = record.run.clone();
    locked.persist_agent(agent_id);
    locked.broadcast(ServerEvent::AgentChanged(run));
}

fn process_session_name_job(state: &Arc<Mutex<RuntimeState>>) {
    let Some(mut job) = claim_session_name_job(state) else {
        return;
    };
    let result = generate_session_name(state, &mut job);
    if result.is_err() && job.receipt.attempts >= 2 {
        let mut locked = state.lock().unwrap();
        if session_name_job_matches(&locked, &job) {
            set_session_naming_failed(&mut locked, job.agent_id, &job.thread_id);
        }
    }
    // Error text may contain provider data. Expose only the bounded failure state.
}

fn generate_session_name(
    state: &Arc<Mutex<RuntimeState>>,
    job: &mut SessionNameJob,
) -> io::Result<()> {
    let mut client = session_namer::open(&job.binary, &job.workspace, job.home.as_deref())?;
    let existing = session_namer::existing_name(&mut client, &job.thread_id)?;
    let name = if let Some(name) = existing.clone().or(job.receipt.generated_name.clone()) {
        name
    } else {
        let name = session_namer::generate(
            &mut client,
            &job.workspace,
            job.model.as_deref(),
            &job.prompt,
        )?;
        job.receipt.generated_name = Some(name.clone());
        let mut locked = state.lock().unwrap();
        if !session_name_job_matches(&locked, job) {
            return Ok(());
        }
        locked
            .store
            .set_setting(
                &name_receipt_key(job.agent_id),
                &serde_json::to_string(&job.receipt)?,
            )
            .map_err(io::Error::other)?;
        name
    };
    {
        let locked = state.lock().unwrap();
        if !session_name_job_matches(&locked, job) {
            return Ok(());
        }
        let run = &locked.agents[&job.agent_id].run;
        if run.user_title.is_some() || run.codex_title.is_some() {
            return Ok(());
        }
    }
    if existing.is_none() {
        session_namer::save(&mut client, &job.thread_id, &name)?;
    }
    let mut locked = state.lock().unwrap();
    if !session_name_job_matches(&locked, job) {
        return Ok(());
    }
    let record = locked.agents.get_mut(&job.agent_id).unwrap();
    if record.run.codex_title.is_some() {
        return Ok(());
    }
    // Never mutate user_title; a manual edit made during either RPC wins.
    record.run.codex_title = Some(name);
    record.run.naming_failed = false;
    let run = record.run.clone();
    locked.persist_agent(job.agent_id);
    locked.broadcast(ServerEvent::AgentChanged(run));
    Ok(())
}
