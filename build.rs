fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("failed to fetch protoc");
    // Edition 2024 marks environment mutation as unsafe.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/internal.proto"], &["proto"])
        .expect("failed to compile protobuf definitions");

    println!("cargo:rerun-if-changed=proto/internal.proto");
}
