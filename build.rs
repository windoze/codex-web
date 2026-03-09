use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let bundled_ui_enabled = env::var_os("CARGO_FEATURE_BUNDLED_UI").is_some();
    if !bundled_ui_enabled {
        return;
    }

    println!("cargo:rerun-if-env-changed=CODEX_WEB_SKIP_UI_BUILD");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let frontend_dir = manifest_dir.join("frontend");
    let dist_dir = frontend_dir.join("dist");

    // Re-run the build script when frontend sources/config change.
    for path in [
        "frontend/index.html",
        "frontend/package.json",
        "frontend/package-lock.json",
        "frontend/tsconfig.json",
        "frontend/vite.config.ts",
        "frontend/src",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            manifest_dir.join(path).display()
        );
    }

    let skip = env::var_os("CODEX_WEB_SKIP_UI_BUILD").is_some();
    if skip {
        ensure_dist_exists(&dist_dir);
        return;
    }

    if !frontend_dir.exists() {
        panic!(
            "bundled-ui is enabled but frontend directory is missing: {}",
            frontend_dir.display()
        );
    }

    // Install dependencies once.
    let node_modules = frontend_dir.join("node_modules");
    if !node_modules.exists() {
        run(
            Command::new("npm").arg("ci").current_dir(&frontend_dir),
            "npm ci",
        );
    }

    // Build the production bundle into `frontend/dist`.
    run(
        Command::new("npm")
            .args(["run", "build"])
            .current_dir(&frontend_dir),
        "npm run build",
    );

    ensure_dist_exists(&dist_dir);
}

fn ensure_dist_exists(dist_dir: &Path) {
    let index = dist_dir.join("index.html");
    if !index.exists() {
        panic!(
            "bundled-ui expects built assets at {}, but {} is missing. Run `cd frontend && npm ci && npm run build` (or unset CODEX_WEB_SKIP_UI_BUILD).",
            dist_dir.display(),
            index.display()
        );
    }
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to run {what}: {e}"));
    if !status.success() {
        panic!("{what} failed with status: {status}");
    }
}
