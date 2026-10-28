//! Plain wording (`.design/2026-10-27-plain-wording.md`): no string andon
//! can print uses a phrasing the glossary retired. Scans every string
//! literal outside `#[cfg(test)]` in `src/`, and the `///` help lines in
//! `src/cli.rs`.

use std::path::Path;

/// The retired phrasings, each with what replaced it.
const BANNED: &[(&str, &str)] = &[
    ("no source reads", "X not configured; add [adapters.Y]"),
    ("cannot decide", "error:"),
    ("the tracker", "bd or gh, by name"),
    ("the messenger", "hail"),
    ("the runner", "ferry or CI"),
    (" holds ", "claimed"),
    (" held ", "claimed"),
    ("cites", "references"),
    ("names no", "has no, or doesn't reference"),
    ("joins no", "doesn't match a seat"),
    ("join no", "don't match a seat"),
    ("quiet", "inactive"),
    ("refused:", "error:"),
    ("warn:", "warning:"),
];

/// The banned phrasings in `text`, with their replacements.
fn banned(text: &str) -> Vec<(&'static str, &'static str)> {
    let lower = text.to_lowercase();
    BANNED
        .iter()
        .filter(|(phrase, _)| lower.contains(phrase))
        .copied()
        .collect()
}

/// The string literals of `source` before its `#[cfg(test)]` module, each
/// with its line, plus its `///` lines when `help` is set.
fn printed(source: &str, help: bool) -> Vec<(usize, String)> {
    let body = source.split("#[cfg(test)]").next().unwrap_or(source);
    let mut out = Vec::new();
    for (n, line) in body.lines().enumerate() {
        let t = line.trim_start();
        if let Some(doc) = t.strip_prefix("///") {
            if help {
                out.push((n + 1, doc.to_owned()));
            }
            continue;
        }
        if t.starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let mut end = None;
            let mut escaped = false;
            for (i, c) in after.char_indices() {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            // prose has a space; a JSON path or a key does not
            if after[..end].contains(' ') {
                out.push((n + 1, after[..end].to_owned()));
            }
            rest = &after[end + 1..];
        }
    }
    out
}

fn sources(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{e}")) {
        let path = entry.unwrap_or_else(|e| panic!("{e}")).path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|x| x == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_printed_string_uses_a_retired_phrasing() {
    let mut files = Vec::new();
    sources(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(),
        &mut files,
    );
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{e}"));
        let help = file.ends_with("cli.rs");
        for (line, text) in printed(&source, help) {
            for (phrase, instead) in banned(&text) {
                found.push(format!(
                    "{}:{line}: {phrase:?} (use {instead}): {text}",
                    file.display()
                ));
            }
        }
    }
    assert!(found.is_empty(), "retired phrasings:\n{}", found.join("\n"));
}

#[test]
fn a_planted_retired_phrasing_is_found() {
    let source = "fn f() -> String {\n    format!(\"not shown, no source reads them: {}\", x)\n}\n";
    let hits: Vec<_> = printed(source, false)
        .into_iter()
        .flat_map(|(_, text)| banned(&text))
        .collect();
    assert_eq!(hits, vec![BANNED[0]]);
    // a comment and a test module are not printed
    let quiet =
        "// no source reads\n#[cfg(test)]\nmod t { const X: &str = \"no source reads\"; }\n";
    assert!(
        printed(quiet, false)
            .iter()
            .all(|(_, t)| banned(t).is_empty())
    );
    // a help line is
    let help = "/// Every seat, the quiet ones too.\n";
    assert_eq!(printed(help, true).len(), 1);
}
