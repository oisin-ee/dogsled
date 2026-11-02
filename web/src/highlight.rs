//! Class-based highlighting for fenced Markdown code, never for raw HTML.

use std::sync::LazyLock;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Unknown languages and parse errors leave escaping to the Markdown renderer.
pub(crate) fn fenced(info: &str, code: &str) -> Option<String> {
    let language = info.split_whitespace().next()?.to_ascii_lowercase();
    let token = match language.as_str() {
        "rs" | "rust" => "rs",
        "bash" | "sh" | "shell" => "sh",
        "js" | "javascript" => "js",
        "ts" | "typescript" => "ts",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" => "md",
        "py" | "python" => "py",
        other => other,
    };
    let syntax = SYNTAXES.find_syntax_by_token(token)?;
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        &SYNTAXES,
        ClassStyle::SpacedPrefixed { prefix: "syn-" },
    );
    for line in LinesWithEndings::from(code) {
        generator
            .parse_html_for_line_which_includes_newline(line)
            .ok()?;
    }
    Some(generator.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_languages_and_aliases_have_classed_output() {
        for (info, code) in [
            ("rust", "fn main() {}\n"),
            ("rs", "fn main() {}\n"),
            ("bash", "echo 'hello'\n"),
            ("sh", "echo 'hello'\n"),
            ("shell", "echo 'hello'\n"),
            ("toml", "[package]\nname = \"web\"\n"),
            ("json", "{\"name\": true}\n"),
            ("yaml", "name: web\n"),
            ("yml", "name: web\n"),
            ("typescript", "const n: number = 1;\n"),
            ("ts", "const n: number = 1;\n"),
            ("javascript", "const n = 1;\n"),
            ("js", "const n = 1;\n"),
            ("diff", "+added\n-removed\n"),
            ("markdown", "# Heading\n"),
            ("md", "# Heading\n"),
            ("python", "def main(): pass\n"),
            ("py", "def main(): pass\n"),
            ("sql", "SELECT * FROM users;\n"),
            ("Rust title=example", "fn main() {}\n"),
        ] {
            let html = fenced(info, code);
            assert!(
                html.as_ref().is_some_and(|html| html.contains("syn-")),
                "{info}: {html:?}"
            );
        }
    }

    #[test]
    fn unknown_and_missing_languages_use_the_plain_text_path() {
        assert!(fenced("", "<script>alert(1)</script>").is_none());
        assert!(fenced("not-a-real-language", "<script>alert(1)</script>").is_none());
    }

    #[test]
    fn highlighted_html_escapes_source_and_preserves_line_endings() {
        let html = fenced("sh", "<script>\n\n</script>");
        assert!(
            html.as_ref().is_some_and(|html| {
                !html.contains("<script>")
                    && html.contains("&lt;")
                    && html.contains("&gt;")
                    && html.matches('\n').count() == 2
            }),
            "{html:?}"
        );
    }
}
