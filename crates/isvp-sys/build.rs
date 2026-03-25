use std::env;

fn main() {
    let buildroot_dir =
        env::var("ATOMET_BUILDROOT_DIR").unwrap_or("/build/buildroot-2024.02".to_string());
    println!(
        "cargo:rustc-link-search=native={}/output/target/usr/lib",
        buildroot_dir
    );
    // Also search the crate's bundled lib/ directory
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search=native={}/lib", manifest_dir);

    println!("cargo:rustc-link-lib=sysutils");
    println!("cargo:rustc-link-lib=imp");
    println!("cargo:rustc-link-lib=alog");
    println!("cargo:rustc-link-lib=audioProcess");
}
