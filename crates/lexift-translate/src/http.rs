use std::time::Duration;

use lexift_core::{Error, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Builds the client shared by every configured production provider.
pub(crate) fn build_client() -> Result<reqwest::Client> {
    build_client_with_timeouts(CONNECT_TIMEOUT, REQUEST_TIMEOUT, true)
}

fn build_client_with_timeouts(
    connect_timeout: Duration,
    request_timeout: Duration,
    use_system_proxy: bool,
) -> Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .user_agent(concat!("Lexift/", env!("CARGO_PKG_VERSION")))
        .retry(reqwest::retry::never());
    let builder = if use_system_proxy {
        builder
    } else {
        builder.no_proxy()
    };
    builder.build().map_err(map_error)
}

#[cfg(test)]
pub(crate) fn build_test_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<reqwest::Client> {
    build_client_with_timeouts(connect_timeout, request_timeout, false)
}

/// Converts transport failures at the adapter boundary without exposing request details.
pub(crate) fn map_error(error: reqwest::Error) -> Error {
    let message = if error.is_connect() {
        "Could not connect to the translation service"
    } else if error.is_timeout() {
        "Translation request timed out"
    } else if error.is_decode() {
        "Translation service returned invalid JSON"
    } else if error.is_body() {
        "Could not read the translation service response"
    } else if error.is_builder() {
        "Could not build the translation request"
    } else if error.is_request() {
        "Translation HTTP request failed"
    } else {
        "Translation HTTP operation failed"
    };

    Error::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_shared_http_client() {
        build_client().expect("shared HTTP client should build");
    }

    #[test]
    fn maps_builder_errors_without_leaking_request_details() {
        let error = reqwest::Client::new()
            .get("\n")
            .build()
            .expect_err("invalid URL should fail request construction");

        assert_eq!(
            map_error(error).to_string(),
            "Could not build the translation request"
        );
    }
}
