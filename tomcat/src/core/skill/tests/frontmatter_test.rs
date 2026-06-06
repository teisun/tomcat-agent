use crate::core::skill::frontmatter::{
    parse, split_frontmatter, strip_frontmatter, SkillParseError, ALLOWED_SKILL_FRONTMATTER,
};

#[test]
fn parse_requires_name_and_description() {
    let text = r#"---
name: commit
description: Create a git commit.
---
# Commit
"#;
    let frontmatter = parse(text).expect("frontmatter should parse");
    assert_eq!(frontmatter.name, "commit");
    assert_eq!(frontmatter.description, "Create a git commit.");
}

#[test]
fn unknown_keys_ignored() {
    let text = r#"---
name: commit
description: Create a git commit.
custom-field: keep-forward-compatible
metadata:
  category: git
---
# Commit
"#;
    let frontmatter = parse(text).expect("unknown keys should be ignored");
    assert_eq!(frontmatter.name, "commit");
    assert_eq!(frontmatter.metadata.len(), 1);
    assert!(ALLOWED_SKILL_FRONTMATTER.contains(&"metadata"));
}

#[test]
fn rejects_missing_frontmatter() {
    let text = "# Commit\n";
    let err = parse(text).expect_err("missing frontmatter must fail");
    assert_eq!(err, SkillParseError::FrontmatterDelimMissing);
}

#[test]
fn parse_rejects_missing_name() {
    let text = r#"---
description: Create a git commit.
---
"#;
    let err = parse(text).expect_err("missing name must fail");
    assert_eq!(err, SkillParseError::MissingField { field: "name" });
}

#[test]
fn parse_rejects_missing_description() {
    let text = r#"---
name: commit
---
"#;
    let err = parse(text).expect_err("missing description must fail");
    assert_eq!(
        err,
        SkillParseError::MissingField {
            field: "description"
        }
    );
}

#[test]
fn parse_allows_allowed_tools_string_or_list() {
    let one = r#"---
name: commit
description: Create a git commit.
allowed-tools: bash
---
"#;
    let many = r#"---
name: commit
description: Create a git commit.
allowed-tools: [bash, read]
---
"#;
    assert_eq!(parse(one).unwrap().allowed_tools, Some(vec!["bash".into()]));
    assert_eq!(
        parse(many).unwrap().allowed_tools,
        Some(vec!["bash".into(), "read".into()])
    );
}

#[test]
fn split_and_strip_frontmatter_preserve_body() {
    let text =
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n1. Run git status.\n";
    let (yaml, body) = split_frontmatter(text).expect("split should succeed");
    assert!(yaml.contains("name: commit"));
    assert_eq!(body, "# Commit\n1. Run git status.\n");
    assert_eq!(
        strip_frontmatter(text).unwrap(),
        "# Commit\n1. Run git status.\n"
    );
}
