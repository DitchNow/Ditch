use ditch_core::AppPaths;
use ditch_protocol::{Envelope, HealthResponse, ServerResponse};
use ditch_store::ensure_app_dirs;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;

fn main() {
    if std::env::args().any(|arg| arg == "--stdio") {
        if let Err(error) = run_stdio() {
            emit_event(&DaemonEvent::Error {
                message: error.to_string(),
            });
            std::process::exit(1);
        }
        return;
    }

    let paths = AppPaths::for_current_user();
    if let Err(error) = ensure_app_dirs(&paths) {
        eprintln!("failed to prepare The Ditch app directories: {error}");
        std::process::exit(1);
    }

    let health = HealthResponse {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        app_paths: paths,
        codex_binary: find_on_path("codex"),
        claude_binary: find_on_path("claude"),
    };

    let response = Envelope::new(ServerResponse::Health(health));
    println!(
        "{}",
        serde_json::to_string_pretty(&response).expect("health response should serialize")
    );
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DaemonCommand {
    StartCodex {
        binary: String,
        cwd: String,
        prompt: String,
        exec_mode: bool,
        env: Vec<(String, String)>,
        rows: u16,
        cols: u16,
    },
    Input {
        text: String,
    },
    Resize {
        rows: u16,
        cols: u16,
    },
    Stop,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DaemonEvent {
    Ready,
    Started,
    Output { text: String },
    Exit { code: Option<i32> },
    Error { message: String },
}

enum PtyCommand {
    Input(String),
    Resize(PtySize),
    Stop,
}

fn run_stdio() -> io::Result<()> {
    emit_event(&DaemonEvent::Ready);
    let stdin = io::stdin();
    let mut session: Option<Sender<PtyCommand>> = None;

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let command = match serde_json::from_str::<DaemonCommand>(&line) {
            Ok(command) => command,
            Err(error) => {
                emit_event(&DaemonEvent::Error {
                    message: format!("invalid command: {error}"),
                });
                continue;
            }
        };

        match command {
            DaemonCommand::StartCodex {
                binary,
                cwd,
                prompt,
                exec_mode,
                env,
                rows,
                cols,
            } => {
                if session.is_some() {
                    emit_event(&DaemonEvent::Error {
                        message: "a PTY session is already active".to_owned(),
                    });
                    continue;
                }
                match spawn_codex_pty(binary, cwd, prompt, exec_mode, env, rows, cols) {
                    Ok(tx) => {
                        session = Some(tx);
                        emit_event(&DaemonEvent::Started);
                    }
                    Err(error) => emit_event(&DaemonEvent::Error {
                        message: error.to_string(),
                    }),
                }
            }
            DaemonCommand::Input { text } => {
                if let Some(tx) = &session {
                    let _ = tx.send(PtyCommand::Input(text));
                }
            }
            DaemonCommand::Resize { rows, cols } => {
                if let Some(tx) = &session {
                    let _ = tx.send(PtyCommand::Resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    }));
                }
            }
            DaemonCommand::Stop => {
                if let Some(tx) = session.take() {
                    let _ = tx.send(PtyCommand::Stop);
                }
            }
        }
    }

    Ok(())
}

fn spawn_codex_pty(
    binary: String,
    cwd: String,
    prompt: String,
    exec_mode: bool,
    env: Vec<(String, String)>,
    rows: u16,
    cols: u16,
) -> io::Result<Sender<PtyCommand>> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(to_io_error)?;

    let mut command = CommandBuilder::new(binary);
    if exec_mode {
        command.arg("exec");
    }
    command.arg(prompt);
    command.cwd(PathBuf::from(cwd));
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    for (key, value) in env {
        command.env(key, value);
    }

    let mut child = pair.slave.spawn_command(command).map_err(to_io_error)?;
    drop(pair.slave);

    let (tx, rx) = mpsc::channel::<PtyCommand>();
    let mut reader = pair.master.try_clone_reader().map_err(to_io_error)?;
    let mut writer = pair.master.take_writer().map_err(to_io_error)?;
    let master = pair.master;

    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => emit_event(&DaemonEvent::Output {
                    text: String::from_utf8_lossy(&buffer[..read]).into_owned(),
                }),
                Err(error) => {
                    emit_event(&DaemonEvent::Error {
                        message: format!("PTY read failed: {error}"),
                    });
                    break;
                }
            }
        }
    });

    thread::spawn(move || {
        for command in rx {
            match command {
                PtyCommand::Input(text) => {
                    let _ = writer.write_all(text.as_bytes());
                    let _ = writer.flush();
                }
                PtyCommand::Resize(size) => {
                    let _ = master.resize(size);
                }
                PtyCommand::Stop => {
                    let _ = writer.write_all(b"\x03");
                    let _ = writer.flush();
                    break;
                }
            }
        }
    });

    thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => Some(status.exit_code() as i32),
            Err(_) => None,
        };
        emit_event(&DaemonEvent::Exit { code });
    });

    Ok(tx)
}

fn emit_event(event: &DaemonEvent) {
    let mut stdout = io::stdout().lock();
    let _ = serde_json::to_writer(&mut stdout, event);
    let _ = stdout.write_all(b"\n");
    let _ = stdout.flush();
}

fn to_io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn find_on_path(binary: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(binary))
            .find(|candidate| candidate.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    })
}
