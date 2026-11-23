use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../tools/recompile.py");
    println!("cargo:rerun-if-env-changed=H717_RECOMP_PRAM");
    println!("cargo:rerun-if-env-changed=H717_RECOMP_RANGES");
    if env::var_os("CARGO_FEATURE_RECOMP").is_none() {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("recompiled.rs");
    let Some(path) = env::var_os("H717_RECOMP_PRAM") else {
        fs::write(
            output,
            "pub(super) fn find_block(_: u32) -> Option<Block> { None }\n",
        )
        .unwrap();
        return;
    };
    let path = fs::canonicalize(path).expect("H717_RECOMP_PRAM must name a PRAM file");
    println!("cargo:rerun-if-changed={}", path.display());
    let ranges = env::var("H717_RECOMP_RANGES").expect("supply the profiled PRAM ranges");
    let status = Command::new("python3")
        .arg("../tools/recompile.py")
        .arg(path)
        .arg(output)
        .args(ranges.split_whitespace())
        .status()
        .expect("start the H717 recompiler");
    assert!(status.success(), "H717 recompiler failed");
}
