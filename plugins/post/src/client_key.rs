//! Profile-inherited ACL policy, exclusive creation and reparse-point rejection.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use vega_herdr::{platform, Failure};

pub fn failure(e: impl std::fmt::Display) -> Failure {
    Failure::new("private_file", e.to_string(), "use a private user profile directory (or an owner-restricted XDG directory); remove symlinks/reparse points")
}
pub fn check_path(path: &Path) -> Result<(), Failure> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(unix)]
                let reparse = meta.file_type().is_symlink();
                if reparse {
                    return Err(failure(format!(
                        "{} is a reparse point/link",
                        ancestor.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(failure(format!("{}: {e}", ancestor.display()))),
        }
    }
    Ok(())
}
pub fn read_private(path: &Path) -> Result<Vec<u8>, Failure> {
    check_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let mut file = options.open(path).map_err(failure)?;
    let meta = file.metadata().map_err(failure)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(failure("reparse-point file"));
        }
    }
    if !meta.is_file() {
        return Err(failure("expected a regular file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(failure)?;
    Ok(bytes)
}
pub fn create_private(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    check_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| failure("missing parent directory"))?;
    fs::create_dir_all(parent).map_err(failure)?;
    check_path(path)?;
    platform::private_dir(parent).map_err(failure)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = platform::private_options(&mut options)
        .open(path)
        .map_err(failure)?;
    file.write_all(bytes).map_err(failure)?;
    file.sync_all().map_err(failure)?;
    platform::sync_dir(parent).map_err(failure)
}
pub fn read_seed(path: &Path) -> Result<String, Failure> {
    let seed = String::from_utf8(read_private(path)?).map_err(failure)?;
    let pair = nkeys::KeyPair::from_seed(seed.trim()).map_err(failure)?;
    if !pair.public_key().starts_with('U') {
        return Err(failure("expected a user nkey"));
    }
    Ok(seed.trim().into())
}
pub fn keygen() -> Result<(), Failure> {
    let path = crate::seed_path(crate::Seed::Host)?;
    let path = Path::new(&path);
    check_path(path)?;
    if !path.exists() {
        let pair = nkeys::KeyPair::new_user();
        create_private(
            path,
            format!("{}\n", pair.seed().map_err(failure)?).as_bytes(),
        )?;
    }
    let seed = read_seed(path)?;
    let public = nkeys::KeyPair::from_seed(&seed)
        .map_err(failure)?
        .public_key();
    println!("host: {}", crate::host());
    println!("public_key: {public}");
    println!(
        "seed: {} (profile-inherited ACL; owner must restrict custom directories)",
        path.display()
    );
    println!(
        "help: give {}={public} to the operator; the seed stays on this host",
        crate::host()
    );
    Ok(())
}
