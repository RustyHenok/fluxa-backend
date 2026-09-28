mod auth;
mod mapping;
#[allow(clippy::result_large_err)]
mod parsing;
mod server;
mod services;

#[allow(clippy::result_large_err)]
pub mod proto {
    tonic::include_proto!("fluxa.internal.v1");
}

pub use server::serve;
