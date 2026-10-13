pub mod input_helper;
pub mod ui;
pub mod util_helper;

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Resolve the path to a native dylib.
/// In development: looks in lib/swift/build/
/// In production: looks in the app bundle's Resources/
pub fn dylib_path(app: &AppHandle, name: &str) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();

    // During development, always prefer the freshly rebuilt library in the
    // repository. A copied resource can otherwise stay stale while Rust code
    // has already hot-restarted.
    if cfg!(debug_assertions) {
        let dev_candidates = [
            cwd.join("lib/swift/build").join(name),
            cwd.join("../lib/swift/build").join(name),
        ];
        for path in dev_candidates {
            if path.exists() {
                return path;
            }
        }
    }

    // Bundled production app.
    if let Ok(resource_dir) = app.path().resource_dir() {
        let path = resource_dir.join(name);
        if path.exists() {
            return path;
        }
    }

    // Development fallback: project root lib/swift/build/
    let dev_path = cwd.join("lib/swift/build").join(name);
    if dev_path.exists() {
        return dev_path;
    }

    // Tauri dev runs from src-tauri/, so try parent directory
    let parent_path = cwd.join("../lib/swift/build").join(name);
    if parent_path.exists() {
        return parent_path;
    }

    // Last resort: try relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let path = parent.join("../Resources").join(name);
            if path.exists() {
                return path;
            }
        }
    }

    // Return the dev path even if it doesn't exist (will fail at load time with clear error)
    dev_path
}

/// Initialize all native libraries. Called once during app setup.
pub fn init(app: &AppHandle) -> Result<(), String> {
    util_helper::init(app)?;
    input_helper::init(app)?;
    Ok(())
}
