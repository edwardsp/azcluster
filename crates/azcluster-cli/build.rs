use std::path::Path;

// `src/main.rs` embeds `bicep/main.json` via `include_str!`. That file is generated
// from `bicep/*.bicep` and not committed, so turn a missing file into an actionable
// error rather than the opaque `include_str!` "couldn't read file" failure.
fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let main_json = Path::new(manifest_dir).join("../../bicep/main.json");
    println!("cargo:rerun-if-changed={}", main_json.display());

    if !main_json.exists() {
        panic!(
            "\n\n\
             bicep/main.json is missing — it is generated from Bicep and not committed.\n\
             Build the ARM template before compiling the CLI:\n\n    \
             az bicep build --file bicep/main.bicep --outfile bicep/main.json\n\n\
             (or, with the standalone Bicep CLI:)\n\n    \
             bicep build bicep/main.bicep --outfile bicep/main.json\n\n"
        );
    }
}
