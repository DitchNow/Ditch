use std::{env,fs,path::PathBuf};
fn main() {
    println!("cargo:rerun-if-env-changed=DITCH_DEPLOYMENT_ENVIRONMENT");
    println!("cargo:rerun-if-changed=../../config/deployment-identities.json");
    let environment = env::var("DITCH_DEPLOYMENT_ENVIRONMENT").unwrap_or_else(|_| "production".into());
    let namespaces: serde_json::Value = serde_json::from_str(&fs::read_to_string("../../config/deployment-identities.json").unwrap()).unwrap();
    let namespace = namespaces.get(&environment).and_then(|v|v.as_object()).expect("Invalid DITCH_DEPLOYMENT_ENVIRONMENT");
    let mut source = format!("pub const ENVIRONMENT: &str = {environment:?};\n");
    for (key,value) in namespace {
        source.push_str(&format!("pub const {}: &str = {:?};\n",key.to_uppercase(),value.as_str().unwrap()));
    }
    fs::write(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("deployment.rs"),source).unwrap();
}
