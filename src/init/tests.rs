use super::render_config;

#[test]
fn overrides_preserve_toml_formatting_and_other_settings() {
    let template = r#"version = 2 # keep version comment

[stages.knowledge]
max_iterations = 7 # keep stage comment

["llm"] # keep model settings comment
'default_provider'  = "future-provider"   # keep provider comment
"default_model"= "future-model" # keep model comment
max_iterations = 4

["github"] # keep repository settings comment
'repo'  = "old/repository" # keep repository comment

["gitlab"] # keep other repository settings comment
repo = "keep/repository" # keep other repository comment

# Keep review guidance.
[review]
max_commits = 12
"#;

    let rendered = render_config(
        template,
        Some("custom-provider"),
        Some("namespace/new-model"),
        Some("new/repository"),
        false,
    )
    .unwrap();

    assert_eq!(
        rendered,
        template
            .replace("\"future-provider\"", "\"custom-provider\"")
            .replace("\"future-model\"", "\"namespace/new-model\"")
            .replace("\"old/repository\"", "\"new/repository\"")
    );
}

#[test]
fn repository_override_removes_example_before_next_table() {
    assert_repository_example_removed(
        "[llm]\ndefault_provider = \"custom\"\ndefault_model = \"model\"\n",
    );
}

#[test]
fn repository_override_removes_example_before_next_key() {
    assert_repository_example_removed("repo = \"old/repository\"\n");
}

#[test]
fn repository_override_removes_trailing_example() {
    assert_repository_example_removed("");
}

fn assert_repository_example_removed(suffix: &str) {
    const EXAMPLE: &str = "# repo = \"owner/repository\" # Set it to use --github.";
    let template = format!(
        "version = 2\nexample_text = '''\n{EXAMPLE}\n'''\n[github]\n# Keep repository guidance.\n{EXAMPLE}\n# Keep following guidance.\n{suffix}"
    );
    let rendered = render_config(&template, None, None, Some("new/repository"), false).unwrap();

    let mut expected: toml::Value = toml::from_str(&template).unwrap();
    expected["github"]
        .as_table_mut()
        .unwrap()
        .insert("repo".into(), "new/repository".into());
    assert_eq!(toml::from_str::<toml::Value>(&rendered).unwrap(), expected);
    assert_eq!(
        rendered.matches(EXAMPLE).count(),
        1,
        "rendered config: {rendered}"
    );
    assert!(rendered.contains("# Keep repository guidance.\n"));
    assert!(rendered.contains("# Keep following guidance.\n"));
}
