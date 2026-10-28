fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("FileDescription", "SyncClipboard Lite Client");
        res.set("ProductName", "SyncClipboard Lite");
        res.set("ProductVersion", "1.0.0");
        res.set("CompanyName", "SyncClipboard");
        res.set("LegalCopyright", "Copyright (C) 2026 SyncClipboard");
        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile Windows resource: {}", e);
        }
    }
}
