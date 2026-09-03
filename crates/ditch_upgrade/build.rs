fn main() {
    println!("cargo:rerun-if-env-changed=DITCH_DEPLOYMENT_ENVIRONMENT");
    println!("cargo:rerun-if-env-changed=DITCH_RELAY_ORIGIN");
}
