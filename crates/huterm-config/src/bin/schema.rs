use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "--check".into());
    if !matches!(mode.as_str(), "--check" | "--write") {
        return Err("expected --check or --write".into());
    }
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    for (name, document) in huterm_config::schema::documents()? {
        let path = directory.join(name);
        if mode == "--write" {
            fs::create_dir_all(&directory)?;
            fs::write(path, document)?;
        } else if fs::read_to_string(&path).ok().as_deref() != Some(&document) {
            return Err(format!(
                "{} is stale; run mise run schema:generate",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}
