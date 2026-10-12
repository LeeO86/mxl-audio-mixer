//! Embeds the web UI: `web/dist/index.html` (one self-contained file, `npm run
//! build` in `web/`) when it exists, otherwise a page that says the UI was not
//! built. Rust builds and tests do not need Node.

use std::path::PathBuf;

fn main() {
    let ui = PathBuf::from("web/dist/index.html");
    println!("cargo:rerun-if-changed={}", ui.display());
    let path = if ui.exists() {
        std::fs::canonicalize(&ui).expect("web/dist/index.html")
    } else {
        let out =
            PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("ui-placeholder.html");
        std::fs::write(
            &out,
            "<!doctype html><meta charset=\"utf-8\"><title>mxl-audio-mixer</title>\
             <p>The web UI is not part of this build (<code>cd web &amp;&amp; npm ci &amp;&amp; npm run build</code>, then build again). The API is under <code>/api/v1</code>.</p>",
        )
        .expect("placeholder");
        out
    };
    println!("cargo:rustc-env=MIXER_UI_HTML={}", path.display());
}
