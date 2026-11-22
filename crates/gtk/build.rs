//! Gives the Windows .exe its icon and version details. Nothing to do elsewhere.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../packaging/windows/familiar.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../packaging/windows/familiar.ico");
        resource.set("ProductName", "Familiar");
        resource.set("FileDescription", "Familiar");
        if let Err(e) = resource.compile() {
            // An .exe without an icon still works; don't fail the build over it.
            println!("cargo:warning=couldn't embed the icon: {e}");
        }
    }
}
