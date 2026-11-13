use crate::result::CoreError;
use reqwest::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, HeaderMap, HeaderValue, PRAGMA,
};

pub fn request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers
}

pub fn apply_request_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder.headers(request_headers())
}

pub fn validate_response(url: &str, response: &reqwest::Response) -> Result<(), CoreError> {
    validate_headers(url, response.headers())
}

fn validate_headers(url: &str, headers: &HeaderMap) -> Result<(), CoreError> {
    let Some(value) = headers.get(CONTENT_ENCODING) else {
        return Ok(());
    };

    let encoding = value.to_str().map_err(|error| {
        CoreError::Other(format!(
            "download response has invalid Content-Encoding for {url}: {error}"
        ))
    })?;

    let transformed = encoding
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .any(|value| !value.eq_ignore_ascii_case("identity"));

    if transformed {
        return Err(CoreError::Other(format!(
            "download response was transformed by Content-Encoding={encoding} for {url}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{request_headers, validate_headers};
    use reqwest::header::{
        ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, HeaderMap, HeaderValue, PRAGMA,
    };

    #[test]
    fn request_headers_require_identity_encoding() {
        let headers = request_headers();

        assert_eq!(
            headers
                .get(ACCEPT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            Some("identity")
        );
        assert_eq!(
            headers
                .get(CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache, no-transform")
        );
        assert_eq!(
            headers.get(PRAGMA).and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[test]
    fn response_headers_accept_identity_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));

        validate_headers("https://example.com/package.appx", &headers)
            .expect("identity encoding should be accepted");
    }

    #[test]
    fn response_headers_reject_transformed_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let error = validate_headers("https://example.com/package.appx", &headers)
            .expect_err("transformed encoding should be rejected");

        assert!(error.to_string().contains("Content-Encoding=gzip"));
    }
}
