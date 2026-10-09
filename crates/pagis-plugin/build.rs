//! A release build embeds each first-party Skill file with
//! `include_bytes!`, so Cargo tracks the files that exist but not the
//! directory. Watching the directory rebuilds the crate when a Skill
//! is added or removed.

fn main() {
    println!("cargo:rerun-if-changed=../../computer/skills");
}
