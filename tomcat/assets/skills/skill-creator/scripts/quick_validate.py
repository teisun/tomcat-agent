#!/usr/bin/env python3
"""Validate a portable Tomcat skill's YAML header and unfinished scaffold."""
import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:
    raise SystemExit("quick_validate.py requires PyYAML; install it in this Python environment")

NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
HEADER_LIMIT = 4096
DESCRIPTION_LIMIT = 1024
TODO = re.compile(r"(?:\[TODO:|\bTODO:\s*(?:add|describe|replace|write|give)\b)", re.IGNORECASE)


class UniqueKeyLoader(yaml.SafeLoader):
    pass


def construct_mapping(loader, node, deep=False):
    mapping = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in mapping:
            raise yaml.YAMLError(f"duplicate key: {key!r}")
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


UniqueKeyLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, construct_mapping)


def fail(message: str) -> None:
    print(message)
    raise SystemExit(1)


def main() -> None:
    if len(sys.argv) != 2:
        fail("Usage: quick_validate.py <skill-directory>")
    root = Path(sys.argv[1])
    if not root.is_dir():
        fail(f"skill directory does not exist: {root}")
    skill = root / "SKILL.md"
    try:
        raw = skill.read_bytes()
        text = raw.decode("utf-8")
    except (OSError, UnicodeDecodeError) as error:
        fail(f"cannot read UTF-8 {skill}: {error}")
    if not text.startswith("---\n"):
        fail("SKILL.md must begin with a complete YAML frontmatter delimiter")
    end = text.find("\n---\n", 4)
    if end < 0:
        fail("SKILL.md frontmatter is not closed by a complete delimiter line")
    header_bytes = raw[: end + 5]
    if len(header_bytes) > HEADER_LIMIT:
        fail(f"SKILL.md frontmatter exceeds {HEADER_LIMIT} UTF-8 bytes")
    try:
        header = yaml.load(text[4:end], Loader=UniqueKeyLoader)
    except yaml.YAMLError as error:
        fail(f"invalid YAML frontmatter: {error}")
    if not isinstance(header, dict):
        fail("frontmatter must be a YAML mapping")
    allowed = {"name", "description", "license", "compatibility", "metadata"}
    unexpected = sorted(set(header) - allowed)
    if unexpected:
        fail(f"unexpected frontmatter key(s): {', '.join(unexpected)}")
    name = header.get("name")
    description = header.get("description")
    if not isinstance(name, str) or not NAME.fullmatch(name):
        fail("name must be a quoted-or-plain lowercase hyphen-case YAML string of at most 64 characters")
    if root.name != name:
        fail(f"directory name {root.name!r} must match frontmatter name {name!r}")
    if not isinstance(description, str) or not description.strip() or "\n" in description:
        fail("description must be a non-empty single-line YAML string")
    if len(description) > DESCRIPTION_LIMIT:
        fail(f"description exceeds {DESCRIPTION_LIMIT} characters")
    license_value = header.get("license")
    if license_value is not None and not isinstance(license_value, str):
        fail("license must be a YAML string when provided")
    compatibility = header.get("compatibility")
    if compatibility is not None and (
        not isinstance(compatibility, str)
        or not compatibility.strip()
        or len(compatibility) > 500
    ):
        fail("compatibility must be a non-empty YAML string of at most 500 characters")
    metadata = header.get("metadata")
    if metadata is not None and (
        not isinstance(metadata, dict)
        or any(not isinstance(key, str) or not isinstance(value, str) for key, value in metadata.items())
    ):
        fail("metadata must be a mapping of YAML strings to YAML strings")
    body = text[end + 5 :]
    if not body.strip():
        fail("skill instructions must not be empty")
    if TODO.search(description) or TODO.search(body):
        fail("skill contains an unfinished TODO placeholder")
    print("Skill is valid for Tomcat.")


if __name__ == "__main__":
    main()
