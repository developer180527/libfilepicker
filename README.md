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

| Platform | Picker | Status |
|---|---|---|
| macOS, Windows, Linux | native dialog ([`rfd`](https://crates.io/crates/rfd); the xdg portal on Linux) | done |
| iOS, iPadOS | `UIDocumentPickerViewController` | next |
| Android | Storage Access Framework | later |

Licensed under MIT or Apache-2.0, at your option.
