//! AI command-line tools published to registries PkgDeck cannot search: npm
//! and PyPI. Searching npm, pnpm, Bun, pipx or uv for a product name or alias
//! offers the exact package below; a name alone never picks a package.
//! Homebrew needs no entries: its own search already finds `copilot-cli`,
//! `kiro-cli` or `block-goose-cli`. Checked against each vendor's install docs
//! on 2026-09-27 (see docs/support-expansion-report.md).

pub(super) struct AiTool {
    pub product: &'static str,
    /// Lowercase words people search for, besides the product name.
    pub terms: &'static [&'static str],
    pub npm: Option<&'static str>,
    pub pypi: Option<&'static str>,
}

pub(super) const AI_TOOLS: &[AiTool] = &[
    AiTool {
        product: "Claude Code",
        terms: &["claude", "anthropic"],
        npm: Some("@anthropic-ai/claude-code"),
        pypi: None,
    },
    AiTool {
        product: "Codex CLI",
        terms: &["codex", "openai"],
        npm: Some("@openai/codex"),
        pypi: None,
    },
    AiTool {
        product: "GitHub Copilot CLI",
        terms: &["copilot", "github"],
        npm: Some("@github/copilot"),
        pypi: None,
    },
    AiTool {
        product: "Grok Build",
        terms: &["grok", "xai"],
        npm: Some("@xai-official/grok"),
        pypi: None,
    },
    AiTool {
        product: "OpenCode",
        terms: &["opencode"],
        npm: Some("opencode-ai"),
        pypi: None,
    },
    AiTool {
        product: "Amp",
        terms: &["amp", "sourcegraph"],
        npm: Some("@sourcegraph/amp"),
        pypi: None,
    },
    AiTool {
        product: "Factory Droid",
        terms: &["droid", "factory"],
        npm: Some("@factory/cli"),
        pypi: None,
    },
    AiTool {
        product: "Qwen Code",
        terms: &["qwen", "alibaba"],
        npm: Some("@qwen-code/qwen-code"),
        pypi: None,
    },
    AiTool {
        product: "Crush",
        terms: &["crush", "charm"],
        npm: Some("@charmland/crush"),
        pypi: None,
    },
    AiTool {
        product: "Cline CLI",
        terms: &["cline"],
        npm: Some("cline"),
        pypi: None,
    },
    AiTool {
        product: "Kilo CLI",
        terms: &["kilo", "kilocode"],
        npm: Some("@kilocode/cli"),
        pypi: None,
    },
    AiTool {
        product: "Kimi Code",
        terms: &["kimi", "moonshot"],
        npm: None,
        pypi: Some("kimi-code"),
    },
    AiTool {
        product: "Mistral Vibe",
        terms: &["vibe", "mistral"],
        npm: None,
        pypi: Some("mistral-vibe"),
    },
    AiTool {
        product: "Aider",
        terms: &["aider"],
        npm: None,
        pypi: Some("aider-chat"),
    },
];

/// Catalog tools whose product name, search terms or package match a query.
/// Queries shorter than three characters match nothing, so a short prefix
/// does not list most of the catalog.
pub(super) fn matching(query: &str) -> impl Iterator<Item = &'static AiTool> {
    let query = query.trim().to_ascii_lowercase();
    AI_TOOLS.iter().filter(move |tool| {
        query.len() >= 3
            && (tool.product.to_ascii_lowercase().contains(&query)
                || tool.terms.iter().any(|term| term.contains(query.as_str()))
                || tool.npm.is_some_and(|name| name.contains(query.as_str()))
                || tool.pypi.is_some_and(|name| name.contains(query.as_str())))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_packages_are_valid_registry_names() {
        for tool in AI_TOOLS {
            assert!(
                tool.npm.is_some() || tool.pypi.is_some(),
                "{}",
                tool.product
            );
            assert!(
                tool.npm.is_none_or(super::super::dev_name),
                "{}",
                tool.product
            );
            assert!(
                tool.pypi.is_none_or(super::super::python_name),
                "{}",
                tool.product
            );
            assert!(tool
                .terms
                .iter()
                .all(|term| *term == term.to_ascii_lowercase()));
        }
    }

    #[test]
    fn products_are_found_by_name_term_or_package() {
        let names = |query| matching(query).map(|tool| tool.product).collect::<Vec<_>>();
        assert_eq!(names("copilot"), ["GitHub Copilot CLI"]);
        assert_eq!(names("Claude"), ["Claude Code"]);
        assert_eq!(names("aider-chat"), ["Aider"]);
        assert_eq!(names("@factory"), ["Factory Droid"]);
        assert!(names("co").is_empty());
        assert!(names("").is_empty());
        // Gemini CLI is deprecated for consumer accounts; Antigravity CLI has
        // no npm or PyPI package, so neither is offered here.
        assert!(names("gemini").is_empty());
        assert!(names("antigravity").is_empty());
    }
}
