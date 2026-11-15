use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CONTROL_WORKFLOW_RELEASE_TAG");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let root = git(&["rev-parse", "--show-toplevel"])
        .and_then(|path| Path::new(&path).canonicalize().ok());
    let commit = if root
        == std::env::current_dir()
            .ok()
            .and_then(|path| path.canonicalize().ok())
        && root.is_some()
    {
        git(&["rev-parse", "--verify", "HEAD"])
    } else {
        None
    };
    if let Some(ref hash) = commit {
        // Watch both worktree-specific HEAD/index and the shared ref it points to.
        for name in ["HEAD", "index"] {
            if let Some(path) = git(&["rev-parse", "--git-path", name]) {
                println!("cargo:rerun-if-changed={path}");
            }
        }
        if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
            && let Some(path) = git(&["rev-parse", "--git-path", &reference])
        {
            println!("cargo:rerun-if-changed={path}");
        }
        if let Some(path) = git(&["rev-parse", "--git-path", "packed-refs"]) {
            println!("cargo:rerun-if-changed={path}");
        }
        // Cargo must rerun this script when tracked content or untracked entries change.
        if let Some(files) = git(&["ls-files", "--cached", "--others", "--exclude-standard"]) {
            println!("cargo:rerun-if-changed=.");
            for file in files.lines() {
                println!("cargo:rerun-if-changed={file}");
                let mut parent = Path::new(file).parent();
                while let Some(dir) = parent.filter(|p| !p.as_os_str().is_empty()) {
                    println!("cargo:rerun-if-changed={}", dir.display());
                    parent = dir.parent();
                }
            }
        }
        let status = git(&["status", "--porcelain", "--untracked-files=normal"]);
        let clean = status.as_deref() == Some("");
        if let Some(tag) = std::env::var("CONTROL_WORKFLOW_RELEASE_TAG")
            .ok()
            .filter(|s| !s.is_empty())
        {
            assert!(
                tag == format!("v{version}")
                    && clean
                    && git(&[
                        "rev-parse",
                        "--verify",
                        &format!("refs/tags/{tag}^{{commit}}")
                    ])
                    .as_deref()
                        == Some(hash),
                "release tag must match Cargo version and clean HEAD"
            );
            println!("cargo:rustc-env=CONTROL_WORKFLOW_BUILD_VERSION={version}");
            return;
        }
        let suffix = if clean { "" } else { "-dirty" };
        println!(
            "cargo:rustc-env=CONTROL_WORKFLOW_BUILD_VERSION={version} (commit {hash}{suffix})"
        );
    } else {
        println!("cargo:rerun-if-changed=.git");
        assert!(
            std::env::var("CONTROL_WORKFLOW_RELEASE_TAG")
                .unwrap_or_default()
                .is_empty(),
            "release requires Git metadata"
        );
        println!("cargo:rustc-env=CONTROL_WORKFLOW_BUILD_VERSION={version} (commit unknown)");
    }
}
