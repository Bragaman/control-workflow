// Resources embedded for independent native distribution.
pub const FILES: &[(&str, &str)] = &[
    (
        "examples/config-codex.json",
        include_str!("../examples/config-codex.json"),
    ),
    (
        "examples/config-claude.json",
        include_str!("../examples/config-claude.json"),
    ),
    (
        "examples/project.json",
        include_str!("../examples/project.json"),
    ),
    ("examples/task.md", include_str!("../examples/task.md")),
    ("AGENTS.md", include_str!("../AGENTS.md")),
    ("CLAUDE.md", include_str!("../CLAUDE.md")),
    (
        "docs/agent-review.md",
        include_str!("../docs/agent-review.md"),
    ),
    (
        "docs/architecture.md",
        include_str!("../docs/architecture.md"),
    ),
    (
        "docs/compatibility.md",
        include_str!("../docs/compatibility.md"),
    ),
    (
        "docs/control-entry.md",
        include_str!("../docs/control-entry.md"),
    ),
    ("docs/git-policy.md", include_str!("../docs/git-policy.md")),
    ("docs/install.md", include_str!("../docs/install.md")),
    ("docs/planning.md", include_str!("../docs/planning.md")),
    ("docs/questions.md", include_str!("../docs/questions.md")),
    ("docs/recovery.md", include_str!("../docs/recovery.md")),
    (
        "docs/runtime-layout.md",
        include_str!("../docs/runtime-layout.md"),
    ),
    ("docs/rust.md", include_str!("../docs/rust.md")),
    (
        "docs/task-profile.md",
        include_str!("../docs/task-profile.md"),
    ),
    ("docs/worker.md", include_str!("../docs/worker.md")),
    (
        "skills/control-complete/SKILL.md",
        include_str!("../skills/control-complete/SKILL.md"),
    ),
    (
        "skills/control-list/SKILL.md",
        include_str!("../skills/control-list/SKILL.md"),
    ),
    (
        "skills/control-memory/SKILL.md",
        include_str!("../skills/control-memory/SKILL.md"),
    ),
    (
        "skills/control-project/SKILL.md",
        include_str!("../skills/control-project/SKILL.md"),
    ),
    (
        "skills/control-review/SKILL.md",
        include_str!("../skills/control-review/SKILL.md"),
    ),
    (
        "skills/control-start/SKILL.md",
        include_str!("../skills/control-start/SKILL.md"),
    ),
    (
        "skills/control-sync/SKILL.md",
        include_str!("../skills/control-sync/SKILL.md"),
    ),
    (
        "skills/control-sync/references/checks.md",
        include_str!("../skills/control-sync/references/checks.md"),
    ),
    (
        "skills/control-task/SKILL.md",
        include_str!("../skills/control-task/SKILL.md"),
    ),
];
