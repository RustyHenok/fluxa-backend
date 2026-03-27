use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rendered = fluxa_backend::openapi::render_pretty()?;

    if let Some(path) = env::args().nth(1) {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("{rendered}\n"))?;
    } else {
        println!("{rendered}");
    }

    Ok(())
}
