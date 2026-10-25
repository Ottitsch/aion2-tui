fn main() {
    // Delay load Npcap so the exe still starts (and can tell the user to install Npcap) when wpcap.dll is
    // missing. Without this Windows refuses to launch it with "wpcap.dll was not found".
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:wpcap.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
}
