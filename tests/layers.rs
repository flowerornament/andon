//! The layers: each module of src/ imports only the layers beneath it, so
//! adapters never depend on a fold or a check, and a fold never depends on
//! a surface. This fails when product code (a file before its
//! `#[cfg(test)]`) names a module at its own layer or above.

use std::path::Path;

/// From the bottom: the event contract and its parts, the readers of
/// sources, the fold, the surfaces and refusals, the hook, the command line.
const LAYERS: &[&[&str]] = &[
    &["event", "time", "commit"],
    &["config"],
    &["goal", "ledger", "seat"],
    &["adapters"],
    &["fold"],
    &["check", "render"],
    &["hook", "listing"],
    &["doctor", "init"],
    &["cli"],
];

fn layer(module: &str) -> Option<usize> {
    LAYERS.iter().position(|l| l.contains(&module))
}

/// The top module a file belongs to: `src/fold/board.rs` is `fold`.
fn module_of(path: &Path, src: &Path) -> Option<String> {
    let rel = path.strip_prefix(src).ok()?;
    let first = rel.components().next()?.as_os_str().to_str()?;
    Some(first.trim_end_matches(".rs").to_owned())
}

/// Every `crate::x` a text names, and each `x` of `use crate::{x, y}`.
fn named(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("crate::") {
        let rest = &text[i + "crate::".len()..];
        if let Some(group) = rest.strip_prefix('{') {
            let end = group.find('}').unwrap_or(group.len());
            out.extend(
                group[..end]
                    .split(',')
                    .map(|m| m.trim().split("::").next().unwrap_or("").to_owned()),
            );
        } else {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            out.push(rest[..end].to_owned());
        }
    }
    out
}

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn each_module_imports_only_the_layers_beneath_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = Vec::new();
    files(&src, &mut paths);
    let mut slips = Vec::new();
    for path in paths {
        let Some(module) = module_of(&path, &src) else {
            continue;
        };
        if module == "lib" || module == "main" {
            continue;
        }
        let Some(own) = layer(&module) else {
            slips.push(format!("{module} has no layer; add it to LAYERS"));
            continue;
        };
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
        let product = text.split("#[cfg(test)]").next().unwrap_or_default();
        for target in named(product) {
            if target == module || target.is_empty() {
                continue;
            }
            match layer(&target) {
                Some(l) if l < own => {}
                Some(_) => slips.push(format!(
                    "{} imports {target}, which is not beneath {module}",
                    path.strip_prefix(&src).unwrap_or(&path).display()
                )),
                None => slips.push(format!("{target} has no layer; add it to LAYERS")),
            }
        }
    }
    slips.sort();
    slips.dedup();
    assert!(slips.is_empty(), "layer slips:\n{}", slips.join("\n"));
}

#[test]
fn a_slip_is_seen() {
    assert_eq!(
        named("use crate::{adapters, fold::board};\nlet x = crate::check::Commit;"),
        vec!["adapters", "fold", "check"]
    );
}
