use reqwest::StatusCode;
use serde_json::Value;

use super::LlmCallError;

#[derive(Debug, Clone)]
pub struct ProviderHttpClient {
    client: reqwest::Client,
    provider_name: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonHttpResponse {
    pub status: StatusCode,
    pub body: Value,
}

impl ProviderHttpClient {
    #[expect(dead_code)]
    pub fn new(client: reqwest::Client, provider_name: &'static str) -> Self {
        Self {
            client,
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
        let response = self.client.execute(request).await?;

        let status = response.status();
        let body_text = response.text().await?;
        let body =
            serde_json::from_str::<Value>(&body_text).map_err(|error| LlmCallError::Permanent {
                message: format!("failed to parse {} response JSON", self.provider_name),
                source: Box::new(error),
            })?;

        Ok(JsonHttpResponse { status, body })
    }
}
