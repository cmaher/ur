fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("proto");

    println!("cargo:rerun-if-changed=../../proto/core.proto");
    println!("cargo:rerun-if-changed=../../proto/git.proto");
    println!("cargo:rerun-if-changed=../../proto/gh.proto");

    let mut protos = Vec::new();

    // Core is always compiled (default feature)
    protos.push(proto_dir.join("core.proto"));

    if cfg!(feature = "git") {
        protos.push(proto_dir.join("git.proto"));
    }

    if cfg!(feature = "gh") {
        protos.push(proto_dir.join("gh.proto"));
    }

    if !protos.is_empty() {
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&protos, &[&proto_dir])?;
    }

    Ok(())
}
