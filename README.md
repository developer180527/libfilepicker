# libfilepicker

Choosing a file to open, on every platform, behind one call:

```rust
libfilepicker::pick(|picked| {
    if let Some(file) = picked {
        open(file.path());
    }
});
```

`pick` hands its answer to a callback because phones show their picker and
answer later; desktops answer before it returns.

`pick_with` takes `PickOptions`: `folders` lets the user choose a folder, and
`start_in` opens the picker in a given folder. On iOS a picked folder grants
access to everything under it (a model and the textures beside it); a picked
file grants that file alone. Keep the `Picked` alive while reading: dropping it
gives the access up.

| Platform | Picker | Status |
|---|---|---|
| macOS, Windows, Linux | native dialog ([`rfd`](https://crates.io/crates/rfd); the xdg portal on Linux) | done |
| iOS, iPadOS | `UIDocumentPickerViewController`, files and folders opened in place | done |
| Android | Storage Access Framework | later |

Licensed under MIT or Apache-2.0, at your option.
