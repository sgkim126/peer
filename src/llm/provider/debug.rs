use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

pub fn format_json_debug(label: &str, value: &serde_json::Value) -> String {
    match serde_json::to_string_pretty(value) {
        Ok(json) => format!("{label}\n{json}"),
        Err(error) => format!("{label} <failed to serialize JSON: {error}>"),
    }
}

pub fn format_headers_debug(label: &str, headers: &HeaderMap) -> String {
    let mut output = String::from(label);
    for (name, value) in headers {
        output.push('\n');
        output.push_str(name.as_str());
        output.push_str(": ");
        output.push_str(&display_header_value(name, value));
    }
    output
}

fn display_header_value(name: &HeaderName, value: &HeaderValue) -> String {
    if is_sensitive_header(name) {
        return "<redacted>".to_string();
    }

    value
        .to_str()
        .map(str::to_string)
        .unwrap_or_else(|_| "<non-utf8>".to_string())
}

fn is_sensitive_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str().to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "api-key"
            | "cookie"
            | "set-cookie"
    )
}

#[cfg(test)]
mod tests {
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};

    use super::*;

    #[test]
    fn formats_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_static("req-123"),
        );

        let formatted = format_headers_debug("response headers", &headers);

        assert!(formatted.contains("response headers"));
        assert!(formatted.contains("content-type: application/json"));
        assert!(formatted.contains("x-request-id: req-123"));
    }

    #[test]
    fn redacts_sensitive_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        headers.insert(
            HeaderName::from_static("proxy-authorization"),
            HeaderValue::from_static("Basic secret"),
        );
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_static("secret"),
        );
        headers.insert(
            HeaderName::from_static("api-key"),
            HeaderValue::from_static("secret"),
        );
        headers.insert(
            HeaderName::from_static("cookie"),
            HeaderValue::from_static("session=secret"),
        );
        headers.insert(
            HeaderName::from_static("x-goog-api-key"),
            HeaderValue::from_static("secret"),
        );
        headers.insert(
            HeaderName::from_static("set-cookie"),
            HeaderValue::from_static("session=secret"),
        );

        let formatted = format_headers_debug("headers", &headers);

        assert!(formatted.contains("authorization: <redacted>"));
        assert!(formatted.contains("proxy-authorization: <redacted>"));
        assert!(formatted.contains("x-api-key: <redacted>"));
        assert!(formatted.contains("api-key: <redacted>"));
        assert!(formatted.contains("cookie: <redacted>"));
        assert!(formatted.contains("x-goog-api-key: <redacted>"));
        assert!(formatted.contains("set-cookie: <redacted>"));
        assert!(!formatted.contains("secret"));
    }

    #[test]
    fn marks_non_utf8_header_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-binary"),
            HeaderValue::from_bytes(b"\xff").unwrap(),
        );

        let formatted = format_headers_debug("headers", &headers);

        assert!(formatted.contains("x-binary: <non-utf8>"));
    }
}
