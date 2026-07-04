//! Windows: embed the file icon and version metadata into `plusplus.exe`, so the binary
//! carries its identity in Explorer/taskbar before the app sets its runtime icon.
//!
//! `cfg(windows)` below is the *host* platform (build scripts compile for the host), which
//! matches how release exes are produced: natively on a Windows runner. Cross-compiled
//! Windows builds from other hosts would skip the resource — acceptable, since only CI
//! artifacts ship.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/icon.ico");
        res.set("ProductName", "plusplus");
        res.set("FileDescription", "plusplus — native database GUI");
        res.compile()
            .expect("failed to embed Windows icon/version resources");
    }
}
