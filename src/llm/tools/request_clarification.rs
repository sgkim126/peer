use serde_json::json;

use super::super::ToolSpec;

pub fn request_clarification() -> ToolSpec {
    ToolSpec {
        name: "request_clarification".to_string(),
        description: "Request clarification about a specific fact that is necessary to assess a concrete potential finding and unavailable from the supplied context or repository tools. Each question must identify the missing fact, affected code or behavior, and why it changes the assessment.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "minLength": 1
                    },
                    "minItems": 1,
                    "description": "Questions for the user. Each question must explain why the missing information is needed."
                }
            },
            "required": ["questions"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_clarification_requests() {
        let tool = request_clarification();

        assert_eq!(tool.name, "request_clarification");
        assert!(tool.description.contains("Request clarification"));
    }

    #[test]
    fn requires_at_least_one_non_empty_string_question() {
        let tool = request_clarification();

        assert_eq!(tool.parameters["required"], json!(["questions"]));
        assert_eq!(tool.parameters["properties"]["questions"]["type"], "array");
        assert_eq!(tool.parameters["properties"]["questions"]["minItems"], 1);
        assert_eq!(
            tool.parameters["properties"]["questions"]["items"]["type"],
            "string"
        );
        assert_eq!(
            tool.parameters["properties"]["questions"]["items"]["minLength"],
            1
        );
    }
}
