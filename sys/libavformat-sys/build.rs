use bindgen::callbacks::{MacroParsingBehavior, ParseCallbacks};
use std::env;
use std::path::PathBuf;

// Apparently, some library in our supply chain redefines the constants
// from cmath as macros (which TBH seems fine) but it breaks bindgen
// unless we do something about it.

#[derive(Clone, Copy, Debug)]
struct OmitMathMacros {}

impl ParseCallbacks for OmitMathMacros {
    fn will_parse_macro(&self, name: &str) -> MacroParsingBehavior {
        match name {
            "FP_NAN" | "FP_INFINITE" | "FP_ZERO" | "FP_SUBNORMAL" | "FP_NORMAL" => {
                MacroParsingBehavior::Ignore
            }
            _ => MacroParsingBehavior::Default,
        }
    }
}

fn main() {
    println!("cargo:rustc-link-lib=avutil");
    println!("cargo:rustc-link-lib=avformat");
    println!("cargo:rerun-if-changed=include/wrapper.h");

    let bindings = bindgen::Builder::default()
        .header("include/wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .parse_callbacks(Box::new(OmitMathMacros {}))
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
