use chrono::Utc;
use ditch_core::{AgentId, AppPaths};
use ditch_protocol::{ClientRequest, Envelope, ServerResponse};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use uuid::Uuid;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = match args.as_slice() {
        [] => print_help(),
        [command] if command == "health" => print_response(ClientRequest::Health),
        [command] if command == "paths" => print_paths(),
        [command] if command == "doctor" => doctor(),
        [runtime, command] if runtime == "runtime" && command == "status" => {
            print_response(ClientRequest::RuntimeStatus)
        }
        [runtime, command] if runtime == "runtime" && command == "reconnect" => {
            print_response(ClientRequest::Snapshot)
        }
        [runtime, command, agent_id] if runtime == "runtime" && command == "stop-agent" => {
            stop_agent(agent_id)
        }
        [runtime, command] if runtime == "runtime" && command == "stop" => {
            print_response(ClientRequest::Shutdown)
        }
        [runtime, command, flag]
            if runtime == "runtime" && command == "stop" && flag == "--force" =>
        {
            print_response(ClientRequest::Shutdown)
        }
        _ => {
            let _ = print_help();
            Err(2)
        }
    };

    if let Err(code) = result {
        std::process::exit(code);
    }
}

fn print_help() -> Result<(), i32> {
    eprintln!("available commands:");
    eprintln!("  ditch health");
    eprintln!("  ditch paths");
    eprintln!("  ditch doctor");
    eprintln!("  ditch runtime status");
    eprintln!("  ditch runtime reconnect");
    eprintln!("  ditch runtime stop-agent <agent-uuid>");
    eprintln!("  ditch runtime stop");
    eprintln!("  ditch runtime stop --force");
    Ok(())
}

fn print_paths() -> Result<(), i32> {
    let paths = AppPaths::for_current_user();
    println!(
        "{}",
        serde_json::to_string_pretty(&paths).expect("app paths should serialize")
    );
    Ok(())
}

fn print_response(request: ClientRequest) -> Result<(), i32> {
    match send_request(request) {
        Ok(response) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&response).expect("response should serialize")
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("{error}");
            Err(1)
        }
    }
}

fn doctor() -> Result<(), i32> {
    let paths = AppPaths::for_current_user();
    println!("runtime identity: The Ditch Runtime");
    println!("socket path: {}", paths.socket_path.display());
    println!("socket exists: {}", paths.socket_path.exists());
    match send_request(ClientRequest::RuntimeStatus) {
        Ok(ServerResponse::RuntimeStatus(status)) => {
            println!("runtime pid: {}", status.pid);
            println!("active sessions: {}", status.active_session_count);
            Ok(())
        }
        Ok(other) => {
            println!("unexpected runtime response: {other:?}");
            Err(1)
        }
        Err(error) => {
            println!("runtime connection error: {error}");
            Err(1)
        }
    }
}

fn stop_agent(agent_id: &str) -> Result<(), i32> {
    let uuid = match Uuid::parse_str(agent_id) {
        Ok(uuid) => uuid,
        Err(error) => {
            eprintln!("invalid agent id: {error}");
            return Err(2);
        }
    };
    print_response(ClientRequest::StopAgent {
        agent_id: AgentId(uuid),
    })
}

fn send_request(request: ClientRequest) -> io::Result<ServerResponse> {
    let paths = AppPaths::for_current_user();
    let mut stream = UnixStream::connect(paths.socket_path)?;
    let envelope = Envelope {
        protocol_version: ditch_protocol::PROTOCOL_VERSION,
        id: Uuid::new_v4(),
        sent_at: Utc::now(),
        body: request,
    };
    serde_json::to_writer(&mut stream, &envelope)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let envelope = serde_json::from_str::<Envelope<ServerResponse>>(&line)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(envelope.body)
}
