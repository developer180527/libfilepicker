//! Choosing a file to open, on every platform.
//!
//! [`pick`] hands its answer to a callback rather than returning it: phones show
//! their document picker and answer later, from the main thread. Desktops answer
//! before `pick` returns.
//!
//! | Platform | Picker |
//! |---|---|
//! | macOS, Windows, Linux | native dialog (`rfd`; the xdg portal on Linux) |
//! | iOS | `UIDocumentPickerViewController` (not wired up yet: always `None`) |
//! | Android | Storage Access Framework (not wired up yet: always `None`) |

use std::path::{Path, PathBuf};

/// A file the user chose.
///
/// Read it through [`Picked::path`] while this is alive. On iOS a file picked in
/// place (from Files or iCloud) is readable only while access to it is held, and
/// dropping this will give that access up.
#[derive(Debug)]
pub struct Picked {
    path: PathBuf,
}

impl Picked {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into())
    }
}

/// Asks the user for a file to open, and calls `done` with it, or with `None` if
/// they cancelled (or this platform has no picker yet). Call from the main thread.
pub fn pick(done: impl FnOnce(Option<Picked>) + 'static) {
    imp::pick(Box::new(done));
}

type Done = Box<dyn FnOnce(Option<Picked>)>;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    use super::{Done, Picked};

    pub fn pick(done: Done) {
        done(rfd::FileDialog::new().pick_file().map(|path| Picked { path }));
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
mod imp {
    use super::Done;

    pub fn pick(done: Done) {
        done(None);
    }
}
