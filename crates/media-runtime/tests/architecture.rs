//! Inspect resolved manifests, including renamed/target/build dependencies, without building engines.
use serde_json::Value;
use std::{path::Path, process::Command};

#[test]
fn native_sdk_dependencies_follow_layer_responsibilities() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .current_dir(root)
        .output()
        .expect("Cargo metadata must be available");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    for (name, allowed) in [
        ("pv-media-runtime", &["tokio", "serde", "serde_json"][..]),
        (
            "pv-media-libwebrtc",
            &["pv-media-runtime", "libwebrtc", "tokio"][..],
        ),
        (
            "pv-media-native",
            &[
                "pv-media-runtime",
                "pv-media-libwebrtc",
                "tokio",
                "serde",
                "serde_json",
                "futures-util",
                "tokio-tungstenite",
                "url",
            ][..],
        ),
    ] {
        let package = packages
            .iter()
            .find(|package| package["name"] == name)
            .expect("SDK crate exists");
        for dependency in package["dependencies"].as_array().unwrap() {
            if dependency["kind"] == "dev" {
                continue;
            }
            let dependency_name = dependency["name"].as_str().unwrap();
            assert!(
                allowed.contains(&dependency_name),
                "{name} crosses its layer boundary through {dependency_name}"
            );
            if dependency_name == "libwebrtc" {
                assert_eq!(
                    dependency["req"], "=0.3.50",
                    "Native engine upgrades require an explicit compatibility review"
                );
            }
        }
    }
}
