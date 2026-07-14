fn main() {
    // On MSVC, delay-load the Vulkan loader. ggml-vulkan links `vulkan-1.dll` at
    // load time; without delay-load this binary fails to *start* on a machine
    // that has no Vulkan driver (VM / Server / Basic Display Adapter). With it,
    // the DLL is only touched on the first Vulkan call — which the probe makes
    // behind a dynamic `ash::Entry::load()` that returns Err instead of crashing.
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
