use serde_json::json;

use super::super::ToolSpec;

#[cfg_attr(not(test), expect(dead_code))]
pub fn submit_review_context_digest() -> ToolSpec {
    ToolSpec {
        name: "submit_review_context_digest".to_string(),
        description: "Submit the faithful, compressed review context.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "overview": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Concise overview of the review intent and agreed direction."
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": [
                                    "requirement",
                                    "decision",
                                    "constraint",
                                    "unresolved",
                                    "superseded"
                                ]
                            },
                            "text": {
                                "type": "string",
                                "minLength": 1
                            },
                            "sources": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "minItems": 1
                            }
                        },
                        "required": ["kind", "text", "sources"],
                        "additionalProperties": false
                    }
                },
                "missing_context": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "minLength": 1
                            },
                            "sources": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "minItems": 1
                            }
                        },
                        "required": ["text", "sources"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["overview", "items", "missing_context"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines_the_review_context_digest_schema() {
        let tool = submit_review_context_digest();

        assert_eq!(tool.name, "submit_review_context_digest");
        assert_eq!(
            tool.parameters["required"],
            json!(["overview", "items", "missing_context"])
        );
        assert_eq!(
            tool.parameters["properties"]["items"]["items"]["properties"]["kind"]["enum"],
            json!([
                "requirement",
                "decision",
                "constraint",
                "unresolved",
                "superseded"
            ])
        );
        assert_eq!(
            tool.parameters["properties"]["items"]["items"]["properties"]["sources"]["minItems"],
            1
        );
        assert_eq!(tool.parameters["additionalProperties"], false);
    }
}
