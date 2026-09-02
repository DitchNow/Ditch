fn main() {
    println!("cargo:rerun-if-env-changed=DITCH_RELAY_ORIGIN");
    println!("cargo:rerun-if-env-changed=DITCH_DEPLOYMENT_ENVIRONMENT");
}
