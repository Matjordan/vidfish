fn main() {
    println!("cargo:rerun-if-env-changed=VIDFISH_VERSION");
    println!("cargo:rerun-if-changed=../../assets/vidfish.ico");
    #[cfg(windows)]
    {
        let version = std::env::var("VIDFISH_VERSION").unwrap_or_else(|_| "0.1.0".into());
        let numeric = version.split(['-', '+']).next().unwrap_or("0.1.0");
        let parts: Vec<u64> = numeric.split('.').map(|v| v.parse().unwrap_or(0)).collect();
        let value = (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16);
        winresource::WindowsResource::new()
            .set_icon("../../assets/vidfish.ico")
            .set("ProductName", "Vidfish")
            .set("FileDescription", "Vidfish scientific camera acquisition")
            .set("ProductVersion", &version)
            .set("FileVersion", &version)
            .set_version_info(winresource::VersionInfo::FILEVERSION, value)
            .set_version_info(winresource::VersionInfo::PRODUCTVERSION, value)
            .compile()
            .expect("Windows version and icon resources");
    }
}
