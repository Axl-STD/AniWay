//! Compiles po/*.po into target/locale/<lang>/LC_MESSAGES/aniway.mo (needs msgfmt; skipped with a warning).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=po");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    let Some(target) = out.ancestors().nth(4) else { return };
    for entry in std::fs::read_dir("po").into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(lang) = path.file_stem().filter(|_| path.extension().is_some_and(|e| e == "po")) else { continue };
        let dir = target.join("locale").join(lang).join("LC_MESSAGES");
        let built = std::fs::create_dir_all(&dir).is_ok_and(|()| {
            Command::new("msgfmt").arg("-o").arg(dir.join("aniway.mo")).arg(&path).status().is_ok_and(|s| s.success())
        });
        if !built {
            println!("cargo:warning=cannot compile {} (is msgfmt installed?)", path.display());
        }
    }
}
