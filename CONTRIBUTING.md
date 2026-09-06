# Contributing to HADES

Thank you for your interest in contributing to **HADES**.

HADES is an open-source, universal AI agent CLI runtime. Our goal is to provide developers with a fast, secure, transparent, and user-controlled AI agent interface across any provider, machine, and workflow.

Whether you are fixing a bug, adding a new provider adapter, writing a diagnostic tool, optimizing the terminal UI, experimenting with a new architecture, or improving documentation, we welcome your contributions.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Development Prerequisites](#development-prerequisites)
- [Repository Structure & Crates](#repository-structure--crates)
- [Branching & Contribution Policy](#branching--contribution-policy)
- [Development Workflow](#development-workflow)
- [Extending HADES](#extending-hades)
  - [1. Adding a New Tool](#1-adding-a-new-tool)
  - [2. Adding a New Provider Adapter](#2-adding-a-new-provider-adapter)
  - [3. Adding a New Slash Command](#3-adding-a-new-slash-command)
- [Code Quality & Testing Standards](#code-quality--testing-standards)
- [Submitting a Pull Request](#submitting-a-pull-request)
- [Security and Responsible Contributions](#security-and-responsible-contributions)
- [Community & Support](#community--support)

---

## Code of Conduct

We are committed to providing a friendly, safe, welcoming, and harassment-free environment for all contributors, regardless of experience level, background, or identity.

All contributors and community members are expected to follow the project's Code of Conduct.

Please read the complete [Code of Conduct](CODE_OF_CONDUCT.md) before contributing.

- **Be respectful and constructive** in code reviews, discussions, and issue tracking.
- **Focus on what is best for the project and community**.
- **Show empathy** towards other maintainers and contributors.
- **Keep technical disagreements professional and constructive**.
- **Do not engage in harassment, discrimination, threats, or personal attacks**.

For the complete set of community expectations and standards, see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

---

## Development Prerequisites

To build and contribute to HADES, you will need:

1. **Rust Toolchain**
   - Latest stable Rust is recommended.
   - Install via [rustup](https://rustup.rs/):
     ```bash
     rustup update stable
     rustup component add rustfmt clippy
     ```

2. **C / C++ Compiler & Build Tools**
   - **Windows**: MinGW-w64 or MSVC C++ Build Tools.
   - **Linux**: `build-essential`, `pkg-config`, `libssl-dev`
     ```bash
     sudo apt update && sudo apt install -y build-essential pkg-config libssl-dev
     ```
   - **macOS**: Xcode Command Line Tools
     ```bash
     xcode-select --install
     ```

---

## Repository Structure & Crates

HADES is organized as a modular Cargo workspace consisting of focused crates:

```text
HADES_CLI/
├── assets/                  # Graphics, banners, and documentation assets
├── crates/
│   ├── hades-cli/           # Main executable, CLI argument parsing (clap), MCP server mode
│   ├── hades-config/        # Configuration management (TOML), schema validation
│   ├── hades-core/          # Core coordinator, state machine, context & command registry
│   ├── hades-events/        # Internal async Pub/Sub event bus
│   ├── hades-mcp/           # Model Context Protocol (MCP) client, manager, and server
│   ├── hades-provider/      # Multi-provider adapters, LLM capability engine, streaming
│   ├── hades-storage/       # Session repository, conversation persistence, time formatting
│   ├── hades-tools/         # Built-in sandboxed tools, permission & security engine
│   └── hades-tui/           # Ratatui full-screen TUI, layout, clipboard, and themes
├── Cargo.toml               # Workspace root manifest
├── CONTRIBUTING.md          # Contribution guidelines
├── CODE_OF_CONDUCT.md       # Community Code of Conduct
├── LICENSE                  # MIT License
└── README.md                # Project documentation
```

### Architectural Principles

1. **The Model is the Brain, HADES is the Control Plane**: The LLM suggests actions, but HADES governs execution, sandboxing, and security.
2. **User is the Authority**: Destructive or mutating actions require appropriate user approval.
3. **Workspace Sandboxing**: Filesystem operations are strictly validated against workspace boundaries to prevent directory traversal attacks.
4. **Secret Protection**: API tokens, private keys, and environment credentials must always be redacted in display outputs, logs, and state objects.
5. **Maintainer-Controlled Stability**: The `main` branch represents the stable project state and is protected from direct contributor changes.

---

## Branching & Contribution Policy

HADES follows a structured branch-based contribution workflow.

The repository uses three primary branches for development and contribution:

| Branch | Purpose |
|---|---|
| `issues` | Bug fixes and work related to existing GitHub Issues |
| `features` | Implementation of approved and accepted new features |
| `development` | Research, experimentation, prototyping, and R&D |
| `main` | Stable project branch controlled by maintainers |

The purpose of this structure is to keep experimental work, feature development, issue fixes, and stable project code separated.

### Main Branch Policy

The `main` branch is the stable and protected branch of HADES.

**Contributors must not push directly to `main` or create Pull Requests targeting `main`.**

The normal contributor workflow does not use `main` as a Pull Request target.

Do not:

- Push directly to `main`.
- Create a Pull Request directly against `main`.
- Use `main` as a development branch.
- Use `main` for experimental or unfinished work.
- Force-push to `main`.
- Attempt to bypass repository branch protection.
- Promote your own changes directly into `main`.

Changes are promoted to `main` only through the maintainer-controlled project workflow after the appropriate review and testing process has been completed.

If a Pull Request is opened against the wrong branch, the maintainers may request that it be retargeted or close it and ask for a new Pull Request against the appropriate branch.

### Branch Selection Rules

Every Pull Request must target the branch corresponding to the type of work being submitted.

```text
Bug / Issue Fix
        |
        v
      issues
        |
        v
  Maintainer Review
        |
        v
      main


Approved Feature
        |
        v
     features
        |
        v
  Maintainer Review
        |
        v
      main


Research / R&D
        |
        v
   development
        |
        v
  Maintainer Review
        |
        v
      main
```

Use the following mapping:

| Contribution | Development Branch | Pull Request Target |
|---|---|---|
| Bug fix | `fix/...` | `issues` |
| Existing GitHub Issue | `fix/...` | `issues` |
| Approved feature | `feat/...` | `features` |
| Feature implementation | `feat/...` | `features` |
| Research | `rnd/...` | `development` |
| Experimentation | `rnd/...` | `development` |
| Architecture prototype | `rnd/...` | `development` |

**The development branch name and Pull Request target are separate concepts.**

For example, a contributor may create:

```text
fix/ollama-timeout
```

and the Pull Request should target:

```text
issues
```

Similarly:

```text
feat/browser-automation
```

should target:

```text
features
```

and:

```text
rnd/new-agent-strategy
```

should target:

```text
development
```

---

## Issues Branch

The `issues` branch is intended for fixing bugs, defects, regressions, and other problems tracked through GitHub Issues.

Typical workflow:

```text
GitHub Issue
     |
     v
Investigate the issue
     |
     v
Create a fix branch
     |
     v
Implement the fix
     |
     v
Test the changes
     |
     v
Pull Request
     |
     v
issues
```

When working on an existing issue:

1. Identify the relevant GitHub Issue.
2. Read the issue description and acceptance criteria carefully.
3. Discuss any ambiguity with the maintainers before implementing a substantial change.
4. Create a dedicated branch for the issue.
5. Implement the required fix.
6. Add or update tests where appropriate.
7. Run the required quality checks.
8. Push the branch to your fork.
9. Open a Pull Request targeting the `issues` branch.
10. Reference the relevant GitHub Issue in the Pull Request.

Example:

```bash
git checkout -b fix/issue-description
```

Then:

```bash
git push origin fix/issue-description
```

The Pull Request must target:

```text
issues
```

**Do not target `main`.**

---

## Features Branch

The `features` branch is intended for implementation of new features that have been discussed and accepted for development.

New feature ideas should not immediately move directly into implementation.

### Feature Proposal Process

Before implementing a new feature, follow this process:

```text
New Feature Idea
       |
       v
GitHub Discussions
General Category
       |
       v
Community Discussion
       |
       v
Maintainer Review
       |
       v
Feature Accepted
       |
       v
GitHub Issue
with "feature" label
       |
       v
Feature Implementation
       |
       v
Pull Request
       |
       v
features
       |
       v
Maintainer Review & Testing
       |
       v
main
```

### Step-by-step Feature Process

1. **Start with GitHub Discussions**

   Introduce the feature idea in the **General** GitHub Discussions category.

2. **Explain the proposal**

   Clearly describe:
   - The problem being solved.
   - Why the feature is useful.
   - The proposed behavior.
   - Possible implementation approaches.
   - Potential impact on existing HADES functionality.

3. **Allow discussion**

   The feature should be discussed with the community and maintainers before implementation begins.

4. **Wait for acceptance**

   Do not assume that every feature proposal is automatically approved for implementation.

   A maintainer should confirm that the feature is appropriate for HADES before substantial implementation work begins.

5. **Create a GitHub Issue**

   Once the feature has been accepted for implementation, create a GitHub Issue describing the feature and its requirements.

6. **Apply the `feature` label**

   The feature Issue should have the appropriate `feature` label.

7. **Implement the feature**

   Create a dedicated development branch:

   ```bash
   git checkout -b feat/feature-name
   ```

8. **Test the implementation**

   Add appropriate tests and verify that the implementation does not introduce regressions.

9. **Open the Pull Request**

   Push your branch:

   ```bash
   git push origin feat/feature-name
   ```

   Open the Pull Request against:

   ```text
   features
   ```

10. **Reference the feature Issue**

    The Pull Request should reference the corresponding feature GitHub Issue.

11. **Maintainer review and testing**

    The maintainers will review and test the implementation before it can be considered for promotion to `main`.

### Important

Do not:

- Implement major new features without prior discussion and acceptance.
- Create feature Pull Requests directly against `main`.
- Skip the feature Issue.
- Skip the `feature` label.
- Assume that a feature will be merged simply because the implementation works.

---

## Development Branch

The `development` branch is intended for research, experimentation, prototyping, architectural exploration, and other R&D activities.

This branch may contain work that is not yet considered stable or production-ready.

Examples include:

- Experimental architectures.
- Proofs of concept.
- New agent strategies.
- Experimental provider integrations.
- Performance experiments.
- New approaches to context management.
- Experimental UI and UX implementations.
- Research into new HADES capabilities.
- Architectural prototypes.
- Experimental integrations.
- Early-stage ideas that require technical validation.

Typical workflow:

```text
Research / Experiment
        |
        v
Create R&D Branch
        |
        v
development
        |
        v
Experimentation
        |
        v
Testing / Evaluation
        |
        v
Pull Request
        |
        v
development
```

Example:

```bash
git checkout -b rnd/experiment-name
```

Push the branch:

```bash
git push origin rnd/experiment-name
```

The Pull Request should target:

```text
development
```

The `development` branch should not be treated as the stable release branch.

Experimental work should remain isolated from `main` until it has been reviewed, tested, and explicitly promoted by the maintainers.

---

## Pull Request Targeting Rules

The Pull Request target branch must match the purpose of the contribution.

```text
Existing Issue / Bug Fix
        |
        +----> PR to issues


Approved Feature
        |
        +----> PR to features


Research / Experiment / R&D
        |
        +----> PR to development


Stable Main Branch
        |
        +----> Maintainer-controlled only
```

Before opening a Pull Request, verify the **base repository** and **base branch**.

The expected targets are:

```text
issues       <- Issue fixes and bug fixes
features     <- Approved feature implementations
development  <- R&D and experimental work
main         <- Maintainer-controlled only
```

### No Pull Requests to `main`

**Pull Requests from contributors must not target `main`.**

If you accidentally select `main` as the Pull Request target, change the base branch to the appropriate development branch before submitting.

Pull Requests targeting `main` may be closed or requested to be retargeted by the maintainers.

The repository's branch protection rules are also configured to protect `main` from unauthorized changes.

---

## Promoting Changes to `main`

The `main` branch is maintained and protected by the HADES maintainers.

Contributors do not directly promote their work into `main`.

Changes may be promoted to `main` only after the maintainers have determined that the changes are ready for the stable branch.

The general promotion process is:

```text
issues
   |
   +----> reviewed and tested
   |
   v
main


features
   |
   +----> reviewed and tested
   |
   v
main


development
   |
   +----> evaluated and stabilized
   |
   v
main
```

Before changes are promoted to `main`, maintainers may perform:

- Code review.
- Automated testing.
- Manual testing.
- Security review.
- Compatibility testing.
- Performance evaluation.
- Platform-specific testing.
- Regression testing.
- Architecture review.

Maintainers may merge, modify, request changes to, reject, or revert contributions when necessary.

Passing automated tests does not automatically guarantee acceptance or promotion to `main`.

---

## Development Workflow

### 1. Fork and Clone

Fork the HADES repository to your GitHub account and clone your fork:

```bash
git clone https://github.com/your-username/HADES_CLI.git
cd HADES_CLI
```

Do not create development work directly on `main`.

Create a dedicated branch appropriate for your contribution:

```bash
# Issue fix
git checkout -b fix/issue-description

# Feature
git checkout -b feat/feature-name

# R&D
git checkout -b rnd/experiment-name
```

The resulting Pull Request must target the appropriate HADES branch:

```text
fix/*  -> issues
feat/* -> features
rnd/*  -> development
```

---

### 2. Keep Your Branch Updated

Before beginning or submitting work, make sure your branch is based on an appropriate and recent project state.

Fetch the latest changes:

```bash
git fetch origin
```

If your branch needs to incorporate changes from its target branch, use the appropriate rebase or merge strategy.

For example:

```bash
git pull --rebase origin issues
```

or:

```bash
git pull --rebase origin features
```

or:

```bash
git pull --rebase origin development
```

Do not rewrite or force-push protected branches.

If you need to force-push a rebased development branch, use:

```bash
git push --force-with-lease
```

Do not use force-push against `main`.

---

### 3. Building the Project

Build all crates in debug mode:

```bash
cargo build --workspace
```

Build the optimized release binary:

```bash
cargo build --release
```

---

### 4. Running HADES Locally

Run HADES with the default settings:

```bash
cargo run -p hades-cli
```

Run HADES with a custom configuration:

```bash
cargo run -p hades-cli -- --config ~/.hades/config.toml
```

---

## Extending HADES

### 1. Adding a New Tool

All agent tools reside in `crates/hades-tools/src/` and implement the `Tool` trait.

1. **Create the Tool Struct and Implement `Tool`:**

   ```rust
   use async_trait::async_trait;
   use serde_json::json;
   use crate::context::ToolContext;
   use crate::definition::{RiskLevel, Tool, ToolDefinition, ToolResult};

   pub struct MyCustomTool;

   #[async_trait]
   impl Tool for MyCustomTool {
       fn definition(&self) -> ToolDefinition {
           ToolDefinition::new(
               "custom.tool_name",
               "Clear, detailed description of what the tool accomplishes.",
               json!({
                   "type": "object",
                   "properties": {
                       "query": {
                           "type": "string",
                           "description": "Description of parameter"
                       }
                   },
                   "required": ["query"],
                   "additionalProperties": false
               }),
               RiskLevel::Safe,
               false,
           )
       }

       async fn execute(
           &self,
           call_id: &str,
           input: serde_json::Value,
           context: &ToolContext,
       ) -> ToolResult {
           let query = match input.get("query").and_then(|v| v.as_str()) {
               Some(q) => q,
               None => return ToolResult::invalid_input(
                   call_id,
                   "custom.tool_name",
                   "Missing 'query'",
               ),
           };

           let result_text = format!("Executed query: {query}");
           ToolResult::success(call_id, "custom.tool_name", result_text)
       }
   }
   ```

2. **Register the Tool**

   Register the tool in:

   ```text
   crates/hades-tools/src/registry.rs
   ```

   within `ToolRegistry::default_registry()`.

3. **Add Comprehensive Tests**

   Add appropriate unit tests in a `#[cfg(test)]` module within the tool's file.

4. **Document Security Implications**

   Tools that access or modify system state must clearly document their security and permission requirements.

5. **Respect HADES Permission Controls**

   New tools must correctly classify their risk level and mutating behavior.

---

### 2. Adding a New Provider Adapter

Provider adapters reside in:

```text
crates/hades-provider/src/adapters/
```

and implement the `Provider` trait.

When adding a provider:

1. Implement the required provider interface.
2. Implement model discovery where supported.
3. Implement credential verification.
4. Implement standard completion.
5. Implement streaming completion.
6. Ensure token streaming yields `StreamEvent::Token` chunks asynchronously.
7. Wrap secrets in `CredentialSecret` to prevent accidental exposure in logs or state.
8. Register the adapter in `crates/hades-provider/src/manager.rs`.
9. Add appropriate tests.
10. Document provider-specific configuration requirements.
11. Avoid hardcoding models that may be deprecated or unavailable when dynamic model discovery is supported.
12. Ensure provider-specific failures are reported clearly to users.

Provider integrations should follow the existing HADES provider abstraction rather than introducing provider-specific behavior into unrelated core components.

---

### 3. Adding a New Slash Command

To add a new slash command:

1. Define a struct implementing `Command` in:

   ```text
   crates/hades-core/src/command.rs
   ```

2. Implement:

   ```text
   name()
   description()
   execute(&self, context: &mut CommandContext)
   ```

3. Register the command in:

   ```text
   CommandRegistry::default_registry()
   ```

4. Add tests covering:
   - Valid input.
   - Invalid input.
   - Expected state changes.
   - Error handling.
   - Relevant edge cases.

5. Update documentation if the command is user-facing.

---

## Code Quality & Testing Standards

Before submitting your Pull Request, verify that all quality gates pass.

### 1. Code Formatting

```bash
cargo fmt --all -- --check
```

### 2. Workspace Compilation

```bash
cargo check --workspace --all-targets
```

### 3. Comprehensive Test Suite

```bash
cargo test --workspace
```

### 4. Strict Clippy Linter

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Contributions should not introduce:

- Compiler warnings.
- Formatting failures.
- Test failures.
- Clippy warnings.
- Unnecessary dependencies.
- Unnecessary breaking changes.
- Security regressions.

If a change requires platform-specific behavior, test it on the relevant platform where possible.

For terminal UI changes, provide screenshots or recordings when they help demonstrate the behavior.

For changes involving:

- System interaction.
- Permissions.
- Filesystem access.
- Process execution.
- Networking.
- Browser automation.
- Authentication.
- Credentials.
- Security-sensitive functionality.

include appropriate tests and clearly document the expected behavior.

---

## Submitting a Pull Request

### 1. Verify the Contribution Type

Before opening a Pull Request, determine what type of contribution you are submitting.

```text
Bug / Issue Fix
    |
    -> issues


Approved Feature
    |
    -> features


Research / R&D
    |
    -> development
```

---

### 2. Verify the Target Branch

Before creating your Pull Request, verify that the base branch is correct.

Use this mapping:

```text
Bug / Issue Fix
    -> issues

Approved Feature
    -> features

Research / R&D
    -> development
```

**Do not target `main`.**

The `main` branch is protected and maintained by the HADES maintainers.

---

### 3. Commit Guidelines

Write clear, concise commit messages following the Conventional Commits format.

Examples:

```text
feat: add system diagnostic tool
fix: resolve prompt text wrapping on narrow terminals
docs: update provider setup instructions
refactor: simplify session restoration
test: add provider streaming coverage
chore: update dependencies
```

Commit messages should clearly communicate the purpose of the change.

---

### 4. Push to Your Fork

Push your development branch to your fork:

```bash
git push origin your-branch-name
```

Examples:

```bash
git push origin fix/issue-description
git push origin feat/feature-name
git push origin rnd/experiment-name
```

Do not push development changes directly to the upstream `main` branch.

---

### 5. Open the Pull Request

Open a Pull Request from your fork to the appropriate HADES branch.

```text
Your Fork
   |
   +-- fix/issue-description
   |       |
   |       +----> issues
   |
   +-- feat/feature-name
   |       |
   |       +----> features
   |
   +-- rnd/experiment-name
           |
           +----> development
```

Before submitting the Pull Request, verify:

- The correct repository is selected as the base repository.
- The correct branch is selected as the base branch.
- The correct development branch is selected as the compare/head branch.
- The Pull Request does not target `main`.

---

### 6. Pull Request Description

Your Pull Request should clearly describe:

- What problem is being solved.
- What changes were introduced.
- Why the changes are necessary.
- Which branch the PR targets and why.
- Related GitHub Issue.
- Related GitHub Discussion where applicable.
- Testing performed.
- Any platform-specific considerations.
- Any known limitations.
- Any follow-up work that may be required.

For UI modifications, attach screenshots or recordings where appropriate.

For feature implementations, reference the corresponding feature Issue.

For issue fixes, reference the relevant GitHub Issue.

---

### 7. Review and Validation

Pull Requests are subject to maintainer review and validation.

Maintainers may:

- Request changes.
- Ask for additional tests.
- Request architectural changes.
- Ask for documentation updates.
- Request the Pull Request to be retargeted to the correct branch.
- Reject changes that do not fit the project's direction.
- Modify or revert changes when necessary.
- Perform additional manual testing.

Passing automated checks does not automatically guarantee acceptance.

The maintainers may perform additional validation before changes are accepted or promoted toward `main`.

---

## Security and Responsible Contributions

HADES provides powerful AI-agent capabilities, including system interaction, filesystem operations, process execution, networking, automation, and other potentially sensitive functionality.

Contributions must be designed and implemented responsibly.

The HADES project does not support malicious, abusive, unauthorized, or illegal use of the software.

Do not submit changes intended to:

- Enable unauthorized access to systems or accounts.
- Steal credentials, tokens, secrets, or private information.
- Deploy malware, ransomware, spyware, or destructive payloads.
- Bypass authentication, authorization, permissions, or security controls.
- Facilitate denial-of-service attacks.
- Abuse third-party services.
- Evade security monitoring or safeguards for malicious purposes.
- Facilitate phishing, scams, fraud, or credential theft.
- Facilitate illegal or harmful activity.
- Expose private or sensitive user information.

Legitimate security research, defensive security work, vulnerability analysis, testing, and responsible disclosure are welcome when conducted in an authorized and responsible context.

Never commit or publish:

- API keys.
- Passwords.
- Access tokens.
- Private keys.
- Credentials.
- Session tokens.
- Personal information.
- Other sensitive secrets.

If you discover a security vulnerability in HADES, do not publicly publish sensitive exploit details through Issues or Discussions. Follow the project's designated security reporting process.

---

## Community & Support

- **Issues**: Report bugs, defects, regressions, and tracked problems through GitHub Issues.
- **Discussions**: Discuss HADES-related ideas, questions, architecture, workflows, integrations, proposals, and community topics through GitHub Discussions.
- **Features**: New feature ideas should be discussed in GitHub Discussions before implementation and should receive an appropriate GitHub Issue with the `feature` label before development begins.
- **Development**: Experimental and R&D work should be isolated in the `development` branch.
- **Code of Conduct**: Please read and follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
- **License**: By contributing to HADES, you agree that your contributions will be licensed under the [MIT License](LICENSE).

---

## Final Note

HADES is built through open-source collaboration.

Contributors are encouraged to ask questions, propose ideas, experiment, improve the implementation, and challenge existing approaches constructively.

The project's branch structure exists to keep experimentation, feature development, issue fixes, and the stable codebase properly separated.

Please:

1. Choose the correct contribution path.
2. Discuss new features before implementation.
3. Create and reference the appropriate GitHub Issue.
4. Work on a dedicated development branch.
5. Target the correct Pull Request branch.
6. Run the required tests and quality checks.
7. Keep sensitive information out of the repository.
8. Do not target or modify `main` directly.
9. Respect the project's Code of Conduct.
10. Follow responsible and authorized use practices.

The `main` branch is maintained as the stable HADES codebase and is protected accordingly.

Thank you for contributing to HADES.
