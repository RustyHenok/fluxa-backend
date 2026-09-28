use tonic::service::Interceptor;
use tonic::{Request, Status};

/// Interceptor that requires every gRPC request to carry an
/// `authorization` metadata entry holding the shared bearer token.
#[derive(Clone)]
pub(super) struct SharedTokenInterceptor {
    expected: String,
}

impl SharedTokenInterceptor {
    pub(super) fn new(expected: String) -> Self {
        Self { expected }
    }
}

impl Interceptor for SharedTokenInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let provided = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| Status::unauthenticated("missing bearer token"))?;

        if !constant_time_eq(provided.as_bytes(), self.expected.as_bytes()) {
            return Err(Status::unauthenticated("invalid bearer token"));
        }

        Ok(request)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authed_request(value: Option<&str>) -> Request<()> {
        let mut request = Request::new(());
        if let Some(value) = value {
            request
                .metadata_mut()
                .insert("authorization", value.parse().unwrap());
        }
        request
    }

    #[test]
    fn accepts_matching_bearer_token() {
        let mut interceptor = SharedTokenInterceptor::new("secret-token".into());
        let header = ["Bearer", "secret-token"].join(" ");
        assert!(interceptor.call(authed_request(Some(&header))).is_ok());
    }

    #[test]
    fn rejects_missing_or_wrong_tokens() {
        let mut interceptor = SharedTokenInterceptor::new("secret-token".into());
        let wrong = ["Bearer", "wrong-token"].join(" ");
        assert!(interceptor.call(authed_request(None)).is_err());
        assert!(interceptor.call(authed_request(Some(&wrong))).is_err());
        assert!(
            interceptor
                .call(authed_request(Some("secret-token")))
                .is_err()
        );
    }

    #[test]
    fn constant_time_eq_compares_correctly() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
