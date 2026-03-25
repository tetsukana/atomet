use std::fs;
use std::process::Command;

fn main() {
    let bin = "/opt/mipsel-gcc472-glibc216/bin";
    let out_dir = std::env::var("OUT_DIR").unwrap();

    let c_files: Vec<_> = fs::read_dir("native")
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "c"))
        .collect();

    let mut obj_files = vec![];
    for src in &c_files {
        let obj = format!(
            "{}/{}.o",
            out_dir,
            src.file_stem().unwrap().to_str().unwrap()
        );
        let output = Command::new(format!("{}/mips-linux-gnu-gcc", bin))
            .args([
                "-c",
                src.to_str().unwrap(),
                "-std=gnu99",
                "-mmxu2",
                "-fPIC",
                "-lm",
                "-O2",
                "-o",
                &obj,
            ])
            .output()
            .expect("Failed to execute GCC");

        if !output.status.success() {
            eprintln!("GCC failed with status: {}", output.status);
            eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
            std::process::exit(1);
        }

        obj_files.push(obj);
    }

    let lib = format!("{}/libimgproc.a", out_dir);
    let output = Command::new(format!("{}/mips-linux-gnu-ar", bin))
        .args(["rcs", &lib])
        .args(&obj_files)
        .output()
        .expect("Failed to execute ar");
    if !output.status.success() {
        eprintln!("ar failed with status: {}", output.status);
        std::process::exit(1);
    }

    println!("cargo:rustc-link-lib=static=imgproc");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rerun-if-changed=native/");
}
