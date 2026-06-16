use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn dependency_direction_keeps_connectors_out_of_engine_and_contract() {
    let root = workspace_root();
    let forbidden = [
        root.join("libagent/Cargo.toml"),
        root.join("runa-forge-capability/Cargo.toml"),
    ];
    for manifest in forbidden {
        let content = fs::read_to_string(&manifest).unwrap();
        assert!(
            !content.contains("runa-connector"),
            "{} must not depend on connector crates",
            manifest.display()
        );
    }

    let mcp_manifest = fs::read_to_string(root.join("runa-mcp/Cargo.toml")).unwrap();
    assert!(
        mcp_manifest.contains("runa-connector-registry"),
        "runa-mcp should link only the generic connector registry"
    );
    assert!(
        !mcp_manifest.contains("runa-connector-github")
            && !mcp_manifest.contains("runa-connector-sourcehut"),
        "runa-mcp must not depend on provider connector crates directly"
    );
}

#[test]
fn provider_grammar_does_not_leak_into_new_engine_or_surface_code() {
    let root = workspace_root();
    let searched = [root.join("runa-mcp/src"), root.join("runa-cli/src")];
    let host_secret_env = ["WEFORGE", "OPERATOR", "PAT"].join("_");
    let forbidden = [
        "github:",
        "sourcehut:",
        "weforge.build",
        host_secret_env.as_str(),
    ];
    for dir in searched {
        for path in rust_files(&dir) {
            let content = fs::read_to_string(&path).unwrap();
            for needle in forbidden {
                assert!(
                    !content.contains(needle),
                    "{} contains provider grammar '{}'",
                    path.display(),
                    needle
                );
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_files(dir, &mut files);
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}
