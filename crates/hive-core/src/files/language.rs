//! The editor language name for a file path.
use std::path::Path;

/// Language name the editor highlighter understands for `path`.
///
/// Most files are named by their lowercase extension; module and JSX variants map to their
/// grammar (`.jsx` to `javascript`, `.mts` to `typescript`), and well-known extensionless
/// build files are recognized by name, so `Makefile` highlights as `make`. An empty string
/// means plain text.
pub fn editor_language(path: &Path) -> String {
    let name = lowercase_name(path);
    if matches!(name.as_str(), "makefile" | "gnumakefile") {
        return "make".to_owned();
    }
    // The highlighter has no Dockerfile grammar; its instructions are mostly shell commands,
    // so bash colors comments, strings, variables and the instruction keywords well enough.
    if is_dockerfile_name(&name) {
        return "bash".to_owned();
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // The highlighter knows these grammars only by their main extension; the JavaScript
    // grammar parses JSX too.
    match extension.as_str() {
        "mk" | "mak" => "make".to_owned(),
        "jsx" | "mjs" | "cjs" => "javascript".to_owned(),
        "mts" | "cts" => "typescript".to_owned(),
        _ => extension,
    }
}

/// Whether `path` is a Dockerfile, including variants such as `Dockerfile-base`,
/// `Dockerfile.dev` or `api.Dockerfile`.
pub fn is_dockerfile(path: &Path) -> bool {
    is_dockerfile_name(&lowercase_name(path))
}

fn is_dockerfile_name(lowercase_name: &str) -> bool {
    lowercase_name.starts_with("dockerfile") || lowercase_name.ends_with(".dockerfile")
}

fn lowercase_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{editor_language, is_dockerfile};
    use std::path::Path;

    #[test]
    fn extension_names_the_language() {
        assert_eq!(editor_language(Path::new("src/main.RS")), "rs");
        assert_eq!(editor_language(Path::new("README")), "");
    }

    #[test]
    fn script_variants_use_their_grammar() {
        for (path, language) in [
            ("app/layout.jsx", "javascript"),
            ("server.mjs", "javascript"),
            ("config.cjs", "javascript"),
            ("vite.config.mts", "typescript"),
            ("rules.cts", "typescript"),
            ("page.tsx", "tsx"),
            ("index.js", "js"),
        ] {
            assert_eq!(editor_language(Path::new(path)), language, "{path}");
        }
    }

    #[test]
    fn makefiles_are_make() {
        for path in [
            "Makefile",
            "dir/makefile",
            "GNUmakefile",
            "rules.mk",
            "build.mak",
        ] {
            assert_eq!(editor_language(Path::new(path)), "make", "{path}");
        }
        assert_eq!(editor_language(Path::new("Makefile.am")), "am");
    }

    #[test]
    fn dockerfiles_are_bash() {
        for path in [
            "Dockerfile",
            "Dockerfile-base",
            "Dockerfile-nodejs.tmpl",
            "Dockerfile.dev",
            "api.Dockerfile",
        ] {
            assert!(is_dockerfile(Path::new(path)), "{path}");
            assert_eq!(editor_language(Path::new(path)), "bash", "{path}");
        }
        assert!(!is_dockerfile(Path::new("docker-compose.yml")));
        assert!(!is_dockerfile(Path::new(".dockerignore")));
    }
}
