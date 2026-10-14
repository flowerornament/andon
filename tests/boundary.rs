//! The boundary: andon's product code knows no project. A project's
//! conventions live in its own andon.toml, and anything bespoke reaches
//! andon through an external adapter it owns (src/adapters/external.rs).
//! This fails when a project's name appears in src/.

use std::path::Path;

/// Public projects andon has been pointed at. Add to it, never remove.
const PROJECTS: &[&str] = &["hyperfine"];

/// Private project names, one per line, from `~/.config/andon/projects`
/// when this machine has one: they stay out of the repository, and the
/// guard still covers them here.
fn local_projects() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    std::fs::read_to_string(Path::new(&home).join(".config/andon/projects"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

#[test]
fn product_code_names_no_project() {
    let mut projects: Vec<String> = PROJECTS.iter().map(|p| (*p).to_owned()).collect();
    projects.extend(local_projects());
    let mut found = Vec::new();
    let mut stack = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{e}"))
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .to_lowercase();
            for (n, line) in text.lines().enumerate() {
                for project in &projects {
                    if line.contains(project.as_str()) {
                        found.push(format!("{}:{}: {project}", path.display(), n + 1));
                    }
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "project names in product code; move the convention to config or an external adapter:\n{}",
        found.join("\n")
    );
}
