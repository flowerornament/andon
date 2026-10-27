//! The README's configuration reference is the config: it parses, and it
//! names every key `src/config.rs` reads.

use std::path::Path;

/// The first TOML code block after the reference's heading.
fn reference(readme: &str) -> Option<&str> {
    let after = readme.split_once("## Configuration reference")?.1;
    let block = after.split_once("```toml\n")?.1;
    Some(block.split_once("```")?.0)
}

/// Every field `config.rs` deserializes: each `pub name:` in a struct that
/// derives `Deserialize`, unless serde skips it.
fn config_keys(source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut in_struct = false;
    let mut deserialize = false;
    let mut skip = false;
    for line in source.lines() {
        let line = line.trim_end();
        if line.starts_with("#[derive(") {
            deserialize = line.contains("Deserialize");
        } else if line.starts_with("pub struct ") {
            in_struct = deserialize;
        } else if line == "}" {
            in_struct = false;
        } else if in_struct && line.trim() == "#[serde(skip)]" {
            skip = true;
        } else if in_struct
            && let Some(rest) = line.strip_prefix("    pub ")
            && let Some((name, _)) = rest.split_once(':')
            && !std::mem::take(&mut skip)
        {
            keys.push(name.to_owned());
        }
    }
    keys
}

#[test]
fn the_config_reference_parses_and_names_every_key() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // the Nix package builds without the README (it is not a build input);
    // `just check` and CI run with it
    let Ok(readme) = std::fs::read_to_string(root.join("README.md")) else {
        return;
    };
    let block =
        reference(&readme).unwrap_or_else(|| panic!("README has no configuration reference"));
    andon::config::Config::parse(block, Path::new("/")).unwrap_or_else(|e| {
        panic!("the README's configuration reference is not a valid config: {e:#}")
    });
    let source =
        std::fs::read_to_string(root.join("src/config.rs")).unwrap_or_else(|e| panic!("{e}"));
    let keys = config_keys(&source);
    assert!(keys.len() > 50, "found only {} keys: {keys:?}", keys.len());
    let named = |key: &str| {
        block.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with(&format!("{key} ="))
                || l.contains(&format!(".{key}]"))
                || l.contains(&format!("[{key}"))
                || l.contains(&format!("{{ {key} ="))
                || l.contains(&format!(", {key} ="))
        })
    };
    // fields of tables keyed by name, or of an inline value, appear inside them
    let missing: Vec<&String> = keys.iter().filter(|k| !named(k)).collect();
    assert!(
        missing.is_empty(),
        "README's configuration reference lacks {missing:?}; add each with its default"
    );
}
