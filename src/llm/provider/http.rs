use std::fmt::{self, Display, Formatter};

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderName};
use serde_json::Value;

use super::LlmCallError;
use crate::console::Console;

#[derive(Debug, Clone)]
pub struct ProviderHttpClient {
    client: reqwest::Client,
    console: Console,
    provider_name: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonHttpResponse {
    pub status: StatusCode,
    pub body: Value,
}

impl ProviderHttpClient {
    #[expect(dead_code)]
    pub fn new(client: reqwest::Client, console: Console, provider_name: &'static str) -> Self {
        Self {
            client,
            console,
            provider_name,
        }
    }

    #[expect(dead_code)]
    pub fn post(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.post(url)
    }

    #[expect(dead_code)]
    pub async fn send_json(
        &self,
        request: reqwest::RequestBuilder,
        body: &Value,
    ) -> Result<JsonHttpResponse, LlmCallError> {
        let request = request
            .json(body)
            .build()
            .map_err(|error| LlmCallError::Permanent {
                message: format!("failed to build {} HTTP request", self.provider_name),
                source: Box::new(error),
            })?;
        self.console.debug(format_args!(
            "[{}] request header:\n{}",
            self.provider_name,
            RedactedHeaders {
                headers: request.headers()
            }
        ));
        self.console.debug(format_args!(
            "[{}] request body:\n{}",
            self.provider_name, body
        ));
        let response = self.client.execute(request).await?;

        let status = response.status();
        self.console.debug(format_args!(
            "[{}] response status: {}",
            self.provider_name,
            status.as_u16()
        ));
        self.console.debug(format_args!(
            "[{}] response header:\n{}",
            self.provider_name,
            RedactedHeaders {
                headers: response.headers()
            }
        ));
        let body_text = response.text().await?;
        self.console.debug(format_args!(
            "[{}] response body:\n{body_text}",
            self.provider_name
        ));
        let body =
            serde_json::from_str::<Value>(&body_text).map_err(|error| LlmCallError::Permanent {
                message: format!("failed to parse {} response JSON", self.provider_name),
                source: Box::new(error),
            })?;

        Ok(JsonHttpResponse { status, body })
    }
}

struct RedactedHeaders<'a> {
    headers: &'a HeaderMap,
}

impl Display for RedactedHeaders<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let mut headers = self.headers.iter().peekable();

        while let Some((name, value)) = headers.next() {
            if is_sensitive_header(name) {
                write!(formatter, "{name}: <******>")?;
            } else {
                let value = value
                    .to_str()
                    .map(str::to_string)
                    .unwrap_or_else(|_| "<non-utf8>".to_string());
                write!(formatter, "{name}: {value}")?;
            }

            if headers.peek().is_some() {
                writeln!(formatter)?;
            }
        }
        Ok(())
    }
}

fn is_sensitive_header(name: &HeaderName) -> bool {
    const SENSITIVE_KEYWORDS: [&str; 7] = [
        "authorization",
        "api",
        "key",
        "cookie",
        "token",
        "secret",
        "password",
    ];
    let name = name.as_str().to_ascii_lowercase();
    SENSITIVE_KEYWORDS
        .iter()
        .any(|sensitive| name.contains(sensitive))
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

        let formatted = (RedactedHeaders { headers: &headers }).to_string();

        assert!(formatted.contains("content-type: application/json"));
        assert!(formatted.contains("x-request-id: req-123"));
        assert_eq!(formatted.matches('\n').count(), headers.len() - 1);
        assert!(!formatted.ends_with('\n'));
    }

    #[test]
    fn redacts_sensitive_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("proxy-authorization"),
            HeaderValue::from_static("Proxy SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_static("SHOULD_BE_REDACTED_api_key"),
        );
        headers.insert(
            HeaderName::from_static("api-key"),
            HeaderValue::from_static("SHOULD_BE_REDACTED_api_key"),
        );
        headers.insert(
            HeaderName::from_static("cookie"),
            HeaderValue::from_static("s_id=SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("x-goog-api-key"),
            HeaderValue::from_static("SHOULD_BE_REDACTED_api_key"),
        );
        headers.insert(
            HeaderName::from_static("set-cookie"),
            HeaderValue::from_static("s=SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("x-csrf-token"),
            HeaderValue::from_static("SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("x-auth-token"),
            HeaderValue::from_static("x-SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("x-secret"),
            HeaderValue::from_static("x-SHOULD_BE_REDACTED"),
        );
        headers.insert(
            HeaderName::from_static("password"),
            HeaderValue::from_static("x-SHOULD_BE_REDACTED"),
        );

        let formatted = (RedactedHeaders { headers: &headers }).to_string();

        assert!(formatted.contains("authorization: <******>"));
        assert!(formatted.contains("proxy-authorization: <******>"));
        assert!(formatted.contains("x-api-key: <******>"));
        assert!(formatted.contains("api-key: <******>"));
        assert!(formatted.contains("cookie: <******>"));
        assert!(formatted.contains("x-goog-api-key: <******>"));
        assert!(formatted.contains("set-cookie: <******>"));
        assert!(formatted.contains("x-csrf-token: <******>"));
        assert!(formatted.contains("x-auth-token: <******>"));
        assert!(formatted.contains("x-secret: <******>"));
        assert!(formatted.contains("password: <******>"));
        assert!(!formatted.contains("SHOULD_BE_REDACTED"));
    }

    #[test]
    fn redacts_headers_containing_sensitive_terms() {
        for name in [
            "x-authorization-token",
            "api-version",
            "client-key-id",
            "my-cookie-id",
            "x-access-token",
            "client-secret",
            "x-password-hash",
        ] {
            assert!(is_sensitive_header(&HeaderName::from_static(name)));
        }
    }

    #[test]
    fn marks_non_utf8_header_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-binary"),
            HeaderValue::from_bytes(b"\xff").unwrap(),
        );

        let formatted = (RedactedHeaders { headers: &headers }).to_string();

        assert!(formatted.contains("x-binary: <non-utf8>"));
    }
}
