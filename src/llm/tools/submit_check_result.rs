use serde_json::json;

use crate::llm::provider::ToolSpec;

pub fn submit_check_result() -> ToolSpec {
    ToolSpec {
        name: "submit_check_result".to_string(),
        description: "Submit the final structured check result.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "One-sentence summary of the check result."
                },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "commit": {
                                "type": "string"
                            },
                            "severity": {
                                "type": "string",
                                "enum": ["info", "low", "medium", "high", "critical"]
                            },
                            "message": {
                                "type": "string"
                            },
                            "file": {
                                "type": "string"
                            },
                            "line": {
                                "type": "integer",
                                "minimum": 1
                            }
                        },
                        "required": ["commit", "severity", "message"]
                    }
                }
            },
            "required": ["findings"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_final_result_submission() {
        let tool = submit_check_result();

        assert_eq!(
            tool.description,
            "Submit the final structured check result."
        );
    }

    #[test]
    fn uses_the_common_check_output_schema() {
        let tool = submit_check_result();

        assert_eq!(tool.parameters["required"], json!(["findings"]));
        assert_eq!(
            tool.parameters["properties"]["findings"]["items"]["required"],
            json!(["commit", "severity", "message"])
        );
        assert_eq!(
            tool.parameters["properties"]["findings"]["items"]["properties"]["severity"]["enum"],
            json!(["info", "low", "medium", "high", "critical"])
        );
    }
}
