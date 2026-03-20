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
    println!("cargo:rerun-if-changed=../../proto/knowledge.proto");

    let protos = vec![
        proto_dir.join("core.proto"),
        proto_dir.join("hostexec.proto"),
        proto_dir.join("builder.proto"),
        proto_dir.join("rag.proto"),
        proto_dir.join("ticket.proto"),
        proto_dir.join("workerd.proto"),
        proto_dir.join("remote_repo.proto"),
        proto_dir.join("knowledge.proto"),
    ];

    tonic_build::configure()
        .type_attribute(".", "#[derive(serde::Serialize)]")
        .build_server(true)
        .build_client(true)
        .compile_protos(&protos, &[&proto_dir])?;

    Ok(())
}
