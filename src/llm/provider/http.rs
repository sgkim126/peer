use std::fmt::{self, Display, Formatter};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderName};
use serde_json::Value;
use time::{
    OffsetDateTime,
    format_description::well_known::{Rfc2822, Rfc3339},
};

use crate::console::Console;

use super::{LlmCallError, Request, Response};

const MAX_ATTEMPTS: u32 = 3;
const BASE_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ProviderHttpClient {
    client: reqwest::Client,
    console: Console,
    provider_name: &'static str,
}

impl ProviderHttpClient {
    pub fn new(client: reqwest::Client, console: Console, provider_name: &'static str) -> Self {
        Self {
            client,
            console,
            provider_name,
        }
    }

    pub async fn send(&self, request: Request) -> Result<Response, LlmCallError> {
        let Request { url, headers, body } = request;
        let request = self
            .client
            .post(url)
            .headers(headers)
            .json(&body)
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
        let response = self.execute_with_retries(&request).await?;

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

        Ok(Response { status, body })
    }

    async fn execute_with_retries(
        &self,
        request: &reqwest::Request,
    ) -> Result<reqwest::Response, LlmCallError> {
        for attempt in 1..=MAX_ATTEMPTS {
            let attempt_request = request.try_clone().ok_or_else(|| LlmCallError::Permanent {
                message: format!("failed to clone {} HTTP request", self.provider_name),
                source: Box::new(ProviderHttpError::UncloneableRequest),
            })?;

            match self.client.execute(attempt_request).await {
                Ok(mut response) => {
                    let status = response.status();
                    if !should_retry_status(status) {
                        return Ok(response);
                    }
                    if attempt == MAX_ATTEMPTS {
                        return Ok(response);
                    }
                    let RetryAt::At(retry_at) = retry_at(response.headers(), attempt) else {
                        return Ok(response);
                    };
                    self.console.debug(format_args!(
                        "[{}] {}: response status={} retrying in {:.1}s (attempt {}/{})",
                        self.provider_name,
                        current_utc_time(),
                        status.as_u16(),
                        retry_at
                            .saturating_duration_since(Instant::now())
                            .as_secs_f64(),
                        attempt + 1,
                        MAX_ATTEMPTS,
                    ));
                    while let Ok(Some(_)) = response.chunk().await {}
                    tokio::time::sleep_until(retry_at.into()).await;
                    continue;
                }
                Err(error) => {
                    if !is_retryable_transport_error(&error) {
                        return Err(error.into());
                    }
                    if attempt == MAX_ATTEMPTS {
                        return Err(error.into());
                    }

                    let RetryAt::At(retry_at) =
                        retry_at(&reqwest::header::HeaderMap::new(), attempt)
                    else {
                        return Err(LlmCallError::Permanent {
                            message: format!(
                                "internal {} HTTP retry error: {error}",
                                self.provider_name
                            ),
                            source: Box::new(ProviderHttpError::MissingRetryDelay { attempt }),
                        });
                    };
                    self.console.debug(format_args!(
                        "[{}] {} transport error retrying in {:.1}s (attempt {}/{}): {}",
                        self.provider_name,
                        current_utc_time(),
                        retry_at
                            .saturating_duration_since(Instant::now())
                            .as_secs_f64(),
                        attempt + 1,
                        MAX_ATTEMPTS,
                        error
                    ));
                    tokio::time::sleep_until(retry_at.into()).await;
                    continue;
                }
            }
        }
        Err(LlmCallError::Permanent {
            message: format!("internal {} HTTP loop exited", self.provider_name),
            source: Box::new(ProviderHttpError::RetryLoopExited),
        })
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

fn current_utc_time() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".to_string())
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

fn should_retry_status(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        408 | 409 | 425 | 429 | 500 | 502 | 503 | 504
    )
}

fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryAt {
    At(Instant),
    DoNotRetry,
}

fn retry_at(headers: &reqwest::header::HeaderMap, attempt: u32) -> RetryAt {
    let now = Instant::now();
    if let Some(delay) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let value = value.trim();
            if let Ok(seconds) = value.parse::<u64>() {
                return Some(Duration::from_secs(seconds));
            }

            OffsetDateTime::parse(value, &Rfc2822)
                .ok()
                .map(|retry_time| {
                    (retry_time - OffsetDateTime::now_utc())
                        .try_into()
                        .unwrap_or(Duration::ZERO)
                })
        })
    {
        return if delay <= MAX_RETRY_DELAY {
            RetryAt::At(now + delay)
        } else {
            RetryAt::DoNotRetry
        };
    }

    let multiplier = 1_u32
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    RetryAt::At(
        now + BASE_RETRY_DELAY
            .saturating_mul(multiplier)
            .min(MAX_RETRY_DELAY),
    )
}

#[derive(Debug)]
enum ProviderHttpError {
    MissingRetryDelay { attempt: u32 },
    RetryLoopExited,
    UncloneableRequest,
}

impl std::fmt::Display for ProviderHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRetryDelay { attempt } => {
                write!(f, "missing retry delay for attempt {attempt}")
            }
            Self::RetryLoopExited => f.write_str("retry loop exited without a response or error"),
            Self::UncloneableRequest => f.write_str("request body cannot be replayed"),
        }
    }
}

impl std::error::Error for ProviderHttpError {}

#[cfg(test)]
mod tests {
    use super::*;

    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};

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

    #[test]
    fn retries_rate_limit_status() {
        assert!(should_retry_status(StatusCode::TOO_MANY_REQUESTS));
    }

    #[test]
    fn does_not_retry_auth_status() {
        assert!(!should_retry_status(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn parses_retry_after_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());

        let now = Instant::now();
        let RetryAt::At(retry_at) = retry_at(&headers, 1) else {
            panic!("missing retry time");
        };
        assert!(retry_at.saturating_duration_since(now) >= Duration::from_secs(7));
    }

    #[test]
    fn parses_retry_after_http_date() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 01 Jan 2042 00:00:00 GMT".parse().unwrap(),
        );

        assert_eq!(retry_at(&headers, 1), RetryAt::DoNotRetry);
    }

    #[test]
    fn treats_past_retry_after_http_date_as_immediate_retry() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Sat, 01 Jan 2000 00:00:00 GMT".parse().unwrap(),
        );

        let RetryAt::At(retry_at) = retry_at(&headers, 1) else {
            panic!("missing retry time");
        };
        assert!(retry_at <= Instant::now());
    }

    #[test]
    fn rejects_retry_after_over_max_delay() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());

        assert_eq!(retry_at(&headers, 1), RetryAt::DoNotRetry);
    }

    #[test]
    fn falls_back_to_exponential_delay() {
        let headers = reqwest::header::HeaderMap::new();

        let now = Instant::now();
        let RetryAt::At(first_retry_at) = retry_at(&headers, 1) else {
            panic!("missing first retry time");
        };
        assert!(first_retry_at.saturating_duration_since(now) >= Duration::from_secs(1));

        let now = Instant::now();
        let RetryAt::At(second_retry_at) = retry_at(&headers, 2) else {
            panic!("missing second retry time");
        };
        assert!(second_retry_at.saturating_duration_since(now) >= Duration::from_secs(2));
    }
}
