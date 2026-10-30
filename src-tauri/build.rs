fn main() {
    println!("cargo:rerun-if-changed=../build_info.json");
    println!("cargo:rerun-if-changed=../version.txt");
    println!("cargo:rerun-if-env-changed=FINFETCHER_BUILD_VERSION");
    let identity: serde_json::Value = std::fs::read_to_string("../build_info.json")
        .ok()
        .map(|text| serde_json::from_str(&text).expect("Invalid build_info.json"))
        .unwrap_or_else(|| serde_json::json!({
            "version": std::fs::read_to_string("../version.txt").expect("Missing version.txt").trim(),
            "commit": "", "branch": "", "run_id": ""
        }));
    let version = std::env::var("FINFETCHER_BUILD_VERSION").unwrap_or_else(|_| {
        identity["version"]
            .as_str()
            .expect("Missing build version")
            .to_owned()
    });
    println!("cargo:rustc-env=FINFETCHER_BUILD_VERSION={version}");
    println!("cargo:rustc-env=FINFETCHER_BUILD_IDENTITY_JSON={identity}");
    tauri_build::build()
}
