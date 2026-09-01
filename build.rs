use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    guard_c_shim();
    emit_version();
    embuild::espidf::sysenv::output();
}

fn emit_version() {
    println!("cargo:rerun-if-env-changed=BEAMER_VERSION");
    for path in [".git/HEAD", ".git/index"] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let version = std::env::var("BEAMER_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| {
            let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
            format!("{pkg}-unknown")
        });
    println!("cargo:rustc-env=BEAMER_VERSION={}", version.trim());
}

fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let described = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!described.is_empty()).then_some(described)
}

fn guard_c_shim() {
    let components = Path::new("components");
    let defaults = Path::new("sdkconfig.defaults");
    if !components.is_dir() || !defaults.is_file() {
        return;
    }

    let mut files = Vec::new();
    collect(components, &mut files);
    files.sort();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    println!("cargo:rerun-if-changed=sdkconfig.defaults");

    let Some(trigger) = mtime(defaults) else {
        return;
    };
    let stale: Vec<&PathBuf> = files
        .iter()
        .filter(|f| mtime(f).is_some_and(|t| t > trigger))
        .collect();
    if stale.is_empty() {
        return;
    }

    let bumped = std::fs::OpenOptions::new()
        .append(true)
        .open(defaults)
        .and_then(|f| f.set_modified(SystemTime::now()))
        .is_ok();

    let names: Vec<String> = stale.iter().map(|f| format!("  {}", f.display())).collect();
    panic!(
        "\n\
         These are newer than sdkconfig.defaults, so ESP-IDF has already been built\n\
         without them:\n\
         \n\
         {}\n\
         \n\
         esp-idf-sys only re-runs its CMake build when a sdkconfig file changes, so an\n\
         edit to the C shim would otherwise be linked from a stale object -- silently,\n\
         and with a clean build log. Refusing to produce that binary.\n\
         \n\
         {}\n\
         Run the same command again.\n",
        names.join("\n"),
        if bumped {
            "sdkconfig.defaults has been touched for you, which is what forces the rebuild."
        } else {
            "Could not touch sdkconfig.defaults; do it by hand, then rebuild."
        }
    );
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}
