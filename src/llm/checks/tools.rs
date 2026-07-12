use crate::llm::provider::ToolSpec;

pub fn get_commit_message() -> ToolSpec {
    revision_tool(
        "get_commit_message",
        "Returns the full commit message for a commit.",
    )
}

pub fn get_commit_diff() -> ToolSpec {
    revision_tool(
        "get_commit_diff",
        "Returns the full unified diff for a commit.",
    )
}

pub fn get_changed_files() -> ToolSpec {
    revision_tool(
        "get_changed_files",
        "Returns the files changed in a commit.",
    )
}

pub fn get_commits_in_range() -> ToolSpec {
    ToolSpec {
        name: "get_commits_in_range".to_string(),
        description: "Returns commit hashes in a two-dot range, oldest to newest.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "range": {
                    "type": "string",
                    "description": "Git two-dot range."
                }
            },
            "required": ["range"]
        }),
    }
}

pub fn get_file_content() -> ToolSpec {
    ToolSpec {
        name: "get_file_content".to_string(),
        description: "Returns a file's content at a commit.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision at which to read the file."
                },
                "path": {
                    "type": "string",
                    "description": "Repository-root-relative path."
                },
                "start_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional first line to return. Must be provided with end_line."
                },
                "end_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional last line to return. Must be provided with start_line."
                }
            },
            "allOf": [
                {
                    "if": { "required": ["start_line"] },
                    "then": { "required": ["end_line"] }
                },
                {
                    "if": { "required": ["end_line"] },
                    "then": { "required": ["start_line"] }
                }
            ],
            "required": ["path", "revision"]
        }),
    }
}

pub fn grep_search() -> ToolSpec {
    ToolSpec {
        name: "grep_search".to_string(),
        description: "Searches a commit snapshot with a regular expression and returns matching lines with optional surrounding context.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Regular expression to search for."
                },
                "revision": {
                    "type": "string",
                    "description": "Git revision whose snapshot to search."
                },
                "path": {
                    "type": "string",
                    "description": "Optional repository-root-relative file or directory to search."
                },
                "context_lines": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 10,
                    "description": "Optional number of surrounding lines to return for each match."
                }
            },
            "required": ["query", "revision"]
        }),
    }
}

pub fn request_user_info() -> ToolSpec {
    ToolSpec {
        name: "request_user_info".to_string(),
        description: "Stop the check and ask the user for information that is necessary to complete the check but is not available from the provided context or repository tools. Do not ask for information that can be obtained with the other available tools. Each question must include enough context to explain why the information is needed.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "description": "Questions for the user. Include the reason the information is needed in each question."
                }
            },
            "required": ["questions"]
        }),
    }
}

fn revision_tool(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision resolving to a commit."
                }
            },
            "required": ["revision"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_search_requires_query_and_revision() {
        let tool = grep_search();

        assert_eq!(tool.name, "grep_search");
        assert_eq!(
            tool.parameters["required"],
            serde_json::json!(["query", "revision"])
        );
        assert_eq!(
            tool.parameters["properties"]["context_lines"]["maximum"],
            10
        );
    }

    #[test]
    fn get_file_content_accepts_an_optional_line_range() {
        let tool = get_file_content();

        assert_eq!(
            tool.parameters["required"],
            serde_json::json!(["path", "revision"])
        );
        assert_eq!(tool.parameters["properties"]["start_line"]["minimum"], 1);
        assert_eq!(tool.parameters["properties"]["end_line"]["minimum"], 1);
    }
}
