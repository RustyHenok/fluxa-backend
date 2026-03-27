mod mapping;
mod parsing;
mod server;
mod services;

pub mod proto {
    tonic::include_proto!("fluxa.internal.v1");
}

pub use server::serve;
