use reqwest::StatusCode;
use serde_json::Value;

use std::time::Duration;

use super::LlmCallError;
use crate::console::Console;
use crate::llm::provider::debug::{format_headers_debug, format_json_debug};

const MAX_ATTEMPTS: u32 = 3;
const BASE_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ProviderHttpClient {
    client: reqwest::Client,
    console: Console,
    provider_label: &'static str,
    provider_name: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonHttpResponse {
    pub status: StatusCode,
    pub body: Value,
}

impl ProviderHttpClient {
    pub fn new(
        client: reqwest::Client,
        console: Console,
        provider_label: &'static str,
        provider_name: &'static str,
    ) -> Self {
        Self {
            client,
            console,
            provider_label,
            provider_name,
        }
    }

    pub fn post(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.post(url)
    }

    pub async fn send_json(
        &self,
        request: reqwest::RequestBuilder,
        body: &Value,
    ) -> Result<JsonHttpResponse, LlmCallError> {
        if self.console.is_debug() {
            self.console.debug(format_args!(
                "{}",
                format_json_debug(&format!("{} request", self.provider_label), body)
            ));
        }
        let request = request
            .json(body)
            .build()
            .map_err(|error| LlmCallError::Permanent {
                message: format!("failed to build {} HTTP request", self.provider_name),
                source: Box::new(error),
            })?;
        if self.console.is_debug() {
            self.console.debug(format_args!(
                "{}",
                format_headers_debug(
                    &format!("{} request headers", self.provider_label),
                    request.headers()
                )
            ));
        }
        let response = self.execute_with_retries(&request).await?;

        let status = response.status();
        self.console.debug(format_args!(
            "{} response status={}",
            self.provider_label,
            status.as_u16()
        ));
        if self.console.is_debug() {
            self.console.debug(format_args!(
                "{}",
                format_headers_debug(
                    &format!("{} response headers", self.provider_label),
                    response.headers()
                )
            ));
        }
        let body_text = response
            .text()
            .await
            .map_err(|error| LlmCallError::Permanent {
                message: format!("failed to read {} response body", self.provider_name),
                source: Box::new(error),
            })?;
        self.console.debug(format_args!(
            "{} response body\n{body_text}",
            self.provider_label
        ));
        let body =
            serde_json::from_str::<Value>(&body_text).map_err(|error| LlmCallError::Permanent {
                message: format!("failed to parse {} response JSON", self.provider_name),
                source: Box::new(error),
            })?;

        Ok(JsonHttpResponse { status, body })
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
                Ok(response) => {
                    let status = response.status();
                    if !should_retry_status(status) {
                        return Ok(response);
                    }
                    if attempt == MAX_ATTEMPTS {
                        return Ok(response);
                    }
                    let RetryDelay::Retry(delay) = retry_delay(response.headers(), attempt) else {
                        return Ok(response);
                    };
                    self.console.debug(format_args!(
                        "{} response status={} retrying in {:.1}s (attempt {}/{})",
                        self.provider_label,
                        status.as_u16(),
                        delay.as_secs_f64(),
                        attempt + 1,
                        MAX_ATTEMPTS
                    ));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(error) => {
                    if !is_retryable_transport_error(&error) {
                        return Err(map_transport_error(error));
                    }
                    if attempt == MAX_ATTEMPTS {
                        return Err(map_transport_error(error));
                    }

                    let RetryDelay::Retry(delay) =
                        retry_delay(&reqwest::header::HeaderMap::new(), attempt)
                    else {
                        return Err(self.permanent_http_error(
                            ProviderHttpError::MissingRetryDelay { attempt },
                        ));
                    };
                    self.console.debug(format_args!(
                        "{} transport error retrying in {:.1}s (attempt {}/{}): {}",
                        self.provider_label,
                        delay.as_secs_f64(),
                        attempt + 1,
                        MAX_ATTEMPTS,
                        error
                    ));
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
        }

        Err(self.permanent_http_error(ProviderHttpError::RetryLoopExited))
    }

    fn permanent_http_error(&self, error: ProviderHttpError) -> LlmCallError {
        LlmCallError::Permanent {
            message: format!("internal {} HTTP retry error: {error}", self.provider_name),
            source: Box::new(error),
        }
    }
}

fn map_transport_error(error: reqwest::Error) -> LlmCallError {
    let message = error.to_string();
    if error.is_timeout() || error.is_connect() {
        LlmCallError::Transient {
            message,
            source: Box::new(error),
        }
    } else {
        LlmCallError::Permanent {
            message,
            source: Box::new(error),
        }
    }
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
enum RetryDelay {
    Retry(Duration),
    DoNotRetry,
}

fn retry_delay(headers: &reqwest::header::HeaderMap, attempt: u32) -> RetryDelay {
    if let Some(delay) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
    {
        return if delay <= MAX_RETRY_DELAY {
            RetryDelay::Retry(delay)
        } else {
            RetryDelay::DoNotRetry
        };
    }

    let multiplier = 1_u32
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    RetryDelay::Retry(
        BASE_RETRY_DELAY
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

        assert_eq!(
            retry_delay(&headers, 1),
            RetryDelay::Retry(Duration::from_secs(7))
        );
    }

    #[test]
    fn rejects_retry_after_over_max_delay() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());

        assert_eq!(retry_delay(&headers, 1), RetryDelay::DoNotRetry);
    }

    #[test]
    fn falls_back_to_exponential_delay() {
        let headers = reqwest::header::HeaderMap::new();

        assert_eq!(
            retry_delay(&headers, 1),
            RetryDelay::Retry(Duration::from_secs(1))
        );
        assert_eq!(
            retry_delay(&headers, 2),
            RetryDelay::Retry(Duration::from_secs(2))
        );
    }
}
