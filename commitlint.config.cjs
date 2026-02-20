module.exports = {
  extends: ["@commitlint/config-conventional"],
  rules: {
    // Keep Conventional Commits, but allow project-specific types already used in history.
    "type-enum": [
      2,
      "always",
      [
        "build",
        "chore",
        "ci",
        "docs",
        "feat",
        "fix",
        "perf",
        "refactor",
        "revert",
        "style",
        "test",
        "ui",
        "install",
      ],
    ],
    // Be pragmatic for long descriptions (especially in Chinese).
    "body-max-line-length": [1, "always", 200],
  },
};
