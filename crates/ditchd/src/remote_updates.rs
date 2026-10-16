// Included in ssh_remote: a requested update belongs to the Mac runtime, not
// the lifetime of the dialog or its response socket. No update starts merely
// because a connection uses an older build.
#[derive(Clone, serde::Serialize, Deserialize)]
struct RequestedUpdate {
    alias: String,
    phase: String,
    message: String,
    build: String,
    #[serde(default)]
    result: Option<RemoteSetupStatus>,
}

impl RemoteConnectionManager {
    fn update_path(identity: &str) -> PathBuf {
        let home = ditch_core::AppPaths::for_current_user();
        home.data_dir
            .join("remote-updates")
            .join(format!("{:x}.json", Sha256::digest(identity.as_bytes())))
    }

    fn save_update(&self, identity: &str, job: RequestedUpdate) -> Result<(), SshError> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // A disk error must not strand the UI at an obsolete progress state.
        self.updates
            .lock()
            .unwrap()
            .insert(identity.into(), job.clone());
        let path = Self::update_path(identity);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700))?;
        let temporary = path.with_extension("next");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(&job).map_err(|e| SshError::Failed(e.to_string()))?)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    fn update_view(job: &RequestedUpdate) -> RemoteSetupStatus {
        if let Some(result) = &job.result {
            return result.clone();
        }
        let mut result = failed_setup(&job.alias, &job.message, None);
        result.connection_state = if job.phase == "failed" {
            "update_failed"
        } else {
            "updating"
        }
        .into();
        result.last_error = (job.phase == "failed").then(|| job.message.clone());
        result.checks = vec![check(
            "runtime",
            "Ditch Runtime",
            RemoteCheckState::InstallAvailable,
            &job.message,
            None,
        )];
        result
    }

    pub fn runtime_update_status(
        &self,
        alias: &str,
    ) -> Result<Option<RemoteSetupStatus>, SshError> {
        let identity = maintenance_identity(&resolve_host(alias)?);
        if let Some(job) = self.updates.lock().unwrap().get(&identity).cloned() {
            return Ok(Some(Self::update_view(&job)));
        }
        let path = Self::update_path(&identity);
        if !path.exists() {
            return Ok(None);
        }
        let job: RequestedUpdate = serde_json::from_slice(&fs::read(path)?).map_err(|e| {
            SshError::Failed(format!("Cannot recover requested runtime update: {e}"))
        })?;
        if !matches!(job.phase.as_str(), "complete" | "failed") {
            // Resume only an already requested operation. A newly installed Mac
            // build must not silently substitute its own artifact in that job.
            if job.build != local_build() {
                return Err(SshError::Failed("The app changed during repair. Use Install / Repair to select this app's runtime.".into()));
            }
            return self.start_runtime_update(alias).map(Some);
        }
        self.updates.lock().unwrap().insert(identity, job.clone());
        Ok(Some(Self::update_view(&job)))
    }

    pub fn start_runtime_update(&self, alias: &str) -> Result<RemoteSetupStatus, SshError> {
        let identity = maintenance_identity(&resolve_host(alias)?);
        {
            let mut jobs = self.updates.lock().unwrap();
            if let Some(job) = jobs.get(&identity) {
                if !matches!(job.phase.as_str(), "complete" | "failed") {
                    return Ok(Self::update_view(job));
                }
            }
            jobs.insert(
                identity.clone(),
                RequestedUpdate {
                    alias: alias.into(),
                    phase: "preparing".into(),
                    message: "Preparing runtime update…".into(),
                    build: local_build().into(),
                    result: None,
                },
            );
        }
        let initial = self.updates.lock().unwrap()[&identity].clone();
        if let Err(error) = self.save_update(&identity, initial.clone()) {
            self.updates.lock().unwrap().remove(&identity);
            return Err(error);
        }
        let connections = self.clone();
        let alias = alias.to_owned();
        std::thread::spawn(move || {
            let mut transient_failures = 0;
            let mut staged = None;
            loop {
                match install_remote_runtime(&connections, &alias, &mut staged) {
                    Ok(result) => {
                        let _ = connections.save_update(
                            &identity,
                            RequestedUpdate {
                                alias: alias.clone(),
                                phase: "complete".into(),
                                message: "Runtime updated.".into(),
                                build: local_build().into(),
                                result: Some(result),
                            },
                        );
                        break;
                    }
                    Err(error) => {
                        let detail = error.to_string();
                        let waiting = update_is_waiting(&detail);
                        let transient = matches!(error, SshError::Timeout)
                            || detail.contains("Connection reset")
                            || detail.contains("Connection refused")
                            || detail.contains("Connection timed out")
                            || detail.contains("Broken pipe")
                            || detail.contains("Resource temporarily unavailable")
                            || detail.contains("timed out")
                            || detail.contains("reconnection failed");
                        if transient {
                            transient_failures += 1;
                        }
                        let retry = waiting || (transient && transient_failures <= 5);
                        let message = if waiting {
                            "Waiting for remote work to finish…".into()
                        } else if retry {
                            "Reconnecting to finish the runtime update…".into()
                        } else {
                            detail
                        };
                        let _ = connections.save_update(
                            &identity,
                            RequestedUpdate {
                                alias: alias.clone(),
                                phase: if retry { "waiting" } else { "failed" }.into(),
                                message,
                                build: local_build().into(),
                                result: None,
                            },
                        );
                        if !retry {
                            break;
                        }
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
            }
            if let Some(staged) = staged {
                // Only this operation's random upload is removed. Immutable
                // installed releases, project data, and journals are retained.
                if let Ok(resolved) = resolve_host(&alias) {
                    let script = format!("rm -f -- '{}'", staged.upload);
                    let _ = run_fixed_admin_authenticated(
                        &alias,
                        &script,
                        connections.password_for(&resolved),
                        false,
                    );
                }
            }
        });
        Ok(Self::update_view(&initial))
    }
}

fn local_build() -> &'static str {
    option_env!("DITCH_BUILD_IDENTIFIER").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn update_is_waiting(message: &str) -> bool {
    message.contains("runtime_busy:")
        || message.contains("repair is in progress")
        || message.contains("upgrade_in_progress:")
        || message.contains("already running for this remote account")
}

#[cfg(test)]
mod update_tests {
    use super::*;
    #[test]
    fn only_busy_or_another_update_is_a_wait_state() {
        assert!(update_is_waiting("runtime_busy: active agent"));
        assert!(update_is_waiting("Remote runtime repair is in progress."));
        assert!(!update_is_waiting(
            "client_update_required: update this Mac"
        ));
        assert!(!update_is_waiting("Checksum mismatch"));
    }
}
