use super::render_config;

#[test]
fn repository_override_only_sets_github_and_preserves_gitlab_guidance() {
    let rendered = render_config(
        crate::config::DEFAULT_CONFIG_TOML,
        None,
        None,
        Some("owner/repo"),
    )
    .unwrap();
    let config: crate::config::Config = toml::from_str(&rendered).unwrap();
    assert_eq!(config.github.repo.as_deref(), Some("owner/repo"));
    assert_eq!(config.gitlab.repo, None);
    assert!(rendered.contains("# repo = \"group/subgroup/project\""));
}

#[test]
fn overrides_follow_toml_keys_and_preserve_surrounding_formatting() {
    let template = r#"version = 2 # keep version comment

[stages.knowledge]
max_iterations = 7 # keep stage comment

["llm"] # keep model settings comment
'default_provider'  = "future-provider"   # keep provider comment
"default_model"= "future-model" # keep model comment
max_iterations = 4

["github"] # keep repository settings comment
'repo'  = "old/repository" # keep repository comment

# Keep review guidance.
[review]
max_commits = 12
"#;

    let rendered = render_config(
        template,
        Some("custom-provider"),
        Some("namespace/new-model"),
        Some("new/repository"),
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
fn repository_override_removes_only_the_example_from_comment_metadata() {
    const EXAMPLE: &str = "# repo = \"owner/repository\" # Set it to use --github.";

    for suffix in [
        "[llm]\ndefault_provider = \"custom\"\ndefault_model = \"model\"\n",
        "repo = \"old/repository\"\n",
        "",
    ] {
        let template = format!(
            "version = 2\n[github]\n# Keep repository guidance.\n{EXAMPLE}\n# Keep following guidance.\n{suffix}"
        );
        let rendered = render_config(&template, None, None, Some("new/repository")).unwrap();

        let mut expected: toml::Value = toml::from_str(&template).unwrap();
        expected["github"]
            .as_table_mut()
            .unwrap()
            .insert("repo".into(), "new/repository".into());
        assert_eq!(toml::from_str::<toml::Value>(&rendered).unwrap(), expected);
        assert!(!rendered.contains(EXAMPLE), "rendered config: {rendered}");
        assert!(rendered.contains("# Keep repository guidance.\n"));
        assert!(rendered.contains("# Keep following guidance.\n"));
    }
}
