//! Choosing a file to open, on every platform.
//!
//! [`pick`] and [`pick_with`] hand their answer to a callback rather than returning
//! it: phones show their document picker and answer later, from the main thread.
//! Desktops answer before the call returns.
//!
//! | Platform | Picker |
//! |---|---|
//! | macOS, Windows, Linux | native dialog (`rfd`; the xdg portal on Linux) |
//! | iOS, iPadOS | `UIDocumentPickerViewController`, opening files and folders in place |
//! | Android | Storage Access Framework (not wired up yet: always `None`) |

use std::path::{Path, PathBuf};

/// A file, or on iOS a folder, the user chose.
///
/// Read it through [`Picked::path`] while this is alive. On iOS a file picked in
/// place (from Files or iCloud) is readable only while access to it is held, and
/// dropping this gives that access up. Access to a file covers that file alone;
/// access to a folder covers everything under it, which is how an app reads a
/// file together with the files next to it (a model and its textures). Not `Send`: on iOS that access belongs to
/// the main thread's objects.
#[derive(Debug)]
pub struct Picked {
    path: PathBuf,
    _access: imp::Access,
}

impl Picked {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into())
    }
}

/// What to ask the user for, and where to start looking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickOptions {
    /// Let the user choose a folder. On iOS the picker then offers files and folders
    /// both; desktop dialogs choose one or the other, so there it picks a folder.
    pub folders: bool,
    /// Open the picker in this folder (a model's folder, so granting it is one tap).
    /// Ignored when it doesn't exist; the system's own default is used then.
    pub start_in: Option<PathBuf>,
}

/// Asks the user for a file to open (on iOS, a file or a folder), and calls `done`
/// with it, or with `None` if they cancelled (or this platform has no picker yet).
/// Shorthand for [`pick_with`] with folders allowed on iOS only, where the picker
/// can offer both at once.
pub fn pick(done: impl FnOnce(Option<Picked>) + 'static) {
    pick_with(PickOptions { folders: cfg!(target_os = "ios"), start_in: None }, done);
}

/// Asks the user for a file or folder as `opts` says, and calls `done` with it, or
/// with `None` if they cancelled (or this platform has no picker yet). Call from the
/// main thread; `done` is called on it too, once the picker is off screen. While a
/// picker is already showing, another call answers `None` at once.
pub fn pick_with(opts: PickOptions, done: impl FnOnce(Option<Picked>) + 'static) {
    imp::pick(opts, Box::new(done));
}

type Done = Box<dyn FnOnce(Option<Picked>)>;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    use std::marker::PhantomData;

    use super::{Done, PickOptions, Picked};

    /// Nothing to hold on desktops; not `Send`, to match iOS.
    #[derive(Debug)]
    pub struct Access(PhantomData<*const ()>);

    pub fn pick(opts: PickOptions, done: Done) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = opts.start_in.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        // The dialog is modal and answers before it returns.
        let path = if opts.folders { dialog.pick_folder() } else { dialog.pick_file() };
        done(path.map(|path| Picked { path, _access: Access(PhantomData) }));
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::path::PathBuf;

    use block2::{Block, RcBlock};
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send};

    use super::{Done, PickOptions, Picked};

    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "UniformTypeIdentifiers", kind = "framework")]
    unsafe extern "C" {
        /// `UTTypeItem`: any file.
        static UTTypeItem: &'static AnyObject;
        /// `UTTypeFolder`: folders, which the picker offers only when asked for them
        /// by name, though a folder is an item too.
        static UTTypeFolder: &'static AnyObject;
    }

    /// The picked file's URL, while we hold security-scoped access to it.
    #[derive(Debug)]
    pub struct Access(Option<Retained<AnyObject>>);

    impl Drop for Access {
        fn drop(&mut self) {
            if let Some(url) = &self.0 {
                // SAFETY: an NSURL whose access we started (see `picked`).
                unsafe { msg_send![url, stopAccessingSecurityScopedResource] }
            }
        }
    }

    define_class!(
        /// The picker's delegate: hands the answer to `done`, once.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "LibFilePickerDelegate"]
        #[ivars = RefCell<Option<Done>>]
        struct Delegate;

        impl Delegate {
            #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
            fn did_pick(&self, picker: &AnyObject, urls: &AnyObject) {
                // SAFETY: UIKit passes an NSArray of NSURLs.
                let url: Option<Retained<AnyObject>> = unsafe { msg_send![urls, firstObject] };
                self.finish(picker, url.and_then(picked));
            }

            #[unsafe(method(documentPickerWasCancelled:))]
            fn cancelled(&self, picker: &AnyObject) {
                self.finish(picker, None);
            }
        }
    );

    impl Delegate {
        /// Calls `done` once the picker has finished going away. UIKit tells the
        /// delegate while the sheet is still sliding down, and frames the app draws
        /// under it then never reach the screen: an app that redraws once and goes
        /// idle would be left showing its last frame from before the picker.
        fn finish(&self, picker: &AnyObject, picked: Option<Picked>) {
            let Some(done) = self.ivars().borrow_mut().take() else { return };
            // SAFETY: a UIViewController; `transitionCoordinator` is nil when it
            // isn't moving, and the completion block runs once, on the main thread.
            unsafe {
                let coordinator: Option<Retained<AnyObject>> = msg_send![picker, transitionCoordinator];
                let Some(coordinator) = coordinator else { return done(picked) };
                let once = std::rc::Rc::new(Cell::new(Some((done, picked))));
                let pending = once.clone();
                let completion = RcBlock::new(move |_context: *mut AnyObject| {
                    if let Some((done, picked)) = pending.take() {
                        done(picked);
                    }
                });
                let queued: bool = msg_send![&coordinator, animateAlongsideTransition: std::ptr::null::<Block<dyn Fn(*mut AnyObject)>>(), completion: &*completion];
                // NO means UIKit couldn't queue it and will never call the completion:
                // answer now rather than lose the pick.
                if !queued && let Some((done, picked)) = once.take() {
                    done(picked);
                }
            }
        }

        /// Still waiting for the user: its `done` hasn't been called.
        fn waiting(&self) -> bool {
            self.ivars().borrow().is_some()
        }
    }

    thread_local! {
        /// The picker only holds its delegate weakly; this keeps the current one
        /// alive until the next pick replaces it.
        static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    }

    fn picked(url: Retained<AnyObject>) -> Option<Picked> {
        // SAFETY: an NSURL from the picker; `path` returns an NSString or nil, and
        // its UTF8String lives as long as the string.
        unsafe {
            let started: bool = msg_send![&url, startAccessingSecurityScopedResource];
            let access = Access(started.then(|| url.clone()));
            let path: Retained<AnyObject> = msg_send![&url, path];
            let utf8: *const c_char = msg_send![&path, UTF8String];
            let path = PathBuf::from(CStr::from_ptr(utf8).to_str().ok()?);
            Some(Picked { path, _access: access })
        }
    }

    /// The view controller on top in the key window: the one that can present.
    fn presenter(_: MainThreadMarker) -> Option<Retained<AnyObject>> {
        // SAFETY: UIKit classes and selectors from its headers, on the main thread.
        unsafe {
            let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
            let scenes: Retained<AnyObject> = msg_send![&app, connectedScenes];
            let scenes: Retained<AnyObject> = msg_send![&scenes, allObjects];
            let count: usize = msg_send![&scenes, count];
            let mut window: Option<Retained<AnyObject>> = None;
            for i in 0..count {
                let scene: Retained<AnyObject> = msg_send![&scenes, objectAtIndex: i];
                let is_window_scene: bool = msg_send![&scene, isKindOfClass: class!(UIWindowScene)];
                if is_window_scene {
                    window = msg_send![&scene, keyWindow];
                    if window.is_none() {
                        let windows: Retained<AnyObject> = msg_send![&scene, windows];
                        window = msg_send![&windows, firstObject];
                    }
                    if window.is_some() {
                        break;
                    }
                }
            }
            let mut top: Retained<AnyObject> = msg_send![&window?, rootViewController];
            while let Some(next) = msg_send![&top, presentedViewController] {
                top = next;
            }
            Some(top)
        }
    }

    /// `NSURL` for a folder, or `None` when it doesn't exist.
    fn folder_url(dir: &std::path::Path) -> Option<Retained<AnyObject>> {
        let dir = dir.is_dir().then(|| dir.to_str()).flatten()?;
        let c = std::ffi::CString::new(dir).ok()?;
        // SAFETY: Foundation's own constructors with a NUL-terminated UTF-8 string.
        unsafe {
            let s: Option<Retained<AnyObject>> = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
            msg_send![class!(NSURL), fileURLWithPath: &*s?, isDirectory: true]
        }
    }

    pub fn pick(opts: PickOptions, done: Done) {
        let Some(mtm) = MainThreadMarker::new() else { return done(None) };
        // One picker at a time: a second would be presented over the first, and the
        // first's answer would have nowhere to go.
        if DELEGATE.with(|d| d.borrow().as_ref().is_some_and(|d| d.waiting())) {
            return done(None);
        }
        let Some(presenter) = presenter(mtm) else { return done(None) };
        let delegate = Delegate::alloc(mtm).set_ivars(RefCell::new(Some(done)));
        // SAFETY: NSObject's init; then UIKit's own classes and selectors, on the
        // main thread. `asCopy: NO` opens the file where it is (Files, iCloud),
        // which is why `picked` takes security-scoped access.
        unsafe {
            let delegate: Retained<Delegate> = msg_send![super(delegate), init];
            let all = [UTTypeItem, UTTypeFolder];
            let types = if opts.folders { &all[..] } else { &all[..1] };
            let types: Retained<AnyObject> = msg_send![class!(NSArray), arrayWithObjects: types.as_ptr(), count: types.len()];
            let picker: Allocated<AnyObject> = msg_send![class!(UIDocumentPickerViewController), alloc];
            let picker: Retained<AnyObject> = msg_send![picker, initForOpeningContentTypes: &*types, asCopy: false];
            let () = msg_send![&picker, setDelegate: &*delegate];
            if let Some(url) = opts.start_in.as_deref().and_then(folder_url) {
                let () = msg_send![&picker, setDirectoryURL: &*url];
            }
            let () = msg_send![&presenter, presentViewController: &*picker, animated: true, completion: std::ptr::null::<AnyObject>()];
            DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::marker::PhantomData;

    use super::{Done, PickOptions};

    #[derive(Debug)]
    pub struct Access(PhantomData<*const ()>);

    pub fn pick(_: PickOptions, done: Done) {
        done(None);
    }
}
