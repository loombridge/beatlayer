//! Moving a file to the system's trash (file-management R17): nothing is
//! erased outright, so a file that was deleted can be brought back.

use std::path::Path;

/// Moves the file at `path` to the system's trash, where the operating
/// system keeps it until the person empties the trash.
///
/// # Errors
///
/// A message saying why the file couldn't be moved, or why the trash
/// couldn't be found.
pub fn to_trash(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} isn't there", path.display()));
    }
    trash_file(path)
}

/// Moves the file into the user's trash folder.
#[cfg(target_os = "macos")]
fn trash_file(path: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("there's no home folder to move the file to")?;
    let trash = Path::new(&home).join(".Trash");
    std::fs::create_dir_all(&trash)
        .map_err(|error| format!("couldn't open {}: {error}", trash.display()))?;
    move_to(path, &trash)
}

/// Moves the file into the user's trash folder.
#[cfg(all(unix, not(target_os = "macos")))]
fn trash_file(path: &Path) -> Result<(), String> {
    // `gio trash` is what the desktop uses, when it is there.
    let moved = std::process::Command::new("gio")
        .args(["trash", &path.to_string_lossy()])
        .status()
        .is_ok_and(|status| status.success());
    if moved {
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or("there's no home folder to move the file to")?;
    let trash = Path::new(&home).join(".local/share/Trash/files");
    std::fs::create_dir_all(&trash)
        .map_err(|error| format!("couldn't open {}: {error}", trash.display()))?;
    move_to(path, &trash)
}

/// Moves the file to the Recycle Bin.
#[cfg(windows)]
fn trash_file(path: &Path) -> Result<(), String> {
    let path = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; \
         [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{path}', \
         'OnlyErrorDialogs', 'SendToRecycleBin')"
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|error| format!("couldn't run PowerShell: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "PowerShell couldn't move {} to the Recycle Bin",
            path
        ))
    }
}

/// Moves a file into a trash folder, under a name nothing is using.
#[cfg(unix)]
fn move_to(path: &Path, trash: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?;
    let mut destination = trash.join(name);
    let mut n = 2;
    while destination.exists() {
        destination = trash.join(format!("{} {n}", name.to_string_lossy()));
        n += 1;
    }
    match std::fs::rename(path, &destination) {
        Ok(()) => Ok(()),
        // Another volume: copy the file and take the original away.
        Err(_) => {
            std::fs::copy(path, &destination).map_err(|error| {
                format!(
                    "couldn't copy {} into {}: {error}",
                    path.display(),
                    trash.display()
                )
            })?;
            std::fs::remove_file(path)
                .map_err(|error| format!("couldn't remove {}: {error}", path.display()))?;
            Ok(())
        }
    }
}

/// Moves a file into a trash folder, under a name nothing is using.
#[cfg(windows)]
fn move_to(path: &Path, trash: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?;
    let mut destination = trash.join(name);
    let mut n = 2;
    while destination.exists() {
        destination = trash.join(format!("{} {n}", name.to_string_lossy()));
        n += 1;
    }
    std::fs::rename(path, &destination).map_err(|error| {
        format!(
            "couldn't move {} into {}: {error}",
            path.display(),
            trash.display()
        )
    })
}
