use ditch_core::AppPaths;
use ditch_protocol::{ClientRequest, Envelope};

fn main() {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "health".to_owned());

    match command.as_str() {
        "health" => print_health_request(),
        "paths" => print_paths(),
        other => {
            eprintln!("unknown ditch command: {other}");
            eprintln!("available commands: health, paths");
            std::process::exit(2);
        }
    }
}

fn print_health_request() {
    let request = Envelope::new(ClientRequest::Health);
    println!(
        "{}",
        serde_json::to_string_pretty(&request).expect("health request should serialize")
    );
}

fn print_paths() {
    let paths = AppPaths::for_current_user();
    println!(
        "{}",
        serde_json::to_string_pretty(&paths).expect("app paths should serialize")
    );
}
