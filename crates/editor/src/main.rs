use std::env;

use anyhow::{Context, Result};
use editor_ui::{run_editor, smoke_test_summary_json, EditorChromeDefinition};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let chrome = load_chrome_definition()?;

    if args.iter().any(|arg| arg == "--smoke-test") {
        println!("{}", smoke_test_summary_json(chrome)?);
        return Ok(());
    }

    run_editor(chrome).map_err(|error| anyhow::anyhow!("failed to launch GameCAD Editor window: {error}"))
}

fn load_chrome_definition() -> Result<EditorChromeDefinition> {
    let path = env::current_dir()
        .context("failed to read current directory")?
        .join("config")
        .join("editor-chrome.json");

    if path.exists() {
        return EditorChromeDefinition::load_from_path(&path)
            .with_context(|| format!("failed to load {}", path.display()));
    }

    Ok(EditorChromeDefinition::default())
}
