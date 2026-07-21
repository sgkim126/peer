#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Provider-specific opaque state that must be replayed with the tool call.
    pub provider_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmResponse {
    ToolCalls(Vec<ToolCall>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmCallResult {
    pub response: LlmResponse,
    pub usage: RawUsage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::AddAssign for RawUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_usage_add_assign_accumulates() {
        let input_tokens1 = 100;
        let input_tokens2 = 200;
        let output_tokens1 = 50;
        let output_tokens2 = 75;
        let mut total = RawUsage::default();
        total += RawUsage {
            input_tokens: input_tokens1,
            output_tokens: output_tokens1,
        };
        total += RawUsage {
            input_tokens: input_tokens2,
            output_tokens: output_tokens2,
        };
        assert_eq!(
            total,
            RawUsage {
                input_tokens: input_tokens1 + input_tokens2,
                output_tokens: output_tokens1 + output_tokens2,
            }
        );
    }
}
