fn main() {
    for name in [
        "DITCH_DEPLOYMENT_ENVIRONMENT",
        "DITCH_BUILD_IDENTIFIER",
        "DITCH_BUILD_NUMBER",
        "DITCH_RELEASE_SEQUENCE",
        "DITCH_COMMUNITY_REVISION",
        "DITCH_COMMUNITY_BUILD_SEQUENCE",
        "DITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64",
        "DITCH_APPLE_TEAM_ID",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
}
