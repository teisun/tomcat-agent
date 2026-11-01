use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const BUILTIN_SKILLS: [&str; 5] = [
    "verify",
    "connectors",
    "skill-creator",
    "plugin-creator",
    "write-plans",
];

/// Dependency installs and editor/OS junk live beside skill sources on dev machines
/// (`npm ci` for the verify scripts, Finder's `.DS_Store`); none of it ships.
fn is_embedded(name: &str) -> bool {
    name != "node_modules" && !name.starts_with('.')
}

fn collect_files(dir: &Path, relative: &str, files: &mut Vec<(String, PathBuf)>) {
    let mut entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| entry.expect("read skill asset entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| panic!("non UTF-8 skill asset name: {}", path.display()));
        if !is_embedded(name) {
            continue;
        }
        let child = format!("{relative}/{name}");
        if path.is_dir() {
            collect_files(&path, &child, files);
        } else {
            files.push((child, path));
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for skill in BUILTIN_SKILLS {
        let root = manifest_dir.join("assets/skills").join(skill);
        println!("cargo:rerun-if-changed={}", root.display());
        collect_files(&root, skill, &mut files);
    }

    let mut table = String::from("static BUILTIN_SKILL_FILES: &[(&str, &[u8])] = &[\n");
    for (relative, path) in &files {
        writeln!(
            table,
            "    ({relative:?}, include_bytes!({:?})),",
            path.display().to_string()
        )
        .expect("write to String");
    }
    table.push_str("];\n");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join("builtin_skills.rs"), table).expect("write builtin_skills.rs");
}
