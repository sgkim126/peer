use reqwest::StatusCode;
use serde_json::Value;

use super::LlmCallError;
use crate::console::Console;
use crate::llm::provider::debug::{format_headers_debug, format_json_debug};

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
        let response = self
            .client
            .execute(request)
            .await
            .map_err(map_transport_error)?;

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
