use crate::llm::result::CheckOutput;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum LlmResponse {
    CheckOutput(CheckOutput),
    ToolCalls(Vec<ToolCall>),
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct LlmCallResult {
    pub response: LlmResponse,
    pub usage: RawUsage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
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
