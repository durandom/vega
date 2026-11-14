fn main() {
    vega_build::emit();
    println!("cargo:rerun-if-changed=../../config/herdr-app/mods/vega-services/helper.py");
}
