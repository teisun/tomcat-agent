use std::path::Path;

fn emit_rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            emit_rerun_if_changed(&entry.path());
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    emit_rerun_if_changed(Path::new("assets/skills/skill-creator"));
    emit_rerun_if_changed(Path::new("assets/skills/plugin-creator"));
}
