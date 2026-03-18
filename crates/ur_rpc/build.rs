fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("proto");

    println!("cargo:rerun-if-changed=../../proto/core.proto");
    println!("cargo:rerun-if-changed=../../proto/hostexec.proto");
    println!("cargo:rerun-if-changed=../../proto/builder.proto");
    println!("cargo:rerun-if-changed=../../proto/rag.proto");
    println!("cargo:rerun-if-changed=../../proto/ticket.proto");
    println!("cargo:rerun-if-changed=../../proto/workerd.proto");
    println!("cargo:rerun-if-changed=../../proto/remote_repo.proto");

    let mut protos = Vec::new();

    // Core is always compiled (default feature)
    protos.push(proto_dir.join("core.proto"));

    if cfg!(feature = "hostexec") {
        protos.push(proto_dir.join("hostexec.proto"));
    }

    if cfg!(feature = "builder") {
        protos.push(proto_dir.join("builder.proto"));
    }

    if cfg!(feature = "rag") {
        protos.push(proto_dir.join("rag.proto"));
    }

    if cfg!(feature = "ticket") {
        protos.push(proto_dir.join("ticket.proto"));
    }

    if cfg!(feature = "workerd") {
        protos.push(proto_dir.join("workerd.proto"));
    }

    if cfg!(feature = "remote_repo") {
        protos.push(proto_dir.join("remote_repo.proto"));
    }

    if !protos.is_empty() {
        tonic_build::configure()
            .type_attribute(".", "#[derive(serde::Serialize)]")
            .build_server(true)
            .build_client(true)
            .compile_protos(&protos, &[&proto_dir])?;
    }

    Ok(())
}
