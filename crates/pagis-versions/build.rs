fn main() {
    println!("cargo:rerun-if-env-changed=PAGIS_COMPUTER_IMAGE");
    let Ok(image) = std::env::var("PAGIS_COMPUTER_IMAGE") else {
        return;
    };
    let prefix = "ghcr.io/pagis-co/pagis-computer@sha256:";
    let digest = image
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("PAGIS_COMPUTER_IMAGE must name the immutable {prefix}<digest>"));
    assert!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "PAGIS_COMPUTER_IMAGE must carry a lowercase SHA-256 digest"
    );
    println!("cargo:rustc-env=PAGIS_COMPUTER_IMAGE={image}");
}
